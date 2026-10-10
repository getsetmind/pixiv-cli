package saucenao_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"

	reversesearch "github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch/saucenao"
)

var migrationCaptureSauceNAO = flag.Bool("migration-capture-reverse-saucenao", false, "capture frozen Go SauceNAO provider using owned in-memory HTTP and source snapshots")

const migrationSauceKey = "synthetic-saucenao-key-marker"
const migrationSauceSecret = "synthetic-upstream-error-marker"
const migrationSauceEmpty = `{"header":{"status":0,"short_remaining":3,"long_remaining":97,"short_limit":"4","long_limit":"100"},"results":[]}`
const migrationSauceMatch = `{"header":{"status":0,"short_remaining":3,"long_remaining":97,"short_limit":"4","long_limit":"100"},"results":[{"header":{"similarity":"91.23","index_id":5,"index_name":"Index #5: Pixiv Images"},"data":{"ext_urls":["https://www.pixiv.net/artworks/123"],"title":"Synthetic title","pixiv_id":"123","member_name":"Synthetic member","author_name":"Synthetic fallback","member_id":"456"}}]}`

type migrationSauceInput struct {
	Operation          string `json:"operation"`
	Client             string `json:"client"`
	Context            string `json:"context"`
	Key                string `json:"key"`
	Endpoint           string `json:"endpoint"`
	Snapshot           string `json:"snapshot"`
	Payload            []byte `json:"payload"`
	Status             int    `json:"status"`
	Response           string `json:"response"`
	ResponseBytes      []byte `json:"response_bytes,omitempty"`
	TransportError     string `json:"transport_error"`
	ResponseReadError  string `json:"response_read_error"`
	ErrorWithLastBytes bool   `json:"error_with_last_bytes"`
	ResponseCloseError bool   `json:"response_close_error"`
	CancelAt           string `json:"cancel_at"`
	ResponseChunk      int    `json:"response_chunk"`
	NilBody            bool   `json:"nil_body"`
	Repeat             int    `json:"repeat"`
	CloseBefore        bool   `json:"close_before"`
}

type migrationSauceRow struct {
	Name        string              `json:"name"`
	Input       migrationSauceInput `json:"input"`
	Observation map[string]any      `json:"observation"`
}

type migrationSauceContextKey struct{}

func migrationSauceError(err error) any {
	if err == nil {
		return nil
	}
	chain := []map[string]any{}
	for nested := err; nested != nil; nested = errors.Unwrap(nested) {
		chain = append(chain, map[string]any{"type": fmt.Sprintf("%T", nested), "message": nested.Error()})
	}
	return map[string]any{"code": reversesearch.CodeOf(err), "message": err.Error(), "canceled": errors.Is(err, context.Canceled), "deadline": errors.Is(err, context.DeadlineExceeded), "chain": chain}
}

func migrationSauceCause(kind string) error {
	switch kind {
	case "":
		return nil
	case "EOF":
		return io.EOF
	case "unexpected_EOF":
		return io.ErrUnexpectedEOF
	case "canceled":
		return fmt.Errorf("%s: %w", migrationSauceSecret, context.Canceled)
	case "deadline":
		return fmt.Errorf("%s: %w", migrationSauceSecret, context.DeadlineExceeded)
	default:
		return errors.New(migrationSauceSecret)
	}
}

type migrationSauceBody struct {
	input  migrationSauceInput
	data   []byte
	offset int
	reads  int
	closes int
	cancel context.CancelFunc
}

func (b *migrationSauceBody) Read(p []byte) (int, error) {
	b.reads++
	n := len(b.data) - b.offset
	if n > len(p) {
		n = len(p)
	}
	if b.input.ResponseChunk > 0 && n > b.input.ResponseChunk {
		n = b.input.ResponseChunk
	}
	copy(p, b.data[b.offset:b.offset+n])
	b.offset += n
	if b.input.CancelAt == "response_read" {
		b.cancel()
	}
	if b.offset == len(b.data) && (n == 0 || b.input.ErrorWithLastBytes) {
		if err := migrationSauceCause(b.input.ResponseReadError); err != nil {
			return n, err
		}
		if n == 0 {
			return 0, io.EOF
		}
	}
	return n, nil
}

func (b *migrationSauceBody) Close() error {
	b.closes++
	if b.input.CancelAt == "response_close" {
		b.cancel()
	}
	if b.input.ResponseCloseError {
		return errors.New(migrationSauceSecret)
	}
	return nil
}

type migrationSauceTransport struct {
	t        *testing.T
	input    migrationSauceInput
	requests []map[string]any
	bodies   []*migrationSauceBody
	idle     int
	cancel   context.CancelFunc
}

func (r *migrationSauceTransport) CloseIdleConnections() { r.idle++ }

func migrationSauceMultipart(t *testing.T, request *http.Request, data []byte, readErr error, input migrationSauceInput) map[string]any {
	t.Helper()
	raw := request.Header.Get("Content-Type")
	mediaType, params, err := mime.ParseMediaType(raw)
	if err != nil {
		t.Fatal(err)
	}
	boundary := params["boundary"]
	decoded, err := hex.DecodeString(boundary)
	if err != nil || len(decoded) != 30 || len(params) != 1 || mediaType != "multipart/form-data" || raw != "multipart/form-data; boundary="+boundary {
		t.Fatalf("multipart boundary/header contract changed: %q %#v", raw, params)
	}
	parts := []map[string]any{}
	reader := multipart.NewReader(bytes.NewReader(data), boundary)
	parseError := ""
	for {
		part, err := reader.NextPart()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			parseError = err.Error()
			break
		}
		payload, err := io.ReadAll(part)
		if err != nil {
			t.Fatal(err)
		}
		parts = append(parts, map[string]any{"headers": part.Header, "name": part.FormName(), "filename": part.FileName(), "payload": payload})
		if err := part.Close(); err != nil {
			t.Fatal(err)
		}
	}
	{
		var exact bytes.Buffer
		expectedPayload := input.Payload
		if input.Snapshot == "closed" {
			expectedPayload = nil
		}
		expectedParts := []struct {
			header  string
			payload []byte
		}{
			{`Content-Disposition: form-data; name="api_key"`, []byte(input.Key)},
			{`Content-Disposition: form-data; name="output_type"`, []byte("2")},
			{`Content-Disposition: form-data; name="db"`, []byte("999")},
			{"Content-Disposition: form-data; name=\"file\"; filename=\"image\"\r\nContent-Type: application/octet-stream", expectedPayload},
		}
		for index, part := range expectedParts {
			if index > 0 {
				exact.WriteString("\r\n")
			}
			exact.WriteString("--" + boundary + "\r\n" + part.header + "\r\n\r\n")
			exact.Write(part.payload)
		}
		exact.WriteString("\r\n--" + boundary + "--\r\n")
		if !bytes.Equal(exact.Bytes(), data) {
			t.Fatal("complete multipart wire bytes differ from fixed Go fields, headers, payload, and framing")
		}
	}
	return map[string]any{"media_type": mediaType, "parameter_names": []string{"boundary"}, "boundary_length": len(boundary), "boundary_lower_hex": boundary == strings.ToLower(boundary), "boundary_decoded_length": len(decoded), "complete_framing": bytes.HasSuffix(data, []byte("\r\n--"+boundary+"--\r\n")), "wire_length": len(data), "parts": parts, "read_error": migrationSauceError(readErr), "parse_error": parseError}
}

func (r *migrationSauceTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	data, readErr := io.ReadAll(request.Body)
	if err := request.Body.Close(); err != nil {
		r.t.Fatal(err)
	}
	multipartObservation := migrationSauceMultipart(r.t, request, data, readErr, r.input)
	headers := request.Header.Clone()
	delete(headers, "Content-Type")
	r.requests = append(r.requests, map[string]any{
		"method": request.Method, "url": request.URL.String(), "request_uri": request.URL.RequestURI(), "host": request.Host,
		"headers_without_generated_content_type": headers, "content_type": multipartObservation,
		"content_length": request.ContentLength, "get_body_available": request.GetBody != nil, "transfer_encoding": request.TransferEncoding,
		"context_value": request.Context().Value(migrationSauceContextKey{}), "context_error": migrationSauceError(request.Context().Err()),
	})
	if r.input.CancelAt == "transport" {
		r.cancel()
	}
	if err := migrationSauceCause(r.input.TransportError); err != nil {
		return nil, err
	}
	response := &http.Response{StatusCode: r.input.Status, Header: http.Header{"Content-Type": []string{"text/plain"}, "X-Synthetic": []string{"retained"}}, Request: request}
	if !r.input.NilBody {
		payload := []byte(r.input.Response)
		if r.input.ResponseBytes != nil {
			payload = r.input.ResponseBytes
		}
		body := &migrationSauceBody{input: r.input, data: payload, cancel: r.cancel}
		r.bodies = append(r.bodies, body)
		response.Body = body
	}
	return response, nil
}

func migrationSauceObserve(t *testing.T, input migrationSauceInput) map[string]any {
	t.Helper()
	ctx := context.WithValue(context.Background(), migrationSauceContextKey{}, "synthetic-caller-context")
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	switch input.Context {
	case "nil":
		ctx = nil
	case "canceled":
		cancel()
	case "deadline":
		var cancelDeadline context.CancelFunc
		ctx, cancelDeadline = context.WithDeadline(ctx, time.Unix(1, 0))
		defer cancelDeadline()
	case "background":
	default:
		t.Fatalf("unknown context %q", input.Context)
	}
	transport := &migrationSauceTransport{t: t, input: input, requests: []map[string]any{}, cancel: cancel}
	httpClient := &http.Client{Transport: transport}
	options := saucenao.Options{APIKey: input.Key, Endpoint: input.Endpoint, HTTPClient: httpClient}
	if input.Client == "default" {
		previous := http.DefaultClient
		http.DefaultClient = httpClient
		defer func() { http.DefaultClient = previous }()
		options.HTTPClient = nil
	}
	client := saucenao.New(options)
	if input.Client == "nil" {
		client = nil
	}
	var snapshot *reversesearch.Snapshot
	var snapshotSummary any
	if input.Snapshot != "nil" {
		snapshot = loadSnapshot(t, input.Payload)
		snapshotSummary = map[string]any{"kind": snapshot.Kind(), "sha256": snapshot.SHA256(), "size": snapshot.Size()}
		if input.Snapshot == "closed" {
			if err := snapshot.Close(); err != nil {
				t.Fatal(err)
			}
		}
	}
	if input.CloseBefore {
		if err := client.Close(); err != nil {
			t.Fatal(err)
		}
	}
	results := []map[string]any{}
	for index := 0; index < input.Repeat; index++ {
		switch input.Operation {
		case "preflight":
			results = append(results, map[string]any{"error": migrationSauceError(client.Preflight(ctx))})
		case "search":
			response, err := client.Search(ctx, snapshot)
			if err != nil {
				chain := errorChainText(err)
				for _, marker := range []string{migrationSauceKey, migrationSauceSecret, "synthetic-private-image-marker"} {
					if strings.Contains(chain, marker) {
						t.Fatalf("provider error chain leaked synthetic input marker: %s", marker)
					}
				}
			}
			results = append(results, map[string]any{"response": response, "matches_nil": response.Matches == nil, "quota_nil": response.Quota == nil, "error": migrationSauceError(err)})
		case "close":
			results = append(results, map[string]any{"error": migrationSauceError(client.Close())})
		default:
			t.Fatal("unknown operation")
		}
	}
	idleBefore := transport.idle
	closeFirst := client.Close()
	closeSecond := client.Close()
	bodies := []map[string]any{}
	for _, body := range transport.bodies {
		bodies = append(bodies, map[string]any{"bytes_read": body.offset, "read_calls": body.reads, "close_calls": body.closes})
		if body.closes != 1 {
			t.Fatalf("response body close count = %d", body.closes)
		}
	}
	if snapshot != nil && input.Snapshot != "closed" {
		opened, err := snapshot.Open()
		if err != nil {
			t.Fatal("provider took ownership of caller snapshot")
		}
		data, err := io.ReadAll(opened)
		if err != nil {
			t.Fatal(err)
		}
		if err := opened.Close(); err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(data, input.Payload) {
			t.Fatal("provider mutated caller snapshot")
		}
	}
	var contextErr error
	if ctx != nil {
		contextErr = ctx.Err()
	}
	return map[string]any{"snapshot": snapshotSummary, "requests": transport.requests, "results": results, "response_bodies": bodies, "idle_close_calls_before_final_close": idleBefore, "idle_close_calls_after_final_close": transport.idle, "first_close_error": migrationSauceError(closeFirst), "second_close_error": migrationSauceError(closeSecond), "context_after": migrationSauceError(contextErr), "caller_http_client_unchanged": httpClient.Transport == transport && httpClient.Timeout == 0 && httpClient.Jar == nil && httpClient.CheckRedirect == nil}
}

func migrationSauceRows() []migrationSauceRow {
	rows := []migrationSauceRow{}
	add := func(name string) *migrationSauceInput {
		rows = append(rows, migrationSauceRow{Name: name, Input: migrationSauceInput{Operation: "search", Client: "supplied", Context: "background", Key: migrationSauceKey, Endpoint: "https://saucenao.invalid/search.php", Snapshot: "open", Payload: []byte("synthetic-private-image-marker\x00\xff\r\n"), Status: 200, Response: migrationSauceMatch, Repeat: 1}})
		return &rows[len(rows)-1].Input
	}
	for _, operation := range []string{"preflight", "search"} {
		for _, client := range []string{"supplied", "nil"} {
			for _, state := range []string{"nil", "canceled", "deadline", "background"} {
				input := add("validation/" + operation + "_" + client + "_" + state)
				input.Operation, input.Client, input.Context, input.Snapshot = operation, client, state, "nil"
			}
		}
		for index, key := range []string{"", " \t\r\n", "\u0085\u00a0\u2003\u3000", "\u200b", " " + migrationSauceKey + "\t"} {
			input := add(fmt.Sprintf("validation/%s_key_%d", operation, index))
			input.Operation, input.Key, input.Snapshot = operation, key, "nil"
		}
	}
	for _, client := range []string{"supplied", "nil", "default"} {
		input := add("close/" + client + "_idempotent")
		input.Operation, input.Client, input.Snapshot, input.Repeat = "close", client, "nil", 3
	}
	add("multipart/default_endpoint").Endpoint = ""
	add("multipart/default_http_client").Client = "default"
	add("multipart/key_whitespace_transmitted_untrimmed").Key = " \t" + migrationSauceKey + "\r\n"
	add("multipart/caller_endpoint_query_fragment").Endpoint = "https://saucenao.invalid/other?existing=yes#fragment"
	add("multipart/empty_payload").Payload = []byte{}
	add("multipart/raw_nul_invalid_utf8_payload").Payload = []byte{0, 255, 192, 175, 13, 10, 127, 0}
	add("multipart/repeat_same_snapshot").Repeat = 2
	add("close/search_after_close_uses_same_client").CloseBefore = true
	add("validation/closed_snapshot_completed_pipe").Snapshot = "closed"
	closedStatus := add("status/http_failure_precedes_closed_snapshot_upload_error")
	closedStatus.Status, closedStatus.Snapshot = 429, "closed"
	add("validation/invalid_endpoint_before_transport").Endpoint = "https://saucenao.invalid/%zz"
	add("validation/control_endpoint_before_transport").Endpoint = "https://saucenao.invalid/\n"
	add("transport/non_http_scheme_delegated_to_mock").Endpoint = "ftp://saucenao.invalid/search.php"
	for _, fault := range []string{"raw", "canceled", "deadline"} {
		add("transport/" + fault + "_safe_chain").TransportError = fault
	}
	for _, status := range []int{199, 200, 201, 204, 206, 299, 300, 301, 400, 401, 403, 404, 429, 500, 503} {
		add(fmt.Sprintf("status/http_%d", status)).Status = status
	}
	for _, status := range []int{200, 429} {
		input := add(fmt.Sprintf("context/cancel_in_transport_status_%d", status))
		input.Status, input.CancelAt = status, "transport"
	}
	input := add("context/cancel_in_transport_with_error")
	input.CancelAt, input.TransportError = "transport", "raw"
	add("context/cancel_during_decode").CancelAt = "response_read"
	add("context/cancel_on_body_close").CancelAt = "response_close"
	add("response/close_error_ignored").ResponseCloseError = true
	add("response/nil_body_defaulted_by_http_client").NilBody = true
	add("response/one_byte_chunks").ResponseChunk = 1
	for _, fault := range []string{"raw", "unexpected_EOF", "canceled", "deadline", "EOF"} {
		for _, last := range []bool{false, true} {
			input := add(fmt.Sprintf("response/read_%s_with_last_%t", fault, last))
			input.ResponseReadError, input.ErrorWithLastBytes = fault, last
		}
	}
	for _, body := range []struct{ name, body string }{
		{"empty", ""}, {"whitespace", " \r\n\t"}, {"invalid_json", migrationSauceSecret}, {"null", "null"}, {"array", "[]"}, {"scalar", "1"}, {"empty_object", "{}"},
		{"null_header", `{"header":null,"results":[]}`}, {"missing_status", `{"header":{},"results":[]}`}, {"null_status", `{"header":{"status":null},"results":[]}`},
		{"reject_minimal", `{"header":{"status":-2,"message":"` + migrationSauceSecret + `"}}`}, {"reject_positive", `{"header":{"status":"1"}}`},
		{"reject_invalid_results_type", `{"header":{"status":-2},"results":"ignored"}`}, {"reject_invalid_quota_type", `{"header":{"status":-2,"short_limit":"bad"}}`},
		{"trailing_second_object", migrationSauceEmpty + ` {"secret":"` + migrationSauceSecret + `"}`}, {"trailing_null", migrationSauceEmpty + ` null`}, {"trailing_garbage", migrationSauceEmpty + ` garbage`},
		{"trailing_whitespace", migrationSauceEmpty + " \r\n\t"}, {"empty_results", migrationSauceEmpty}, {"unknown_fields", strings.Replace(migrationSauceEmpty, `"results":[]`, `"results":[],"unrecognized":{"private":"`+migrationSauceSecret+`"}`, 1)},
	} {
		add("decode/" + body.name).Response = body.body
	}
	for _, field := range []string{"status", "short_remaining", "long_remaining", "short_limit", "long_limit"} {
		for _, token := range []string{"missing", "null", `"0"`, `"+1"`, `"01"`, `"-0"`, `" 1 "`, `"1e0"`, "1.0", "1e0", "true", "{}", "[]", `"9223372036854775807"`, `"-9223372036854775808"`, `"9223372036854775808"`, `"-9223372036854775809"`} {
			var wire map[string]json.RawMessage
			_ = json.Unmarshal([]byte(migrationSauceEmpty), &wire)
			var header map[string]json.RawMessage
			_ = json.Unmarshal(wire["header"], &header)
			if token == "missing" {
				delete(header, field)
			} else {
				header[field] = json.RawMessage(token)
			}
			h, _ := json.Marshal(header)
			wire["header"] = h
			encoded, _ := json.Marshal(wire)
			add(fmt.Sprintf("quota/%s_%x", field, token)).Response = string(encoded)
		}
	}
	mutate := func(name, location, field, token string) {
		var wire map[string]json.RawMessage
		_ = json.Unmarshal([]byte(migrationSauceMatch), &wire)
		var results []map[string]json.RawMessage
		_ = json.Unmarshal(wire["results"], &results)
		var object map[string]json.RawMessage
		_ = json.Unmarshal(results[0][location], &object)
		if token == "missing" {
			delete(object, field)
		} else {
			object[field] = json.RawMessage(token)
		}
		o, _ := json.Marshal(object)
		results[0][location] = o
		r, _ := json.Marshal(results)
		wire["results"] = r
		encoded, _ := json.Marshal(wire)
		add(name).Response = string(encoded)
	}
	for _, field := range []string{"similarity", "index_id", "index_name"} {
		for _, token := range []string{"missing", "null", `""`, "0", "true", "{}", "[]"} {
			mutate(fmt.Sprintf("result/%s_%x", field, token), "header", field, token)
		}
	}
	for _, token := range []string{`"NaN"`, `"Inf"`, `"-Inf"`, `"Infinity"`, `"1e309"`, `"1e-999"`, `"-0"`, `"0x1p2"`, `"1_2.5"`, `" 80 "`, `"+80.5"`, "-3.5", "101", "1.5e2"} {
		mutate(fmt.Sprintf("float/similarity_%x", token), "header", "similarity", token)
	}
	for _, field := range []string{"pixiv_id", "member_id"} {
		for _, token := range []string{"missing", "null", "0", "-123", `"+123"`, `"00123"`, "1.0", "1e2", `"bad"`, `"9223372036854775807"`} {
			mutate(fmt.Sprintf("ids/%s_%x", field, token), "data", field, token)
		}
	}
	for _, field := range []string{"title", "member_name", "author_name"} {
		for _, token := range []string{"missing", "null", `""`, `" "`, "1", "[]"} {
			mutate(fmt.Sprintf("text/%s_%x", field, token), "data", field, token)
		}
	}
	for _, token := range []string{"missing", "null", "[]", `["","https://example.invalid/a","https://example.invalid/a"]`, `[1]`, `"https://example.invalid/a"`} {
		mutate(fmt.Sprintf("urls/ext_urls_%x", token), "data", "ext_urls", token)
	}
	for _, body := range []struct{ name, body string }{
		{"missing_results", strings.Replace(migrationSauceEmpty, `,"results":[]`, "", 1)},
		{"null_results", strings.Replace(migrationSauceEmpty, `"results":[]`, `"results":null`, 1)},
		{"null_result", strings.Replace(migrationSauceEmpty, `"results":[]`, `"results":[null]`, 1)},
		{"empty_result", strings.Replace(migrationSauceEmpty, `"results":[]`, `"results":[{}]`, 1)},
		{"missing_result_header", strings.Replace(migrationSauceMatch, `"header":{"similarity":"91.23","index_id":5,"index_name":"Index #5: Pixiv Images"},`, "", 1)},
		{"null_result_header", strings.Replace(migrationSauceMatch, `"header":{"similarity":"91.23","index_id":5,"index_name":"Index #5: Pixiv Images"}`, `"header":null`, 1)},
		{"null_result_data", strings.Replace(migrationSauceMatch, `"data":{"ext_urls":["https://www.pixiv.net/artworks/123"],"title":"Synthetic title","pixiv_id":"123","member_name":"Synthetic member","author_name":"Synthetic fallback","member_id":"456"}`, `"data":null`, 1)},
		{"empty_result_data", strings.Replace(migrationSauceMatch, `"data":{"ext_urls":["https://www.pixiv.net/artworks/123"],"title":"Synthetic title","pixiv_id":"123","member_name":"Synthetic member","author_name":"Synthetic fallback","member_id":"456"}`, `"data":{}`, 1)},
		{"duplicate_status_last_success", strings.Replace(migrationSauceEmpty, `"status":0`, `"status":-1,"status":0`, 1)},
		{"duplicate_status_last_failure", strings.Replace(migrationSauceEmpty, `"status":0`, `"status":0,"status":-1`, 1)},
		{"case_insensitive_fields", strings.ReplaceAll(strings.ReplaceAll(migrationSauceMatch, `"header"`, `"HEADER"`), `"data"`, `"DATA"`)},
		{"duplicate_similarity_last", strings.Replace(migrationSauceMatch, `"similarity":"91.23"`, `"similarity":5,"similarity":90`, 1)},
		{"whitespace_index_name", strings.Replace(migrationSauceMatch, `Index #5: Pixiv Images`, ` `, 1)},
		{"same_document_two_results", strings.Replace(migrationSauceMatch, `}]}`, `},`+`{"header":{"similarity":80.5,"index_id":"9","index_name":"Synthetic other"},"data":{"title":"Other","author_name":"Other author","ext_urls":["https://example.invalid/a"]}}]}`, 1)},
	} {
		add("structure/" + body.name).Response = body.body
	}
	input = add("text/invalid_utf8_replaced_by_json_decoder")
	input.ResponseBytes = []byte(strings.Replace(migrationSauceMatch, "Synthetic title", string([]byte{255, 0xc0, 0xaf}), 1))
	return rows
}

func migrationSauceVerifySources(t *testing.T, root string) map[string]any {
	t.Helper()
	const frozen = "4b4426487ef18bed276706daec385e0d0a6979f9"
	const published = "99246d8bfb8712caa0903426953423850836cefe"
	listing, err := exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", frozen).Output()
	if err != nil {
		t.Fatal(err)
	}
	production := map[string]string{}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if !(strings.HasSuffix(path, ".go") && !strings.HasSuffix(path, "_test.go")) && path != "go.mod" && path != "go.sum" {
			continue
		}
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		original, err := exec.Command("git", "-C", root, "show", frozen+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(current, original) {
			t.Fatalf("frozen Go production/module path changed: %s", path)
		}
		production[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	if len(production) != 434 {
		t.Fatalf("production inventory = %d, want 434", len(production))
	}
	preserved := map[string]string{}
	listing, err = exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", published, "crates").Output()
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if !strings.Contains(path, "/tests/fixtures/") {
			continue
		}
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		original, err := exec.Command("git", "-C", root, "show", published+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(current, original) {
			t.Fatalf("published fixture changed: %s", path)
		}
		preserved[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	if len(preserved) != 104 {
		t.Fatalf("published fixture inventory = %d, want 104", len(preserved))
	}
	sources := map[string]string{}
	for _, path := range []string{"internal/services/reversesearch/saucenao/client.go", "internal/services/reversesearch/saucenao/client_test.go", "internal/services/reversesearch/source.go", "internal/services/reversesearch/contracts.go", "internal/services/reversesearch/errors.go"} {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		original, err := exec.Command("git", "-C", root, "show", frozen+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(data, original) {
			t.Fatalf("provider source or reused existing helper differs from frozen source: %s", path)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	stdlib := map[string]string{}
	for _, path := range []string{"net/http/client.go", "net/http/request.go", "mime/multipart/writer.go", "mime/multipart/multipart.go", "mime/mediatype.go", "encoding/json/decode.go", "encoding/json/stream.go", "strconv/number.go", "internal/strconv/atoi.go", "internal/strconv/atof.go", "strings/strings.go", "context/context.go", "io/pipe.go", "io/io.go", "sync/once.go"} {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		stdlib[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	encoded, _ := json.Marshal(production)
	return map[string]any{"frozen_go_production_guard": map[string]any{"byte_equal_to_frozen_commit": true, "path_count": len(production), "path_sha256_map": production, "path_sha256_map_json_sha256": fmt.Sprintf("%x", sha256.Sum256(encoded))}, "protected_published_fixtures_sha256": preserved, "source_sha256": sources, "go_stdlib_sha256": stdlib}
}

func TestMigrationReverseSauceNAOFrozenGo(t *testing.T) {
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("Go toolchain must be pinned go1.27.1, got %s", runtime.Version())
	}
	root := filepath.Join("..", "..", "..", "..")
	fixture := migrationSauceVerifySources(t, root)
	rows := migrationSauceRows()
	if len(rows) != 277 {
		t.Fatalf("bounded actual-provider inventory = %d, want 277", len(rows))
	}
	families := map[string]int{}
	seen := map[string]bool{}
	for index := range rows {
		row := &rows[index]
		if seen[row.Name] {
			t.Fatalf("duplicate case %s", row.Name)
		}
		seen[row.Name] = true
		families[strings.Split(row.Name, "/")[0]]++
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationSauceObserve(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	fixture["source_commit"] = "4b4426487ef18bed276706daec385e0d0a6979f9"
	fixture["published_base"] = "99246d8bfb8712caa0903426953423850836cefe"
	fixture["go_version"] = runtime.Version()
	fixture["case_family_counts"] = families
	fixture["cases"] = rows
	fixture["public_operations"] = []string{"saucenao.New", "saucenao.Client.Preflight", "saucenao.Client.Search", "saucenao.Client.Close", "reversesearch.NewSourceLoader", "reversesearch.Loader.Load", "reversesearch.Snapshot.Open", "reversesearch.Snapshot.Close"}
	fixture["evidence"] = "Actual unchanged frozen SauceNAO provider through normal net/http.Client and owned in-memory RoundTripper. Every request pipe is fully consumed and closed, and every snapshot is genuinely created from an owned synthetic local file through SourceLoader. Random multipart boundary values are excluded from expectations; exact Content-Type shape, parameter names, length, lowercase hex properties, ordered complete part headers and payload bytes, and complete raw body framing reconstructed with that actual boundary are checked. All provider output fields, nil/empty distinctions, safe error chains, caller context, response reads/closes, client idle close idempotence, and caller snapshot reuse are observed. Serialization is byte-exact on replay."
	fixture["go_only_projections"] = []string{"Concrete error type names and response reader call counts describe pinned Go representations; semantic error classification, text, context matching, response bytes consumed, and close counts are retained.", "The random multipart boundary value is nondeterministic by Go design. It is validated with its actual raw header and wire framing, while only its invariant properties and lossless complete ordered parts enter the stable fixture."}
	fixture["limitations"] = []string{"Provider boundary only, not CLI/MCP composition, aggregator deduplication, source URL fetching, private helpers, or native transport fingerprint behavior.", "No live provider accounts, API credentials, third-party media, external network, browser, upload, HEAD, multiplex, or socket probes. Endpoint syntax is observed through net/http.Client with an injected RoundTripper; native transport scheme, redirect, and security enforcement are outside this scope.", "Closed-source upload failure uses only an owned fully consumed in-memory request pipe; no unfinished upload or denied supplemental probe is retried.", "Context cancellation is injected at bounded deterministic points; concurrent socket cancellation, redirect/security behavior, wall-clock timeout, and non-Linux runtime parity remain separate verification scopes.", "All 434 frozen Go production/module paths and all 104 fixtures tracked at the published base are byte-preserved; no Go production API, module manifest, or private-helper export was changed."}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "reverse-saucenao.json")
	if *migrationCaptureSauceNAO {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(expected, data) {
		t.Fatal("SauceNAO provider differs from frozen Go fixture; inspect actual protocol behavior before explicit recapture")
	}
	names := []string{}
	for name := range families {
		names = append(names, name)
	}
	sort.Strings(names)
	t.Logf("frozen Go %s SauceNAO provider: %d rows, 434 production/module paths, 104 published fixtures; bytes=%d sha256=%x", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
	for _, name := range names {
		t.Logf("%s: %d", name, families[name])
	}
}
