package sdk_test

import (
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateResourceIO = flag.Bool("migration-update-resource-io", false, "capture resource request and response contracts from the fixed Go reference")

type migrationResourceReader struct {
	reads  int
	closes int
}

func (r *migrationResourceReader) Read(p []byte) (int, error) { r.reads++; return 0, io.EOF }
func (r *migrationResourceReader) Close() error               { r.closes++; return nil }

func TestMigrationResourceResponseKeepsUnreadStreamOwnership(t *testing.T) {
	reader := &migrationResourceReader{}
	response := sdk.NewResourceResponse(200, http.Header{}, reader)
	_ = response.Header()
	_ = response.ContentLength()
	if reader.reads != 0 || reader.closes != 0 {
		t.Fatal("response construction or metadata access consumed the stream")
	}
	if err := response.Body.Close(); err != nil {
		t.Fatal(err)
	}
	if reader.reads != 0 || reader.closes != 1 {
		t.Fatal("closing an unread response did not release the owned stream")
	}
}

func TestMigrationResourceIOMatchesFrozenHeadersAndValidation(t *testing.T) {
	type requestCase struct {
		Method          string     `json:"method"`
		Range           string     `json:"range"`
		IfNoneMatch     string     `json:"if_none_match"`
		IfModifiedSince string     `json:"if_modified_since"`
		IfRange         string     `json:"if_range"`
		Reason          sdk.Reason `json:"reason"`
		Message         string     `json:"message"`
	}
	type responseCase struct {
		Status        int         `json:"status"`
		Input         http.Header `json:"input"`
		Headers       http.Header `json:"headers"`
		ContentType   string      `json:"content_type"`
		ContentLength int64       `json:"content_length"`
		ContentRange  string      `json:"content_range"`
		AcceptRanges  string      `json:"accept_ranges"`
		ETag          string      `json:"etag"`
		LastModified  string      `json:"last_modified"`
		CacheControl  string      `json:"cache_control"`
		Body          []byte      `json:"body"`
	}
	contract := struct {
		Requests  []requestCase  `json:"requests"`
		Responses []responseCase `json:"responses"`
	}{}
	record := func(input requestCase) {
		err := (sdk.OpenResourceRequest{Method: sdk.ResourceMethod(input.Method), Range: input.Range, IfNoneMatch: input.IfNoneMatch, IfModifiedSince: input.IfModifiedSince, IfRange: input.IfRange}).Validate()
		input.Reason = sdk.ReasonOf(err)
		if err != nil {
			input.Message = err.Error()
		}
		contract.Requests = append(contract.Requests, input)
	}
	for _, method := range []string{"", "GET", "HEAD", "get", "head", "POST", "OPTIONS", " GET"} {
		record(requestCase{Method: method})
	}
	for _, control := range append(func() []byte {
		values := make([]byte, 32)
		for i := range values {
			values[i] = byte(i)
		}
		return values
	}(), 127) {
		for field := 0; field < 4; field++ {
			input := requestCase{Method: "GET"}
			value := "fixture" + string(control) + "value"
			switch field {
			case 0:
				input.Range = value
			case 1:
				input.IfNoneMatch = value
			case 2:
				input.IfModifiedSince = value
			case 3:
				input.IfRange = value
			}
			record(input)
		}
	}
	record(requestCase{Range: "bytes=0-99", IfNoneMatch: `"fixture-etag"`, IfModifiedSince: "Wed, 01 Jan 2025 00:00:00 GMT", IfRange: `W/"fixture"`})
	record(requestCase{IfNoneMatch: "日本語\u0085\u200b"})
	record(requestCase{Method: "POST", Range: "\n", IfNoneMatch: "\r"})
	record(requestCase{Range: "\n", IfNoneMatch: "\r", IfModifiedSince: "\t", IfRange: "\x00"})
	for _, length := range []string{"", "0", "1024", "-1", "+123", "00123", " 123 ", "1.5", "1e2", "9223372036854775807", "-9223372036854775808", "9223372036854775808", "123, 456"} {
		input := http.Header{"Content-Type": {"image/png", "image/jpeg"}, "Content-Length": {length, "999"}, "Content-Range": {"bytes 0-2/10"}, "Accept-Ranges": {"bytes"}, "Etag": {`"fixture"`}, "Last-Modified": {"Wed, 01 Jan 2025 00:00:00 GMT"}, "Cache-Control": {"public, max-age=60"}, "Location": {"https://fixture.invalid/secret"}, "Set-Cookie": {"fixture-cookie-secret"}, "Authorization": {"fixture-auth-secret"}, "X-Internal": {"fixture-internal-secret"}}
		response := sdk.NewResourceResponse(206, input, io.NopCloser(bytes.NewReader([]byte{0, 1, 128, 255})))
		body, err := io.ReadAll(response.Body)
		if err != nil {
			t.Fatal(err)
		}
		if err := response.Body.Close(); err != nil {
			t.Fatal(err)
		}
		contract.Responses = append(contract.Responses, responseCase{response.StatusCode, input, response.Header(), response.ContentType(), response.ContentLength(), response.ContentRange(), response.AcceptRanges(), response.ETag(), response.LastModified(), response.CacheControl(), body})
	}
	for _, status := range []int{0, 200, 204, 304, -1} {
		input := http.Header{"content-type": {"ignored-lowercase"}, "ETag": {"ignored-noncanonical"}, "Cache-Control": nil, "Content-Type": {}}
		response := sdk.NewResourceResponse(status, input, io.NopCloser(bytes.NewReader([]byte("fixture-body"))))
		body, err := io.ReadAll(response.Body)
		if err != nil {
			t.Fatal(err)
		}
		if err := response.Body.Close(); err != nil {
			t.Fatal(err)
		}
		contract.Responses = append(contract.Responses, responseCase{response.StatusCode, input, response.Header(), response.ContentType(), response.ContentLength(), response.ContentRange(), response.AcceptRanges(), response.ETag(), response.LastModified(), response.CacheControl(), body})
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "docs", "migration", "contracts", "resource-io.json")
	if *migrationUpdateResourceIO {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("resource I/O contracts differ from the fixed Go reference")
	}
}
