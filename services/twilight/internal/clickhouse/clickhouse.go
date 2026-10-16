package clickhouse

import (
	"bufio"
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

type Settings struct {
	URL      string
	User     string
	Password string
	CA       string
	Timeout  time.Duration
}

type Client struct {
	endpoint *url.URL
	user     string
	password string
	http     *http.Client
}

type Error struct {
	Status  int
	Message string
}

func (failure *Error) Error() string {
	return fmt.Sprintf("clickhouse answered %d: %s", failure.Status, failure.Message)
}

func New(settings Settings) (*Client, error) {
	endpoint, failure := url.Parse(strings.TrimRight(settings.URL, "/") + "/")
	if failure != nil || endpoint.Host == "" || (endpoint.Scheme != "http" && endpoint.Scheme != "https") {
		return nil, fmt.Errorf("the clickhouse URL %q is not an http or https URL", settings.URL)
	}
	transport := http.DefaultTransport.(*http.Transport).Clone()
	if settings.CA != "" {
		pem, failure := os.ReadFile(settings.CA)
		if failure != nil {
			return nil, fmt.Errorf("read the clickhouse CA: %w", failure)
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(pem) {
			return nil, fmt.Errorf("%s holds no PEM certificate", settings.CA)
		}
		transport.TLSClientConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
	}
	if settings.Timeout <= 0 {
		settings.Timeout = 10 * time.Minute
	}
	return &Client{endpoint: endpoint, user: settings.User, password: settings.Password, http: &http.Client{Timeout: settings.Timeout, Transport: transport}}, nil
}

type Query struct {
	Text       string
	Parameters map[string]string
	Settings   map[string]string
	Body       io.Reader
}

func (client *Client) send(operation context.Context, query Query) (*http.Response, error) {
	values := url.Values{}
	for name, value := range query.Parameters {
		values.Set("param_"+name, value)
	}
	for name, value := range query.Settings {
		values.Set(name, value)
	}
	body := query.Body
	if body == nil {
		body = strings.NewReader(query.Text)
	} else {
		values.Set("query", query.Text)
	}
	target := *client.endpoint
	target.RawQuery = values.Encode()
	request, failure := http.NewRequestWithContext(operation, http.MethodPost, target.String(), body)
	if failure != nil {
		return nil, failure
	}
	request.Header.Set("X-ClickHouse-User", client.user)
	request.Header.Set("X-ClickHouse-Key", client.password)
	response, failure := client.http.Do(request)
	if failure != nil {
		return nil, failure
	}
	if response.StatusCode != http.StatusOK {
		content, _ := io.ReadAll(io.LimitReader(response.Body, 64<<10))
		response.Body.Close()
		return nil, &Error{Status: response.StatusCode, Message: strings.TrimSpace(string(content))}
	}
	return response, nil
}

func (client *Client) Exec(operation context.Context, query Query) error {
	response, failure := client.send(operation, query)
	if failure != nil {
		return failure
	}
	defer response.Body.Close()
	content, failure := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if failure != nil {
		return fmt.Errorf("read the clickhouse answer: %w", failure)
	}
	if exception := exceptionIn(content); exception != "" {
		return &Error{Status: http.StatusOK, Message: exception}
	}
	return nil
}

func exceptionIn(content []byte) string {
	if index := bytes.Index(content, []byte("Code: ")); index >= 0 && bytes.Contains(content[index:], []byte("Exception")) {
		return strings.TrimSpace(string(content[index:]))
	}
	return ""
}

func (client *Client) Rows(operation context.Context, query Query, visit func(line []byte) error) error {
	if query.Settings == nil {
		query.Settings = map[string]string{}
	}
	if _, chosen := query.Settings["output_format_json_quote_64bit_integers"]; !chosen {
		query.Settings["output_format_json_quote_64bit_integers"] = "0"
	}
	query.Settings["http_write_exception_in_output_format"] = "1"
	query.Text = strings.TrimSpace(query.Text) + " FORMAT JSONEachRow"
	response, failure := client.send(operation, query)
	if failure != nil {
		return failure
	}
	defer response.Body.Close()
	scanner := bufio.NewScanner(response.Body)
	scanner.Buffer(make([]byte, 0, 1<<16), 64<<20)
	for scanner.Scan() {
		line := scanner.Bytes()
		if len(bytes.TrimSpace(line)) == 0 {
			continue
		}
		if bytes.HasPrefix(line, []byte(`{"exception"`)) {
			var exception struct {
				Exception string `json:"exception"`
			}
			_ = json.Unmarshal(line, &exception)
			return &Error{Status: http.StatusOK, Message: exception.Exception}
		}
		if exception := exceptionIn(line); exception != "" && !json.Valid(line) {
			return &Error{Status: http.StatusOK, Message: exception}
		}
		if failure := visit(line); failure != nil {
			return failure
		}
	}
	if failure := scanner.Err(); failure != nil {
		return fmt.Errorf("read the clickhouse rows: %w", failure)
	}
	return nil
}

func Scan[Row any](operation context.Context, client *Client, query Query) ([]Row, error) {
	var rows []Row
	failure := client.Rows(operation, query, func(line []byte) error {
		var row Row
		if failure := json.Unmarshal(line, &row); failure != nil {
			return fmt.Errorf("decode a clickhouse row: %w", failure)
		}
		rows = append(rows, row)
		return nil
	})
	return rows, failure
}

func Quote(text string) string {
	return "'" + strings.NewReplacer(`\`, `\\`, `'`, `\'`).Replace(text) + "'"
}
