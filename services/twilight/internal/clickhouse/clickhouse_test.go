package clickhouse

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func newClient(test *testing.T, handler http.HandlerFunc) *Client {
	server := httptest.NewServer(handler)
	test.Cleanup(server.Close)
	client, failure := New(Settings{URL: server.URL, User: "twilight", Password: "secret"})
	if failure != nil {
		test.Fatal(failure)
	}
	return client
}

func TestExecSendsTheQueryParametersAndSettings(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		query := request.URL.Query()
		if request.Header.Get("X-ClickHouse-User") != "twilight" || request.Header.Get("X-ClickHouse-Key") != "secret" {
			http.Error(writer, "Code: 516. DB::Exception: Authentication failed", http.StatusUnauthorized)
			return
		}
		if string(body) != "ALTER TABLE dusk.files DELETE WHERE device_id = {device:String}" || query.Get("param_device") != "ab" || query.Get("mutations_sync") != "0" {
			http.Error(writer, "unexpected", http.StatusBadRequest)
		}
	})
	if failure := client.Exec(context.Background(), Query{Text: "ALTER TABLE dusk.files DELETE WHERE device_id = {device:String}",
		Parameters: map[string]string{"device": "ab"}, Settings: map[string]string{"mutations_sync": "0"}}); failure != nil {
		test.Fatal(failure)
	}
}

func TestInsertSendsTheQueryInTheURLAndTheRowsInTheBody(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		if request.URL.Query().Get("query") != "INSERT INTO dusk.erasure_scope FORMAT TSV" || string(body) != "a\tdevice\tb\n" {
			http.Error(writer, "unexpected", http.StatusBadRequest)
		}
	})
	if failure := client.Exec(context.Background(), Query{Text: "INSERT INTO dusk.erasure_scope FORMAT TSV", Body: strings.NewReader("a\tdevice\tb\n")}); failure != nil {
		test.Fatal(failure)
	}
}

func TestErrorsCarryTheServersMessage(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		http.Error(writer, "Code: 60. DB::Exception: Unknown table expression identifier 'dusk.nothing'", http.StatusNotFound)
	})
	var answered *Error
	failure := client.Exec(context.Background(), Query{Text: "SELECT 1 FROM dusk.nothing"})
	if !errors.As(failure, &answered) || answered.Status != http.StatusNotFound || !strings.Contains(answered.Message, "Unknown table") {
		test.Fatalf("failure %v", failure)
	}
}

func TestScanDecodesRowsAndStopsAtAnException(test *testing.T) {
	client := newClient(test, func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		if !strings.HasSuffix(string(body), " FORMAT JSONEachRow") || request.URL.Query().Get("http_write_exception_in_output_format") != "1" {
			http.Error(writer, "unexpected", http.StatusBadRequest)
			return
		}
		if strings.Contains(string(body), "broken") {
			fmt.Fprintln(writer, `{"path":"a","matched":1}`)
			fmt.Fprintln(writer, `{"exception": "Code: 241. DB::Exception: Memory limit exceeded"}`)
			return
		}
		fmt.Fprintln(writer, `{"path":"a","matched":1}`)
		fmt.Fprintln(writer, `{"path":"b","matched":2}`)
	})
	type row struct {
		Path    string `json:"path"`
		Matched int64  `json:"matched"`
	}
	rows, failure := Scan[row](context.Background(), client, Query{Text: "SELECT _path AS path, count() AS matched FROM dusk.lake_files_parquet GROUP BY path"})
	if failure != nil || len(rows) != 2 || rows[1].Path != "b" || rows[1].Matched != 2 {
		test.Fatalf("rows %+v %v", rows, failure)
	}
	if _, failure := Scan[row](context.Background(), client, Query{Text: "SELECT broken"}); failure == nil || !strings.Contains(failure.Error(), "Memory limit") {
		test.Fatalf("failure %v", failure)
	}
}

func TestQuoteEscapesForAStringLiteral(test *testing.T) {
	if got := Quote(`it's a \ path`); got != `'it\'s a \\ path'` {
		test.Fatalf("quoted %s", got)
	}
}
