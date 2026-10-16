package objectstore

import (
	"bytes"
	"context"
	"crypto/md5"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

type Settings struct {
	Endpoint    string
	Region      string
	AccessKey   string
	SecretKey   string
	CA          string
	Timeout     time.Duration
	Attempts    int
	BaseBackoff time.Duration
}

type Client struct {
	endpoint    *url.URL
	region      string
	accessKey   string
	secretKey   string
	http        *http.Client
	attempts    int
	baseBackoff time.Duration
	now         func() time.Time
}

type Object struct {
	Key          string
	Size         int64
	LastModified time.Time
}

type Error struct {
	Status  int
	Code    string
	Message string
}

func (failure *Error) Error() string {
	return fmt.Sprintf("object store answered %d %s: %s", failure.Status, failure.Code, failure.Message)
}

var ErrNoLifecycle = errors.New("the bucket has no lifecycle configuration")

func New(settings Settings) (*Client, error) {
	endpoint, failure := url.Parse(strings.TrimRight(settings.Endpoint, "/"))
	if failure != nil || endpoint.Host == "" || (endpoint.Scheme != "http" && endpoint.Scheme != "https") {
		return nil, fmt.Errorf("the object store endpoint %q is not an http or https URL", settings.Endpoint)
	}
	transport := http.DefaultTransport.(*http.Transport).Clone()
	if settings.CA != "" {
		pem, failure := os.ReadFile(settings.CA)
		if failure != nil {
			return nil, fmt.Errorf("read the object store CA: %w", failure)
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(pem) {
			return nil, fmt.Errorf("%s holds no PEM certificate", settings.CA)
		}
		transport.TLSClientConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
	}
	if settings.Timeout <= 0 {
		settings.Timeout = time.Minute
	}
	if settings.Attempts <= 0 {
		settings.Attempts = 4
	}
	if settings.BaseBackoff <= 0 {
		settings.BaseBackoff = 500 * time.Millisecond
	}
	return &Client{endpoint: endpoint, region: settings.Region, accessKey: settings.AccessKey, secretKey: settings.SecretKey,
		http: &http.Client{Timeout: settings.Timeout, Transport: transport}, attempts: settings.Attempts, baseBackoff: settings.BaseBackoff, now: time.Now}, nil
}

func (client *Client) location(bucket, key string, query url.Values) *url.URL {
	target := *client.endpoint
	target.Path = strings.TrimRight(target.Path, "/") + "/" + bucket
	if key != "" {
		target.Path += "/" + key
	}
	target.RawPath = encodeComponent(target.Path, true)
	target.RawQuery = query.Encode()
	return &target
}

func retryable(status int) bool {
	return status == http.StatusTooManyRequests || status >= http.StatusInternalServerError
}

func (client *Client) do(operation context.Context, method string, target *url.URL, body []byte, headers map[string]string) ([]byte, error) {
	var lastFailure error
	for attempt := range client.attempts {
		if attempt > 0 {
			ceiling := client.baseBackoff << min(attempt, 6)
			select {
			case <-operation.Done():
				return nil, operation.Err()
			case <-time.After(time.Duration(rand.Int64N(int64(ceiling)) + 1)):
			}
		}
		request, failure := http.NewRequestWithContext(operation, method, target.String(), bytes.NewReader(body))
		if failure != nil {
			return nil, failure
		}
		for name, value := range headers {
			request.Header.Set(name, value)
		}
		sign(request, client.accessKey, client.secretKey, client.region, hashHex(body), client.now())
		response, failure := client.http.Do(request)
		if failure != nil {
			lastFailure = failure
			if operation.Err() != nil {
				return nil, operation.Err()
			}
			continue
		}
		content, failure := io.ReadAll(io.LimitReader(response.Body, 64<<20))
		response.Body.Close()
		if failure != nil {
			lastFailure = failure
			continue
		}
		if response.StatusCode/100 == 2 {
			return content, nil
		}
		answered := &Error{Status: response.StatusCode}
		var parsed struct {
			Code    string `xml:"Code"`
			Message string `xml:"Message"`
		}
		if xml.Unmarshal(content, &parsed) == nil {
			answered.Code, answered.Message = parsed.Code, parsed.Message
		}
		if !retryable(response.StatusCode) {
			return nil, answered
		}
		lastFailure = answered
	}
	return nil, fmt.Errorf("%s %s failed after %d attempts: %w", method, target.Path, client.attempts, lastFailure)
}

func (client *Client) List(operation context.Context, bucket, prefix string, visit func(Object) error) error {
	continuation := ""
	for {
		query := url.Values{"list-type": {"2"}, "prefix": {prefix}, "max-keys": {"1000"}}
		if continuation != "" {
			query.Set("continuation-token", continuation)
		}
		content, failure := client.do(operation, http.MethodGet, client.location(bucket, "", query), nil, nil)
		if failure != nil {
			return failure
		}
		var page struct {
			IsTruncated           bool   `xml:"IsTruncated"`
			NextContinuationToken string `xml:"NextContinuationToken"`
			Contents              []struct {
				Key          string    `xml:"Key"`
				Size         int64     `xml:"Size"`
				LastModified time.Time `xml:"LastModified"`
			} `xml:"Contents"`
		}
		if failure := xml.Unmarshal(content, &page); failure != nil {
			return fmt.Errorf("parse the listing of %s/%s: %w", bucket, prefix, failure)
		}
		for _, entry := range page.Contents {
			if failure := visit(Object{Key: entry.Key, Size: entry.Size, LastModified: entry.LastModified}); failure != nil {
				return failure
			}
		}
		if !page.IsTruncated || page.NextContinuationToken == "" {
			return nil
		}
		continuation = page.NextContinuationToken
	}
}

func (client *Client) DeleteObject(operation context.Context, bucket, key string) error {
	_, failure := client.do(operation, http.MethodDelete, client.location(bucket, key, nil), nil, nil)
	var answered *Error
	if errors.As(failure, &answered) && answered.Status == http.StatusNotFound {
		return nil
	}
	return failure
}

func (client *Client) DeleteObjects(operation context.Context, bucket string, keys []string) error {
	for start := 0; start < len(keys); start += 1000 {
		batch := keys[start:min(start+1000, len(keys))]
		type entry struct {
			Key string `xml:"Key"`
		}
		document := struct {
			XMLName xml.Name `xml:"Delete"`
			Quiet   bool     `xml:"Quiet"`
			Objects []entry  `xml:"Object"`
		}{Quiet: true}
		for _, key := range batch {
			document.Objects = append(document.Objects, entry{Key: key})
		}
		body, failure := xml.Marshal(document)
		if failure != nil {
			return failure
		}
		digest := md5.Sum(body)
		content, failure := client.do(operation, http.MethodPost, client.location(bucket, "", url.Values{"delete": {""}}), body,
			map[string]string{"Content-Type": "application/xml", "Content-Md5": base64.StdEncoding.EncodeToString(digest[:])})
		if failure != nil {
			return failure
		}
		var result struct {
			Errors []struct {
				Key     string `xml:"Key"`
				Code    string `xml:"Code"`
				Message string `xml:"Message"`
			} `xml:"Error"`
		}
		if failure := xml.Unmarshal(content, &result); failure != nil {
			return fmt.Errorf("parse the answer to deleting %d objects from %s: %w", len(batch), bucket, failure)
		}
		for _, refused := range result.Errors {
			if refused.Code != "NoSuchKey" {
				return fmt.Errorf("the object store did not delete %s/%s: %s %s", bucket, refused.Key, refused.Code, refused.Message)
			}
		}
	}
	return nil
}

type Retention struct {
	Mode string
	Days int
}

func (client *Client) ObjectLockRetention(operation context.Context, bucket string) (Retention, error) {
	content, failure := client.do(operation, http.MethodGet, client.location(bucket, "", url.Values{"object-lock": {""}}), nil, nil)
	if failure != nil {
		return Retention{}, failure
	}
	var document struct {
		Rule struct {
			DefaultRetention struct {
				Mode  string `xml:"Mode"`
				Days  int    `xml:"Days"`
				Years int    `xml:"Years"`
			} `xml:"DefaultRetention"`
		} `xml:"Rule"`
	}
	if failure := xml.Unmarshal(content, &document); failure != nil {
		return Retention{}, fmt.Errorf("parse the object lock configuration of %s: %w", bucket, failure)
	}
	retention := document.Rule.DefaultRetention
	return Retention{Mode: retention.Mode, Days: retention.Days + 365*retention.Years}, nil
}

type ExpirationRule struct {
	Prefix string
	Days   int
}

func (client *Client) ExpirationRules(operation context.Context, bucket string) ([]ExpirationRule, error) {
	content, failure := client.do(operation, http.MethodGet, client.location(bucket, "", url.Values{"lifecycle": {""}}), nil, nil)
	var answered *Error
	if errors.As(failure, &answered) && answered.Code == "NoSuchLifecycleConfiguration" {
		return nil, ErrNoLifecycle
	}
	if failure != nil {
		return nil, failure
	}
	var document struct {
		Rules []struct {
			Status string `xml:"Status"`
			Prefix string `xml:"Prefix"`
			Filter struct {
				Prefix string `xml:"Prefix"`
			} `xml:"Filter"`
			Expiration struct {
				Days int `xml:"Days"`
			} `xml:"Expiration"`
		} `xml:"Rule"`
	}
	if failure := xml.Unmarshal(content, &document); failure != nil {
		return nil, fmt.Errorf("parse the lifecycle configuration of %s: %w", bucket, failure)
	}
	var rules []ExpirationRule
	for _, rule := range document.Rules {
		if rule.Status != "Enabled" || rule.Expiration.Days == 0 {
			continue
		}
		prefix := rule.Filter.Prefix
		if prefix == "" {
			prefix = rule.Prefix
		}
		rules = append(rules, ExpirationRule{Prefix: prefix, Days: rule.Expiration.Days})
	}
	return rules, nil
}

func ExpirationFor(rules []ExpirationRule, key string) (int, bool) {
	best, days := -1, 0
	for _, rule := range rules {
		if strings.HasPrefix(key, rule.Prefix) && len(rule.Prefix) > best {
			best, days = len(rule.Prefix), rule.Days
		}
	}
	return days, best >= 0
}
