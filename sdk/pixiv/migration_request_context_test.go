package pixiv

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationCaptureRequestContext = flag.Bool("migration-capture-request-context", false, "capture public SDK request contexts from frozen Go")

const migrationRequestContextFixture = "../../crates/pixiv-sdk/tests/fixtures/request-context.json"
const migrationRequestContextArtwork = `{"illust":{"id":73,"type":"illust","create_date":"2026-01-01T00:00:00Z","page_count":1,"user":{"id":42},"meta_single_page":{"original_image_url":"https://i.pximg.net/context-fixture.png"}}}`
const migrationRequestContextOAuth = `{"access_token":"fixture-access","refresh_token":"fixture-rotated","user":{"id":42,"name":"fixture-user"}}`

type migrationRequestContextKey struct{}

type migrationRequestContextSnapshot struct {
	Value        string `json:"value"`
	ExactCaller  bool   `json:"go_exact_caller_context"`
	Deadline     string `json:"deadline_relation"`
	Error        string `json:"error"`
	ScopePresent bool   `json:"scope_present"`
	ExactScope   bool   `json:"go_exact_caller_scope"`
}

type migrationRequestContextRequest struct {
	Method  string                          `json:"method"`
	Host    string                          `json:"host"`
	Path    string                          `json:"path"`
	Context migrationRequestContextSnapshot `json:"context"`
}

type migrationRequestContextEvent struct {
	Caller    string `json:"caller"`
	Module    string `json:"module"`
	Kind      string `json:"kind"`
	RequestID uint64 `json:"request_id"`
	Route     string `json:"route"`
	Resource  string `json:"resource"`
	Status    int    `json:"status"`
}

type migrationRequestContextOutcome struct {
	Operation string `json:"operation"`
	Caller    string `json:"caller"`
	Reason    string `json:"reason"`
	Context   string `json:"context_error"`
	Result    string `json:"result"`
}

type migrationRequestContextInput struct {
	Caller    string `json:"caller"`
	State     string `json:"state"`
	Deadline  string `json:"deadline"`
	RequestID uint64 `json:"request_id"`
}

type migrationRequestContextCase struct {
	Name                  string                            `json:"name"`
	PacingMS              int64                             `json:"pacing_ms"`
	ClientTimeoutMS       int64                             `json:"client_timeout_ms"`
	ClientRelation        string                            `json:"go_http_client_relation"`
	GoOnly                string                            `json:"go_only,omitempty"`
	TransportBehavior     string                            `json:"transport_behavior"`
	Inputs                []migrationRequestContextInput    `json:"inputs"`
	Requests              []migrationRequestContextRequest  `json:"requests"`
	Outcomes              []migrationRequestContextOutcome  `json:"outcomes"`
	Events                []migrationRequestContextEvent    `json:"network_events"`
	Body                  []migrationRequestContextSnapshot `json:"body_contexts"`
	BodyReadError         string                            `json:"body_read_error,omitempty"`
	BodyBytes             string                            `json:"body_bytes,omitempty"`
	ExactTransportBody    bool                              `json:"go_exact_transport_body"`
	BodyReadsBeforeReturn int                               `json:"body_reads_before_return"`
	CallerAliveAfterClose bool                              `json:"caller_alive_after_body_close"`
	FinalContexts         []migrationRequestContextSnapshot `json:"final_contexts"`
}

type migrationRequestContextContract struct {
	SchemaVersion    int                           `json:"schema_version"`
	SourceCommit     string                        `json:"source_commit"`
	GoVersion        string                        `json:"go_version"`
	SourceSHA256     map[string]string             `json:"source_sha256"`
	DependencySHA256 map[string]string             `json:"dependency_sha256"`
	Cases            []migrationRequestContextCase `json:"cases"`
	SourceWitnesses  []string                      `json:"source_witnesses"`
	Limitations      []string                      `json:"limitations"`
}

type migrationRequestContextRecorder struct {
	mu       sync.Mutex
	inputs   []migrationRequestContextInput
	contexts map[string]context.Context
	requests []migrationRequestContextRequest
	events   []migrationRequestContextEvent
	bodies   []*migrationRequestContextBody
}

func migrationRequestContextNewRecorder() *migrationRequestContextRecorder {
	return &migrationRequestContextRecorder{contexts: map[string]context.Context{}, inputs: []migrationRequestContextInput{}, requests: []migrationRequestContextRequest{}, events: []migrationRequestContextEvent{}, bodies: []*migrationRequestContextBody{}}
}

type migrationRequestContextSink struct {
	recorder *migrationRequestContextRecorder
	label    string
}

func (s *migrationRequestContextSink) Emit(event diagnostics.Event) {
	s.recorder.mu.Lock()
	s.recorder.events = append(s.recorder.events, migrationRequestContextEvent{Caller: s.label, Module: string(event.Module), Kind: string(event.Kind), RequestID: event.RequestID, Route: event.Route, Resource: event.Resource, Status: event.Status})
	s.recorder.mu.Unlock()
}

func migrationRequestContextError(err error) string {
	switch {
	case err == nil:
		return ""
	case errors.Is(err, context.Canceled):
		return "canceled"
	case errors.Is(err, context.DeadlineExceeded):
		return "deadline_exceeded"
	default:
		return "other"
	}
}

func (r *migrationRequestContextRecorder) caller(t *testing.T, label, state string, id uint64) (context.Context, context.CancelFunc) {
	t.Helper()
	input := migrationRequestContextInput{Caller: label, State: state, RequestID: id}
	if state == "nil" {
		r.inputs = append(r.inputs, input)
		return nil, func() {}
	}
	deadline := time.Date(2100, 1, 1, 0, 0, int(id), 0, time.UTC)
	if state == "expired" {
		deadline = time.Date(2020, 1, 1, 0, 0, int(id), 0, time.UTC)
	}
	input.Deadline = deadline.Format(time.RFC3339)
	ctx, cancel := context.WithDeadline(context.WithValue(context.Background(), migrationRequestContextKey{}, label), deadline)
	ctx = diagnostics.WithScope(ctx, &migrationRequestContextSink{recorder: r, label: label}, diagnostics.ModulePixivMCP, id)
	if state == "canceled" {
		cancel()
	}
	t.Cleanup(cancel)
	r.inputs = append(r.inputs, input)
	r.contexts[label] = ctx
	return ctx, cancel
}

func (r *migrationRequestContextRecorder) snapshot(ctx context.Context) migrationRequestContextSnapshot {
	label, _ := ctx.Value(migrationRequestContextKey{}).(string)
	caller := r.contexts[label]
	got := migrationRequestContextSnapshot{Value: label, ExactCaller: caller != nil && ctx == caller, Error: migrationRequestContextError(ctx.Err()), Deadline: "none"}
	deadline, hasDeadline := ctx.Deadline()
	if hasDeadline {
		got.Deadline = "unmatched"
		if caller != nil {
			original, ok := caller.Deadline()
			if ok && original.Equal(deadline) {
				got.Deadline = "caller"
			} else if ok && deadline.Before(original) {
				got.Deadline = "http_client_shorter"
			}
		}
	}
	observed, ok := diagnostics.ScopeFromContext(ctx)
	got.ScopePresent = ok
	if caller != nil {
		expected, present := diagnostics.ScopeFromContext(caller)
		got.ExactScope = ok && present && observed == expected
	}
	return got
}

func (r *migrationRequestContextRecorder) observe(req *http.Request) {
	row := migrationRequestContextRequest{Method: req.Method, Host: req.URL.Host, Path: req.URL.Path, Context: r.snapshot(req.Context())}
	r.mu.Lock()
	r.requests = append(r.requests, row)
	r.mu.Unlock()
}

func (r *migrationRequestContextRecorder) response(req *http.Request, honorBody bool) *http.Response {
	payload := "{}"
	if req.URL.Path == "/auth/token" {
		payload = migrationRequestContextOAuth
	} else if req.URL.Host == "app-api.pixiv.net" && req.Method == http.MethodGet {
		payload = migrationRequestContextArtwork
	}
	var body io.ReadCloser = io.NopCloser(strings.NewReader(payload))
	if req.URL.Host == "i.pximg.net" {
		media := &migrationRequestContextBody{ctx: req.Context(), recorder: r, honor: honorBody, source: strings.NewReader("fixture-media"), contexts: []migrationRequestContextSnapshot{}}
		r.mu.Lock()
		r.bodies = append(r.bodies, media)
		r.mu.Unlock()
		body = media
	}
	return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: body, Request: req}
}

type migrationRequestContextTransport func(*http.Request) (*http.Response, error)

func (f migrationRequestContextTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	return f(req)
}

type migrationRequestContextPort struct {
	handler migrationRequestContextTransport
}

func (p *migrationRequestContextPort) RoundTrip(req *http.Request) (*http.Response, error) {
	return p.handler(req)
}

type migrationRequestContextBody struct {
	mu       sync.Mutex
	ctx      context.Context
	recorder *migrationRequestContextRecorder
	honor    bool
	source   *strings.Reader
	reads    int
	contexts []migrationRequestContextSnapshot
}

func (b *migrationRequestContextBody) Read(dst []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.reads++
	if b.reads == 1 {
		b.contexts = append(b.contexts, b.recorder.snapshot(b.ctx))
	}
	if b.honor && b.ctx.Err() != nil {
		return 0, b.ctx.Err()
	}
	return b.source.Read(dst)
}

func (b *migrationRequestContextBody) Close() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.contexts = append(b.contexts, b.recorder.snapshot(b.ctx))
	return nil
}

func migrationRequestContextNewClient(t *testing.T, r *migrationRequestContextRecorder, pace, timeout time.Duration, handler migrationRequestContextTransport) (*Client, string) {
	t.Helper()
	port := &migrationRequestContextPort{handler: handler}
	base := &http.Client{Transport: port, Timeout: timeout}
	client, err := NewWith("fixture-access", Options{HTTPClient: base, Pacing: Pacing{MinInterval: pace}})
	if err != nil {
		t.Fatal(err)
	}
	if pace <= 0 {
		if client.httpClient != base || client.selfHTTP {
			t.Fatal("unpaced injected client identity or ownership changed")
		}
		return client, "exact_caller_owned_client"
	}
	if client.httpClient == base || client.selfHTTP || base.Transport != port || client.httpClient.Timeout != timeout {
		t.Fatal("paced injected client derivation or ownership changed")
	}
	return client, "derived_caller_owned_client_with_shared_pacing_and_diagnostics"
}

func migrationRequestContextCall(t *testing.T, c *Client, ctx context.Context, op, label string) migrationRequestContextOutcome {
	t.Helper()
	result := migrationRequestContextOutcome{Operation: op, Caller: label}
	var err error
	switch op {
	case "Artwork":
		var artwork Artwork
		artwork, err = c.Artwork(ctx, ArtworkRequest{ArtworkID: 73})
		if err == nil {
			if artwork.ID != 73 {
				t.Fatalf("Artwork ID: %d", artwork.ID)
			}
			result.Result = "artwork_73"
		}
	case "AddArtworkBookmark":
		err = c.AddArtworkBookmark(ctx, AddArtworkBookmarkRequest{ArtworkID: 73})
		if err == nil {
			result.Result = "bookmark_success"
		}
	default:
		t.Fatalf("unknown context fixture operation %s", op)
	}
	result.Context = migrationRequestContextError(err)
	if err != nil {
		var classified *sdk.Error
		if !errors.As(err, &classified) {
			t.Fatalf("unclassified %s error: %v", op, err)
		}
		result.Reason = string(classified.Reason)
	}
	return result
}

func (r *migrationRequestContextRecorder) finish(row migrationRequestContextCase) migrationRequestContextCase {
	row.Inputs, row.Requests, row.Events = r.inputs, r.requests, r.events
	row.Body = []migrationRequestContextSnapshot{}
	row.FinalContexts = []migrationRequestContextSnapshot{}
	for _, body := range r.bodies {
		row.Body = append(row.Body, body.contexts...)
	}
	for _, input := range r.inputs {
		if ctx := r.contexts[input.Caller]; ctx != nil {
			row.FinalContexts = append(row.FinalContexts, r.snapshot(ctx))
		}
	}
	return row
}

func migrationRequestContextCheck(t *testing.T, row migrationRequestContextCase, requests int) {
	t.Helper()
	if len(row.Requests) != requests {
		t.Fatalf("%s request count=%d want=%d: %+v", row.Name, len(row.Requests), requests, row)
	}
	for _, request := range row.Requests {
		got := request.Context
		if got.Value == "" {
			if row.GoOnly == "" || got.ExactCaller || got.ScopePresent || got.Deadline != "none" {
				t.Fatalf("%s unexpected nil-context representation: %+v", row.Name, got)
			}
			continue
		}
		if !got.ScopePresent || !got.ExactScope || got.Deadline == "unmatched" || got.Deadline == "none" || got.ExactCaller != (row.ClientTimeoutMS == 0) {
			t.Fatalf("%s lost caller context contract: %+v", row.Name, got)
		}
	}
	if row.PacingMS == 0 && len(row.Events) != 0 {
		t.Fatalf("%s injected unpaced client unexpectedly emits SDK network events: %+v", row.Name, row.Events)
	}
	for _, event := range row.Events {
		if event.Module != string(diagnostics.ModulePixivNetwork) || event.RequestID == 0 || event.Caller == "" {
			t.Fatalf("%s network scope changed: %+v", row.Name, event)
		}
	}
}

func migrationRequestContextShared(t *testing.T, pace time.Duration, overlapping bool) migrationRequestContextCase {
	t.Helper()
	r := migrationRequestContextNewRecorder()
	a, cancelA := r.caller(t, "A", "active", 11)
	b, _ := r.caller(t, "B", "active", 22)
	c, _ := r.caller(t, "C", "active", 33)
	row := migrationRequestContextCase{Name: "shared_sequential", PacingMS: pace.Milliseconds(), TransportBehavior: "success_with_per_operation_context", Outcomes: []migrationRequestContextOutcome{}}
	entered := make(chan struct{})
	resultA := make(chan migrationRequestContextOutcome, 1)
	handler := migrationRequestContextTransport(func(req *http.Request) (*http.Response, error) {
		r.observe(req)
		if overlapping && req.Context().Value(migrationRequestContextKey{}) == "A" {
			close(entered)
			select {
			case <-req.Context().Done():
				return nil, req.Context().Err()
			case <-time.After(5 * time.Second):
				return nil, errors.New("fixture overlapping cancellation did not arrive")
			}
		}
		return r.response(req, false), nil
	})
	client, relation := migrationRequestContextNewClient(t, r, pace, 0, handler)
	row.ClientRelation = relation
	if overlapping {
		row.Name = "shared_overlapping_cancel_A_only"
		row.TransportBehavior = "A_waits_for_own_cancellation_B_and_future_C_succeed"
		go func() { resultA <- migrationRequestContextCall(t, client, a, "Artwork", "A") }()
		select {
		case <-entered:
		case <-time.After(5 * time.Second):
			t.Fatal("overlapping A did not reach transport")
		}
		outcomeB := migrationRequestContextCall(t, client, b, "AddArtworkBookmark", "B")
		cancelA()
		select {
		case outcomeA := <-resultA:
			if outcomeA.Context != "canceled" || outcomeA.Reason != string(sdk.UpstreamUnavailable) {
				t.Fatalf("overlapping A cancellation: %+v", outcomeA)
			}
			row.Outcomes = append(row.Outcomes, outcomeA, outcomeB)
		case <-time.After(5 * time.Second):
			t.Fatal("overlapping A did not finish")
		}
	} else {
		row.Outcomes = append(row.Outcomes, migrationRequestContextCall(t, client, a, "Artwork", "A"), migrationRequestContextCall(t, client, b, "AddArtworkBookmark", "B"), migrationRequestContextCall(t, client, a, "Artwork", "A"))
		cancelA()
		row.Outcomes = append(row.Outcomes, migrationRequestContextCall(t, client, b, "AddArtworkBookmark", "B"))
	}
	row.Outcomes = append(row.Outcomes, migrationRequestContextCall(t, client, c, "Artwork", "C"))
	for _, outcome := range row.Outcomes {
		if outcome.Caller != "A" || !overlapping {
			if outcome.Reason != "" || outcome.Result == "" {
				t.Fatalf("shared client operation failed: %+v", outcome)
			}
		}
	}
	if b.Err() != nil || c.Err() != nil || a.Err() != context.Canceled {
		t.Fatal("canceling A changed another caller or failed to cancel A")
	}
	if pace > 0 {
		row.Name += "_paced"
	} else {
		row.Name += "_unpaced"
	}
	row = r.finish(row)
	want := 5
	if overlapping {
		want = 3
	}
	migrationRequestContextCheck(t, row, want)
	if pace > 0 && len(row.Events) != want {
		t.Fatalf("shared scoped wrapper events=%d want=%d", len(row.Events), want)
	}
	return row
}

func migrationRequestContextSingle(t *testing.T, op, state string, honor bool, pace, timeout time.Duration, retry bool) migrationRequestContextCase {
	t.Helper()
	r := migrationRequestContextNewRecorder()
	ctx, _ := r.caller(t, "single", state, 44)
	behavior := "returns_success_even_if_context_done"
	if honor {
		behavior = "returns_context_error_when_done"
	}
	row := migrationRequestContextCase{Name: strings.ToLower(op) + "_" + state + "_" + behavior, PacingMS: pace.Milliseconds(), ClientTimeoutMS: timeout.Milliseconds(), TransportBehavior: behavior, Outcomes: []migrationRequestContextOutcome{}}
	if state == "nil" {
		row.GoOnly = "nil_context_representation_not_a_portable_Rust_input"
	}
	if retry {
		row.Name += "_retry_after"
		row.TransportBehavior = "first_429_then_success_with_same_context_unless_retry_wait_canceled"
	}
	calls := 0
	handler := migrationRequestContextTransport(func(req *http.Request) (*http.Response, error) {
		r.observe(req)
		calls++
		if honor && req.Context().Err() != nil {
			return nil, req.Context().Err()
		}
		if retry && calls == 1 {
			after := "0"
			if state == "canceled" {
				after = "60"
			}
			return &http.Response{StatusCode: 429, Header: http.Header{"Retry-After": {after}}, Body: io.NopCloser(strings.NewReader("{}")), Request: req}, nil
		}
		return r.response(req, false), nil
	})
	client, relation := migrationRequestContextNewClient(t, r, pace, timeout, handler)
	row.ClientRelation = relation
	outcome := migrationRequestContextOutcome{Operation: op, Caller: "single"}
	var err error
	extraOutcomes := []migrationRequestContextOutcome{}
	switch op {
	case "Artwork", "AddArtworkBookmark":
		outcome = migrationRequestContextCall(t, client, ctx, op, "single")
	case "OpenWith":
		var opened *Client
		var credentials Credentials
		opened, credentials, err = OpenWith(ctx, "fixture-refresh", Options{HTTPClient: &http.Client{Transport: handler, Timeout: timeout}, Pacing: Pacing{MinInterval: pace}})
		if err == nil {
			if opened.UserID() != 42 || credentials.AccessToken() != "fixture-access" {
				t.Fatal("OAuth synthetic identity/token changed")
			}
			outcome.Result = "opened_identity_42"
			future, _ := r.caller(t, "after-open", "active", 45)
			futureOutcome := migrationRequestContextCall(t, opened, future, "Artwork", "after-open")
			if futureOutcome.Reason != "" || futureOutcome.Result != "artwork_73" {
				t.Fatalf("Open context retained by returned client: %+v", futureOutcome)
			}
			extraOutcomes = append(extraOutcomes, futureOutcome)
		}
	case "OpenResource":
		ref, refErr := sdk.NewResourceRef("pixiv", []byte(`{"k":"artwork","id":73,"v":"original"}`))
		if refErr != nil {
			t.Fatal(refErr)
		}
		var response *sdk.ResourceResponse
		response, err = client.OpenResource(ctx, sdk.OpenResourceRequest{Ref: ref})
		if err == nil {
			body := r.bodies[len(r.bodies)-1]
			row.BodyReadsBeforeReturn, row.ExactTransportBody = body.reads, response.Body == body
			data, readErr := io.ReadAll(response.Body)
			if readErr != nil {
				t.Fatal(readErr)
			}
			row.BodyBytes = string(data)
			if closeErr := response.Body.Close(); closeErr != nil {
				t.Fatal(closeErr)
			}
			if row.BodyReadsBeforeReturn != 0 || row.BodyBytes != "fixture-media" || row.ExactTransportBody != (timeout == 0) {
				t.Fatalf("resource streaming/identity changed: %+v", row)
			}
			outcome.Result = "resource_200"
		}
	default:
		t.Fatalf("unknown single op %s", op)
	}
	if err != nil {
		var classified *sdk.Error
		if !errors.As(err, &classified) {
			t.Fatal(err)
		}
		outcome.Reason, outcome.Context = string(classified.Reason), migrationRequestContextError(err)
	}
	wantError := honor && (state == "canceled" || state == "expired") || state == "nil" && op == "OpenResource" || retry && state == "canceled"
	if wantError {
		if outcome.Reason != string(sdk.UpstreamUnavailable) || outcome.Result != "" {
			t.Fatalf("context error result: %+v", outcome)
		}
		if state != "nil" && outcome.Context != map[string]string{"canceled": "canceled", "expired": "deadline_exceeded"}[state] {
			t.Fatalf("context error chain: %+v", outcome)
		}
	} else if outcome.Reason != "" || outcome.Result == "" {
		t.Fatalf("injected successful response discarded: %+v", outcome)
	}
	row.Outcomes = append(row.Outcomes, outcome)
	row.Outcomes = append(row.Outcomes, extraOutcomes...)
	if timeout > 0 {
		row.Name += "_client_timeout"
	}
	if pace > 0 {
		row.Name += "_paced"
	}
	row = r.finish(row)
	want := 1
	if op == "OpenWith" && !wantError {
		want = 2
	}
	if op == "OpenResource" && state != "nil" {
		want = 2
	}
	if retry && state != "canceled" {
		want = 2
	}
	migrationRequestContextCheck(t, row, want)
	if timeout > 0 && row.Requests[0].Context.Deadline != "http_client_shorter" {
		t.Fatal("HTTP client timeout did not derive a shorter context")
	}
	return row
}

func migrationRequestContextStreaming(t *testing.T, timeout time.Duration) migrationRequestContextCase {
	t.Helper()
	r := migrationRequestContextNewRecorder()
	a, cancelA := r.caller(t, "stream-A", "active", 55)
	b, _ := r.caller(t, "stream-B", "active", 66)
	row := migrationRequestContextCase{Name: "returned_resource_body_cancellation_and_future_call", ClientTimeoutMS: timeout.Milliseconds(), TransportBehavior: "headers_succeed_body_reads_honor_own_context", Outcomes: []migrationRequestContextOutcome{}}
	handler := migrationRequestContextTransport(func(req *http.Request) (*http.Response, error) { r.observe(req); return r.response(req, true), nil })
	client, relation := migrationRequestContextNewClient(t, r, 0, timeout, handler)
	row.ClientRelation = relation
	ref, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"artwork","id":73,"v":"original"}`))
	if err != nil {
		t.Fatal(err)
	}
	response, err := client.OpenResource(a, sdk.OpenResourceRequest{Ref: ref})
	if err != nil {
		t.Fatal(err)
	}
	bodyA := r.bodies[0]
	row.BodyReadsBeforeReturn, row.ExactTransportBody = bodyA.reads, response.Body == bodyA
	if response.StatusCode != 200 || bodyA.reads != 0 || row.ExactTransportBody != (timeout == 0) {
		t.Fatal("OpenResource eagerly consumed/replaced an unwrapped body")
	}
	row.Outcomes = append(row.Outcomes, migrationRequestContextOutcome{Operation: "OpenResource", Caller: "stream-A", Result: "resource_200_before_body_read"})
	cancelA()
	_, readErr := response.Body.Read(make([]byte, 1))
	row.BodyReadError = migrationRequestContextError(readErr)
	if row.BodyReadError != "canceled" {
		t.Fatalf("returned body cancellation: %v", readErr)
	}
	if err = response.Body.Close(); err != nil {
		t.Fatal(err)
	}
	future, err := client.OpenResource(b, sdk.OpenResourceRequest{Ref: ref})
	if err != nil {
		t.Fatal(err)
	}
	data, err := io.ReadAll(future.Body)
	if err != nil {
		t.Fatal(err)
	}
	row.BodyBytes = string(data)
	if err := future.Body.Close(); err != nil {
		t.Fatal(err)
	}
	row.CallerAliveAfterClose = b.Err() == nil
	if row.BodyBytes != "fixture-media" || !row.CallerAliveAfterClose {
		t.Fatal("A cancellation or body close invalidated future B")
	}
	row.Outcomes = append(row.Outcomes, migrationRequestContextOutcome{Operation: "OpenResource", Caller: "stream-B", Result: "cached_resource_200"})
	if timeout > 0 {
		row.Name += "_client_timeout"
	}
	row = r.finish(row)
	row.Body = append(row.Body, r.snapshot(r.bodies[1].ctx))
	migrationRequestContextCheck(t, row, 3)
	wantFinalBodyError := ""
	if timeout > 0 {
		wantFinalBodyError = "canceled"
	}
	if row.Body[len(row.Body)-1].Error != wantFinalBodyError {
		t.Fatalf("HTTP timeout body close context lifetime: %+v", row.Body)
	}
	return row
}

func TestMigrationRequestContextMatchesFrozenGo(t *testing.T) {
	contract := migrationRequestContextContract{SchemaVersion: 1, SourceCommit: "4b4426487ef18bed276706daec385e0d0a6979f9", GoVersion: runtime.Version(), SourceSHA256: migrationRequestContextSourceSHA256, DependencySHA256: migrationRequestContextDependencySHA256, Cases: []migrationRequestContextCase{}}
	for path, expected := range contract.SourceSHA256 {
		source, err := os.ReadFile(filepath.Join("..", "..", path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(source)); got != expected {
			t.Fatalf("frozen request-context source %s changed: %s", path, got)
		}
	}
	for name, expected := range contract.DependencySHA256 {
		path := filepath.Join(os.Getenv("GOMODCACHE"), "github.com", "go-resty", "resty", "v2@v2.17.2", name)
		if strings.HasPrefix(name, "Go/") {
			path = filepath.Join(runtime.GOROOT(), strings.TrimPrefix(name, "Go/"))
		}
		source, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(source)); got != expected {
			t.Fatalf("request-context dependency %s changed: %s", name, got)
		}
	}
	for _, pace := range []time.Duration{0, time.Millisecond} {
		contract.Cases = append(contract.Cases, migrationRequestContextShared(t, pace, false), migrationRequestContextShared(t, pace, true))
	}
	for _, op := range []string{"Artwork", "AddArtworkBookmark", "OpenWith", "OpenResource"} {
		for _, state := range []string{"canceled", "expired", "nil"} {
			contract.Cases = append(contract.Cases, migrationRequestContextSingle(t, op, state, false, 0, 0, false))
		}
	}
	for _, state := range []string{"canceled", "expired"} {
		contract.Cases = append(contract.Cases, migrationRequestContextSingle(t, "Artwork", state, true, 0, 0, false))
	}
	contract.Cases = append(contract.Cases,
		migrationRequestContextSingle(t, "Artwork", "active", false, 0, time.Minute, false),
		migrationRequestContextSingle(t, "Artwork", "canceled", false, time.Millisecond, 0, false),
		migrationRequestContextSingle(t, "Artwork", "active", false, 0, 0, true),
		migrationRequestContextSingle(t, "Artwork", "canceled", false, 0, 0, true),
		migrationRequestContextStreaming(t, 0),
		migrationRequestContextStreaming(t, time.Minute))
	contract.SourceWitnesses = []string{
		"Go public SDK methods receive context per operation; NewWith stores no caller context, and OpenWith's OAuth context is not retained as the returned client's future operation context",
		"Options.HTTPClient with nonpositive pacing returns the exact caller client without diagnostic wrapping; Resty NewWithClient may initialize its nil Transport, independently frozen by client-ownership.json",
		"positive pacing derives a caller-owned client, preserving Timeout/Jar/CheckRedirect and sharing diagnostic then pacing wrappers across operation families",
		"Go net/http.Client Timeout can derive a shorter request context and wrap returned Body in cancelTimerBody; Close cancels that derived context without canceling the caller",
		"Resty SetContext(nil) leaves the net/http request's Background context; resource.Open instead passes nil to NewRequestWithContext and returns a classified transport error after any required metadata lookup",
		"retry wait selects caller Done and propagates cancellation; pacing checks Done only when a positive wait is needed, so the first canceled call can still reach an injected transport",
		"OpenResource propagates the same per-operation context through uncached metadata and media requests and returns an unread body; resolved URLs are client-scoped and future context calls reuse that registry",
	}
	contract.Limitations = []string{
		"Only synthetic public SDK calls, ordinary injected net/http.RoundTripper implementations and context-aware response-body ports are exercised; no external network or native protocol probes",
		"go_exact_caller_context, go_exact_caller_scope, go_http_client_relation, go_exact_transport_body and nil_context cases are Go representation witnesses, not Rust pointer/nil requirements; Rust correspondence must preserve the portable context value, deadline, scope and isolated cancellation outcomes",
		"deadline_relation records exact equality or strictly earlier derived deadline, not a wall-clock timestamp invented for client Timeout; each active input has a fixed explicit future deadline",
		"No global/task-local context or automatic broadcast cancellation is modeled; no unconditional cancellation precheck may discard a successful injected response",
		"This representative slice does not duplicate the full OAuth PKCE, resource policy, retry or pacing matrices, and makes no Rust implementation or native HTTP/2 coverage claim",
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *migrationCaptureRequestContext {
		if err := os.WriteFile(migrationRequestContextFixture, data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(migrationRequestContextFixture)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, expected) {
		t.Fatalf("public SDK per-request context differs from frozen Go:\n%s", data)
	}
	t.Logf("frozen Go %s: %d public SDK per-request context cases", contract.GoVersion, len(contract.Cases))
}

var migrationRequestContextSourceSHA256 = map[string]string{
	"go.mod":                    "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum":                    "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	"sdk/pixiv/pixiv.go":        "daefb42f9f90359f5ce3d326d18df7afe8c10615750d88fef049cce78224d317",
	"sdk/pixiv/ops_artwork.go":  "f8aa00684b84463c6ba82b18db87d4c2e403a0dcaa048f3c3282f6555fd7445c",
	"sdk/pixiv/ops_mutation.go": "82e3e91b897ba8933ec1037d867052f7480da357e93cf71629c5f877f20baf0a",
	"sdk/pixiv/resource.go":     "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
	"sdk/pixiv/errors.go":       "9a1830393129ca195ef4d57c0bda17f219f0f750c7293abf5520aa888bfe81a8",
	"sdk/error.go":              "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
	"internal/services/pixiv/appapi/appapi.go":     "b5d9502ad9c534c88bda076740553c8bb6f389c3d2f4e3bd80c32c7f281ba9ff",
	"internal/services/pixiv/oauth/oauth.go":       "cdc91d906255256f452789a3ce4b5efd8844e5b84a44f29756c1172b051ce376",
	"internal/services/pixiv/resource/resource.go": "c516f925b15a20531eba51f83e6a7797280b6569ad0fd519591e9330199b581a",
	"internal/shared/diagnostics/diagnostics.go":   "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76",
}

var migrationRequestContextDependencySHA256 = map[string]string{
	"client.go":                  "bc0107de3d9a596b6843dfd864f71208b81aa9c17402fd068a69d4acecb8783f",
	"request.go":                 "77909fcd89b143995236e55474628c27398f55c6b1723792b4ef6434dd699030",
	"middleware.go":              "0d2ed4e3a7d0e120007c57238d6f99e1d46f07d1e978f6c7abf4166b4dfcf4c6",
	"Go/src/net/http/client.go":  "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9",
	"Go/src/net/http/request.go": "c3257079994b4e4f74f01cef72508919983ba31f91fcf599d348e9d52cec1540",
	"Go/src/context/context.go":  "971f00ffa375b79d3f65e6da33cca3a493cc3fce091c589233abfb4283e29b7d",
}
