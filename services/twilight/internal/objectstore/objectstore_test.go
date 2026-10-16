package objectstore

import (
	"context"
	"crypto/md5"
	"encoding/base64"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestSignatureMatchesTheAWSGetObjectExample(test *testing.T) {
	request, failure := http.NewRequest(http.MethodGet, "https://examplebucket.s3.amazonaws.com/test.txt", nil)
	if failure != nil {
		test.Fatal(failure)
	}
	request.Header.Set("Range", "bytes=0-9")
	at := time.Date(2013, 5, 24, 0, 0, 0, 0, time.UTC)
	sign(request, "AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "us-east-1", hashHex(nil), at)
	expected := "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, " +
		"SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
	if got := request.Header.Get("Authorization"); got != expected {
		test.Fatalf("authorization\n got %s\nwant %s", got, expected)
	}
}

func TestCanonicalPathAndQueryEncodeStrictly(test *testing.T) {
	if got := encodeComponent("/dusk-lake/connections/dt=2026-10-01/a b+c.parquet", true); got != "/dusk-lake/connections/dt%3D2026-10-01/a%20b%2Bc.parquet" {
		test.Fatalf("path %s", got)
	}
	query := map[string][]string{"prefix": {"files/ab/"}, "list-type": {"2"}, "delete": {""}}
	if got := canonicalQuery(query); got != "delete=&list-type=2&prefix=files%2Fab%2F" {
		test.Fatalf("query %s", got)
	}
}

func newClient(test *testing.T, handler http.HandlerFunc) *Client {
	server := httptest.NewServer(handler)
	test.Cleanup(server.Close)
	client, failure := New(Settings{Endpoint: server.URL, Region: "us-east-1", AccessKey: "access", SecretKey: "secret", BaseBackoff: time.Millisecond})
	if failure != nil {
		test.Fatal(failure)
	}
	return client
}

func TestListFollowsContinuationTokens(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		if !strings.HasPrefix(request.Header.Get("Authorization"), "AWS4-HMAC-SHA256 Credential=access/") {
			http.Error(writer, "unsigned", http.StatusForbidden)
			return
		}
		if request.URL.Path != "/dusk-files" || request.URL.Query().Get("prefix") != "files/ab/" || request.URL.Query().Get("list-type") != "2" {
			http.Error(writer, "unexpected "+request.URL.String(), http.StatusBadRequest)
			return
		}
		if request.URL.Query().Get("continuation-token") == "" {
			fmt.Fprint(writer, `<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>next</NextContinuationToken>
				<Contents><Key>files/ab/1/0-a</Key><Size>3</Size><LastModified>2026-10-01T10:00:00.000Z</LastModified></Contents></ListBucketResult>`)
			return
		}
		fmt.Fprint(writer, `<ListBucketResult><IsTruncated>false</IsTruncated>
			<Contents><Key>files/ab/2/0-b</Key><Size>5</Size><LastModified>2026-10-02T10:00:00.000Z</LastModified></Contents></ListBucketResult>`)
	})
	var keys []string
	if failure := client.List(context.Background(), "dusk-files", "files/ab/", func(object Object) error {
		keys = append(keys, fmt.Sprintf("%s:%d:%s", object.Key, object.Size, object.LastModified.Format(time.DateOnly)))
		return nil
	}); failure != nil {
		test.Fatal(failure)
	}
	if strings.Join(keys, ",") != "files/ab/1/0-a:3:2026-10-01,files/ab/2/0-b:5:2026-10-02" {
		test.Fatalf("listed %v", keys)
	}
}

func TestDeleteObjectsSendsQuietBatchesWithTheirDigest(test *testing.T) {
	var batches atomic.Int32
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		digest := md5.Sum(body)
		if request.Method != http.MethodPost || request.URL.RawQuery != "delete=" || request.Header.Get("Content-Md5") != base64.StdEncoding.EncodeToString(digest[:]) {
			http.Error(writer, "malformed delete", http.StatusBadRequest)
			return
		}
		var document struct {
			Quiet   bool `xml:"Quiet"`
			Objects []struct {
				Key string `xml:"Key"`
			} `xml:"Object"`
		}
		if xml.Unmarshal(body, &document) != nil || !document.Quiet || len(document.Objects) > 1000 {
			http.Error(writer, "bad document", http.StatusBadRequest)
			return
		}
		batches.Add(1)
		fmt.Fprint(writer, `<DeleteResult><Error><Key>gone</Key><Code>NoSuchKey</Code><Message>missing</Message></Error></DeleteResult>`)
	})
	keys := make([]string, 2500)
	for index := range keys {
		keys[index] = fmt.Sprintf("files/ab/%d", index)
	}
	if failure := client.DeleteObjects(context.Background(), "dusk-files", keys); failure != nil {
		test.Fatal(failure)
	}
	if batches.Load() != 3 {
		test.Fatalf("sent %d batches, want 3", batches.Load())
	}
}

func TestDeleteObjectsReportsARefusedKey(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		fmt.Fprint(writer, `<DeleteResult><Error><Key>files/ab/1</Key><Code>AccessDenied</Code><Message>no</Message></Error></DeleteResult>`)
	})
	failure := client.DeleteObjects(context.Background(), "dusk-files", []string{"files/ab/1"})
	if failure == nil || !strings.Contains(failure.Error(), "AccessDenied") {
		test.Fatalf("failure %v", failure)
	}
}

func TestRetriesServerErrorsAndStopsOnClientErrors(test *testing.T) {
	var calls atomic.Int32
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		if calls.Add(1) < 3 {
			writer.WriteHeader(http.StatusServiceUnavailable)
			fmt.Fprint(writer, `<Error><Code>SlowDown</Code><Message>later</Message></Error>`)
			return
		}
		writer.WriteHeader(http.StatusNoContent)
	})
	if failure := client.DeleteObject(context.Background(), "dusk-lake", "connections/dt=2026-10-01/x.parquet"); failure != nil {
		test.Fatal(failure)
	}
	if calls.Load() != 3 {
		test.Fatalf("%d calls, want 3", calls.Load())
	}
	denied := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		writer.WriteHeader(http.StatusForbidden)
		fmt.Fprint(writer, `<Error><Code>AccessDenied</Code><Message>no</Message></Error>`)
	})
	var answered *Error
	failure := denied.DeleteObject(context.Background(), "dusk-lake", "x")
	if !errors.As(failure, &answered) || answered.Code != "AccessDenied" || answered.Status != http.StatusForbidden {
		test.Fatalf("failure %v", failure)
	}
}

func TestReadsObjectLockRetentionAndExpirationRules(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		switch {
		case request.URL.Query().Has("object-lock"):
			fmt.Fprint(writer, `<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>COMPLIANCE</Mode><Days>400</Days></DefaultRetention></Rule></ObjectLockConfiguration>`)
		case request.URL.Path == "/empty":
			writer.WriteHeader(http.StatusNotFound)
			fmt.Fprint(writer, `<Error><Code>NoSuchLifecycleConfiguration</Code></Error>`)
		default:
			fmt.Fprint(writer, `<LifecycleConfiguration>
				<Rule><ID>a</ID><Status>Enabled</Status><Filter><Prefix></Prefix></Filter><Expiration><Days>90</Days></Expiration></Rule>
				<Rule><ID>b</ID><Status>Enabled</Status><Filter><Prefix>ledger/</Prefix></Filter><Expiration><Days>400</Days></Expiration></Rule>
				<Rule><ID>c</ID><Status>Disabled</Status><Filter><Prefix>files/</Prefix></Filter><Expiration><Days>1</Days></Expiration></Rule>
				<Rule><ID>d</ID><Status>Enabled</Status><Filter><Prefix></Prefix></Filter><AbortIncompleteMultipartUpload><DaysAfterInitiation>1</DaysAfterInitiation></AbortIncompleteMultipartUpload></Rule>
			</LifecycleConfiguration>`)
		}
	})
	retention, failure := client.ObjectLockRetention(context.Background(), "dusk-ledger-evidence")
	if failure != nil || retention.Mode != "COMPLIANCE" || retention.Days != 400 {
		test.Fatalf("retention %+v %v", retention, failure)
	}
	rules, failure := client.ExpirationRules(context.Background(), "dusk-lake")
	if failure != nil {
		test.Fatal(failure)
	}
	if days, found := ExpirationFor(rules, "ledger/dt=2026-10-01/x.parquet"); !found || days != 400 {
		test.Fatalf("ledger expires after %d %v", days, found)
	}
	if days, found := ExpirationFor(rules, "files/dt=2026-10-01/x.parquet"); !found || days != 90 {
		test.Fatalf("files expire after %d %v", days, found)
	}
	if _, failure := client.ExpirationRules(context.Background(), "empty"); failure != ErrNoLifecycle {
		test.Fatalf("failure %v", failure)
	}
}
