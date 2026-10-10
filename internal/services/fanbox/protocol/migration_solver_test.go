package protocol

import (
	"archive/zip"
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
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
)

var migrationCaptureSolver = flag.Bool("migration-capture-fanbox-solver", false, "capture frozen Go FANBOX solver workflow")

const migrationSolverDocument = `{"status":"ok","solution":{"userAgent":"synthetic-solver-agent","cookies":[{"name":"cf_clearance","value":"synthetic-clearance"}]}}`

type migrationSolverRow struct {
	Name        string `json:"name"`
	Family      string `json:"family"`
	Input       any    `json:"input"`
	Observation any    `json:"observation"`
}

type migrationSolverTime struct {
	String      string `json:"go_only_time_string"`
	RFC3339Nano string `json:"rfc3339_nano"`
	Unix        int64  `json:"unix_seconds"`
	Nanosecond  int    `json:"nanosecond"`
	Location    string `json:"go_only_location"`
	Zero        bool   `json:"is_zero"`
}

func migrationSolverTimeOf(value time.Time) migrationSolverTime {
	return migrationSolverTime{value.String(), value.Format(time.RFC3339Nano), value.Unix(), value.Nanosecond(), value.Location().String(), value.IsZero()}
}

type migrationSolverStateView struct {
	UserAgent string              `json:"private_user_agent"`
	Clearance string              `json:"private_clearance"`
	ExpiresAt migrationSolverTime `json:"private_expires_at"`
	HasExpiry bool                `json:"private_has_expiry"`
}

func migrationSolverStateOf(state solverState) migrationSolverStateView {
	return migrationSolverStateView{state.userAgent, state.clearance, migrationSolverTimeOf(state.expiresAt), state.hasExpiry}
}

func migrationSolverErrorOf(err error) map[string]any {
	chain := []map[string]string{}
	for cause := err; cause != nil; cause = errors.Unwrap(cause) {
		chain = append(chain, map[string]string{"go_only_type": fmt.Sprintf("%T", cause), "message": cause.Error()})
	}
	return map[string]any{"chain": chain, "challenge": errors.Is(err, ErrChallenge), "forbidden": errors.Is(err, ErrForbidden), "not_authenticated": errors.Is(err, ErrNotAuthenticated), "solver_unavailable": errors.Is(err, ErrSolverUnavailable), "solver_failed": errors.Is(err, ErrSolverFailed), "malformed_solver": errors.Is(err, ErrMalformedSolverResponse), "canceled": errors.Is(err, context.Canceled), "deadline": errors.Is(err, context.DeadlineExceeded)}
}

type migrationSolverContextKey struct{}

func migrationSolverContextOf(ctx context.Context) map[string]any {
	if ctx == nil {
		return map[string]any{"nil": true}
	}
	deadlineTime, deadline := ctx.Deadline()
	_, scope := diagnostics.ScopeFromContext(ctx)
	return map[string]any{"nil": false, "go_only_type": fmt.Sprintf("%T", ctx), "done_present": ctx.Done() != nil, "has_deadline": deadline, "deadline": migrationSolverTimeOf(deadlineTime), "value": ctx.Value(migrationSolverContextKey{}), "diagnostic_scope": scope, "error": migrationSolverErrorOf(ctx.Err())}
}

func migrationSolverCallOf(call *solverCall) any {
	if call == nil {
		return nil
	}
	closed := false
	select {
	case <-call.done:
		closed = true
	default:
	}
	return map[string]any{"private_context": migrationSolverContextOf(call.ctx), "private_cancel_present": call.cancel != nil, "private_waiters": call.waiters, "private_done_present": call.done != nil, "private_done_closed": closed, "private_state": migrationSolverStateOf(call.state), "private_error": migrationSolverErrorOf(call.err)}
}

func migrationSolverSessionOf(session *Session) any {
	if session == nil {
		return nil
	}
	session.solverMu.Lock()
	defer session.solverMu.Unlock()
	var state any
	if session.solverState != nil {
		state = migrationSolverStateOf(*session.solverState)
	}
	return map[string]any{"private_solver_state": state, "private_solver_active": migrationSolverCallOf(session.solverActive), "go_only_control_client_present": session.solverHTTPClient != nil, "go_only_solver_configured": session.flareSolverr != nil}
}

type migrationSolverRecorder struct {
	mu     sync.Mutex
	trace  []string
	events []diagnostics.Event
}

func (r *migrationSolverRecorder) add(value string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.trace = append(r.trace, value)
}

func (r *migrationSolverRecorder) Emit(event diagnostics.Event) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, event)
	r.trace = append(r.trace, "diagnostic:"+string(event.Kind))
}

func (r *migrationSolverRecorder) view() any {
	r.mu.Lock()
	defer r.mu.Unlock()
	return map[string]any{"trace": append([]string{}, r.trace...), "typed_events": append([]diagnostics.Event{}, r.events...)}
}

type migrationSolverStep struct {
	Status         int         `json:"status"`
	Body           string      `json:"body"`
	Header         http.Header `json:"header"`
	ReadError      string      `json:"read_error"`
	CloseError     string      `json:"close_error"`
	TransportError string      `json:"transport_error"`
	Chunk          int         `json:"chunk"`
	NilBody        bool        `json:"nil_body"`
	NilResponse    bool        `json:"nil_response"`
	ContentLength  int64       `json:"content_length"`
	CancelOnRead   bool        `json:"cancel_on_read"`
	CancelOnClose  bool        `json:"cancel_on_close"`
}

type migrationSolverBody struct {
	mu                   sync.Mutex
	reader               *strings.Reader
	step                 migrationSolverStep
	recorder             *migrationSolverRecorder
	name                 string
	cancel               context.CancelFunc
	reads, bytes, closes int
	trace                []string
}

func migrationSolverInjectedError(kind string) error {
	switch kind {
	case "":
		return nil
	case "canceled":
		return context.Canceled
	case "deadline":
		return context.DeadlineExceeded
	default:
		return errors.New("synthetic-external-canary:" + kind)
	}
}

func (b *migrationSolverBody) Read(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.reads++
	b.trace = append(b.trace, "read")
	b.recorder.add(b.name + ":read")
	if b.step.CancelOnRead {
		b.cancel()
	}
	if b.step.Chunk > 0 && len(p) > b.step.Chunk {
		p = p[:b.step.Chunk]
	}
	n, err := b.reader.Read(p)
	b.bytes += n
	if b.step.ReadError != "" && b.reader.Len() == 0 {
		err = migrationSolverInjectedError(b.step.ReadError)
	}
	return n, err
}

func (b *migrationSolverBody) Close() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.closes++
	b.trace = append(b.trace, "close")
	b.recorder.add(b.name + ":close")
	if b.step.CancelOnClose {
		b.cancel()
	}
	return migrationSolverInjectedError(b.step.CloseError)
}

func (b *migrationSolverBody) view() any {
	b.mu.Lock()
	defer b.mu.Unlock()
	return map[string]any{"go_only_read_calls": b.reads, "bytes_read": b.bytes, "close_calls": b.closes, "go_only_trace": append([]string{}, b.trace...)}
}

type migrationSolverRequest struct {
	Method        string      `json:"method"`
	URL           string      `json:"url"`
	Host          string      `json:"host"`
	Header        http.Header `json:"header"`
	Body          string      `json:"body"`
	ContentLength int64       `json:"content_length"`
	Context       any         `json:"go_only_context"`
}

type migrationSolverTransport struct {
	mu       sync.Mutex
	t        *testing.T
	name     string
	steps    []migrationSolverStep
	requests []migrationSolverRequest
	bodies   []*migrationSolverBody
	recorder *migrationSolverRecorder
	cancel   context.CancelFunc
	block    func(*http.Request)
}

func (transport *migrationSolverTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	var payload []byte
	if request.Body != nil {
		var err error
		payload, err = io.ReadAll(request.Body)
		closeErr := request.Body.Close()
		if err != nil || closeErr != nil {
			return nil, errors.New("owned request body failed")
		}
	}
	transport.mu.Lock()
	index := len(transport.requests)
	transport.requests = append(transport.requests, migrationSolverRequest{request.Method, request.URL.String(), request.Host, request.Header.Clone(), string(payload), request.ContentLength, migrationSolverContextOf(request.Context())})
	transport.recorder.add(transport.name + ":request")
	if index >= len(transport.steps) {
		transport.mu.Unlock()
		transport.t.Errorf("unexpected %s request %d", transport.name, index)
		return nil, errors.New("unexpected owned request")
	}
	step := transport.steps[index]
	var body *migrationSolverBody
	if !step.NilBody && !step.NilResponse && step.TransportError == "" {
		body = &migrationSolverBody{reader: strings.NewReader(step.Body), step: step, recorder: transport.recorder, name: fmt.Sprintf("%s-body-%d", transport.name, index), cancel: transport.cancel, trace: []string{}}
		transport.bodies = append(transport.bodies, body)
	}
	transport.mu.Unlock()
	if transport.block != nil {
		transport.block(request)
	}
	if step.TransportError != "" {
		return nil, migrationSolverInjectedError(step.TransportError)
	}
	if step.NilResponse {
		return nil, nil
	}
	response := &http.Response{StatusCode: step.Status, Header: step.Header.Clone(), Request: request, ContentLength: step.ContentLength}
	if body != nil {
		response.Body = body
	}
	return response, nil
}

func (transport *migrationSolverTransport) view() any {
	transport.mu.Lock()
	defer transport.mu.Unlock()
	bodies := []any{}
	for _, body := range transport.bodies {
		bodies = append(bodies, body.view())
	}
	return map[string]any{"requests": append([]migrationSolverRequest{}, transport.requests...), "owned_response_bodies": bodies}
}

func (transport *migrationSolverTransport) assertOwnedControl(t *testing.T) {
	t.Helper()
	transport.mu.Lock()
	defer transport.mu.Unlock()
	for _, request := range transport.requests {
		if request.Method != http.MethodPost || request.Header.Get("Cookie") != "" || request.Header.Get("User-Agent") != "" || request.Header.Get("Origin") != "" || request.Header.Get("Referer") != "" {
			t.Fatal("anonymous solver control inherited native request metadata")
		}
		for _, privateValue := range []string{"synthetic-session", "synthetic-business", "native-proxy.example"} {
			if strings.Contains(request.URL, privateValue) || strings.Contains(request.Body, privateValue) {
				t.Fatal("anonymous solver control inherited native credential, route or proxy")
			}
		}
		var payload struct {
			Command string `json:"cmd"`
			URL     string `json:"url"`
		}
		if err := json.Unmarshal([]byte(request.Body), &payload); err != nil || payload.Command != "request.get" || payload.URL != WebBaseURL {
			t.Fatal("solver control changed the anonymous homepage operation")
		}
	}
	for _, body := range transport.bodies {
		body.mu.Lock()
		closes := body.closes
		body.mu.Unlock()
		if closes != 1 {
			t.Fatalf("owned solver response close calls = %d, want one", closes)
		}
	}
}

func migrationSolverNewSession(t *testing.T, recorder *migrationSolverRecorder, nativeSteps, controlSteps []migrationSolverStep, options *FlareSolverrOptions, cancel context.CancelFunc) (*Session, *migrationSolverTransport, *migrationSolverTransport) {
	t.Helper()
	native := &migrationSolverTransport{t: t, name: "native", steps: nativeSteps, recorder: recorder, cancel: cancel}
	control := &migrationSolverTransport{t: t, name: "control", steps: controlSteps, recorder: recorder, cancel: cancel}
	session, err := NewSessionWithOptions("FANBOXSESSID=synthetic-session", SessionOptions{HTTPClient: &http.Client{Transport: native}, SolverHTTPClient: &http.Client{Transport: control, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}, ProxyURL: "https://native-proxy.example:8443/path", UserAgent: "synthetic-native-agent", FlareSolverr: options})
	if err != nil {
		t.Fatal(err)
	}
	return session, native, control
}

func migrationSolverContext(recorder *migrationSolverRecorder) (context.Context, context.CancelFunc) {
	ctx := context.WithValue(context.Background(), migrationSolverContextKey{}, "synthetic-context-value")
	ctx = diagnostics.WithScope(ctx, recorder, diagnostics.ModuleFanboxCLI, 73)
	return context.WithCancel(ctx)
}

func migrationSolverValidationRows(t *testing.T) []migrationSolverRow {
	t.Helper()
	rows := []migrationSolverRow{}
	base := func() *solverSolution {
		return &solverSolution{UserAgent: "synthetic-solver-agent", Cookies: []solverCookie{{Name: "cf_clearance", Value: "synthetic-clearance"}}}
	}
	solutions := []struct {
		name  string
		value *solverSolution
	}{{"nil", nil}, {"valid", base()}, {"no_cookies", &solverSolution{UserAgent: "agent"}}, {"empty_cookies", &solverSolution{UserAgent: "agent", Cookies: []solverCookie{}}}}
	for _, test := range []struct{ name, value string }{{"empty", ""}, {"spaces", " \t "}, {"tab", "a\tb"}, {"newline", "a\nb"}, {"nul", "a\x00b"}, {"del", "a\x7fb"}, {"unicode", "日本語"}, {"surrounding_spaces", " agent "}, {"non_ascii_control", "a\u0085b"}} {
		value := base()
		value.UserAgent = test.value
		solutions = append(solutions, struct {
			name  string
			value *solverSolution
		}{"agent_" + test.name, value})
	}
	for _, test := range []struct{ name, value string }{{"empty", ""}, {"space", "a b"}, {"quote", "a\"b"}, {"comma", "a,b"}, {"semicolon", "a;b"}, {"backslash", "a\\b"}, {"nul", "a\x00b"}, {"unicode", "日本語"}, {"del", "a\x7fb"}, {"ascii_boundaries", "!#$%&'()*+-./:<=>?@[]^_`{|}~"}} {
		value := base()
		value.Cookies[0].Value = test.value
		solutions = append(solutions, struct {
			name  string
			value *solverSolution
		}{"clearance_" + test.name, value})
	}
	for _, test := range []struct {
		name    string
		cookies []solverCookie
	}{{"wrong_case", []solverCookie{{Name: "CF_CLEARANCE", Value: "value"}}}, {"other_cookies_only", []solverCookie{{Name: "FANBOXSESSID", Value: "ignored"}}}, {"duplicate", []solverCookie{{Name: "cf_clearance", Value: "one"}, {Name: "cf_clearance", Value: "two"}}}, {"ignored_invalid_cookie", []solverCookie{{Name: "FANBOXSESSID", Value: "\n;invalid", Expires: json.RawMessage(`{}`)}, {Name: "cf_clearance", Value: "value"}}}, {"expiry_wins", []solverCookie{{Name: "cf_clearance", Value: "value", Expiry: json.RawMessage(`1`), Expires: json.RawMessage(`"invalid"`)}}}, {"expiry_null_falls_back", []solverCookie{{Name: "cf_clearance", Value: "value", Expiry: json.RawMessage(`null`), Expires: json.RawMessage(`1`)}}}, {"expiry_space_null_overrides", []solverCookie{{Name: "cf_clearance", Value: "value", Expiry: json.RawMessage(` null `), Expires: json.RawMessage(`1`)}}}, {"expiry_invalid_overrides", []solverCookie{{Name: "cf_clearance", Value: "value", Expiry: json.RawMessage(`0`), Expires: json.RawMessage(`1`)}}}} {
		solutions = append(solutions, struct {
			name  string
			value *solverSolution
		}{test.name, &solverSolution{UserAgent: "agent", Cookies: test.cookies}})
	}
	for _, test := range solutions {
		state, err := validateSolverSolution(test.value)
		rows = append(rows, migrationSolverRow{"solution/" + test.name, "solution_validation", test.value, map[string]any{"state": migrationSolverStateOf(state), "error": migrationSolverErrorOf(err)}})
	}
	for _, test := range []struct{ name, raw string }{{"absent", ""}, {"null", "null"}, {"spaced_null", " null "}, {"one", "1"}, {"zero", "0"}, {"negative", "-1"}, {"fraction", "1.5"}, {"integer_decimal", "1.0"}, {"exponent", "1e3"}, {"max_i64", "9223372036854775807"}, {"overflow", "9223372036854775808"}, {"numeric_string", `"1"`}, {"rfc3339", `"2026-10-10T01:02:03Z"`}, {"fraction_offset", `"2026-10-10T01:02:03.123456789+09:00"`}, {"http_date", `"Sat, 10 Oct 2026 01:02:03 GMT"`}, {"past_date", `"1960-01-01T00:00:00Z"`}, {"empty_string", `""`}, {"boolean", "true"}, {"object", "{}"}, {"array", "[]"}, {"invalid_json", "{"}, {"whitespace_string", `" 2026-10-10T01:02:03Z "`}} {
		expiry, has, err := parseSolverExpiry(json.RawMessage(test.raw))
		rows = append(rows, migrationSolverRow{"expiry/" + test.name, "expiry", test.raw, map[string]any{"expires_at": migrationSolverTimeOf(expiry), "has_expiry": has, "error": migrationSolverErrorOf(err)}})
	}
	return rows
}

func migrationSolverControlRows(t *testing.T) []migrationSolverRow {
	t.Helper()
	rows := []migrationSolverRow{}
	tests := []struct {
		name     string
		step     migrationSolverStep
		proxy    string
		canceled bool
	}{
		{"valid", migrationSolverStep{Status: 200, Body: migrationSolverDocument}, "", false},
		{"upstream_proxy", migrationSolverStep{Status: 201, Body: migrationSolverDocument}, "socks5://upstream-proxy.example:1080/", false},
		{"status_299", migrationSolverStep{Status: 299, Body: migrationSolverDocument}, "", false},
		{"status_300", migrationSolverStep{Status: 300, Body: migrationSolverDocument}, "", false},
		{"status_500", migrationSolverStep{Status: 500, Body: migrationSolverDocument}, "", false},
		{"empty_body", migrationSolverStep{Status: 200}, "", false},
		{"nil_body_client_supplies_empty", migrationSolverStep{Status: 200, NilBody: true}, "", false},
		{"nil_body_positive_length_client_rejects", migrationSolverStep{Status: 200, NilBody: true, ContentLength: 16}, "", false},
		{"nil_response_client_rejects", migrationSolverStep{NilResponse: true}, "", false},
		{"invalid_json", migrationSolverStep{Status: 200, Body: "{"}, "", false},
		{"status_case", migrationSolverStep{Status: 200, Body: strings.Replace(migrationSolverDocument, `"ok"`, `"OK"`, 1)}, "", false},
		{"missing_status", migrationSolverStep{Status: 200, Body: `{"solution":{}}`}, "", false},
		{"null_document", migrationSolverStep{Status: 200, Body: `null`}, "", false},
		{"missing_solution", migrationSolverStep{Status: 200, Body: `{"status":"ok"}`}, "", false},
		{"wrong_solution_type", migrationSolverStep{Status: 200, Body: `{"status":"ok","solution":[]}`}, "", false},
		{"trailing_second_json_ignored", migrationSolverStep{Status: 200, Body: migrationSolverDocument + ` {"status":"error"}`}, "", false},
		{"trailing_garbage_ignored", migrationSolverStep{Status: 200, Body: migrationSolverDocument + ` invalid`}, "", false},
		{"duplicate_status_last_wins", migrationSolverStep{Status: 200, Body: strings.Replace(migrationSolverDocument, `"status":"ok"`, `"status":"error","status":"ok"`, 1)}, "", false},
		{"unknown_fields", migrationSolverStep{Status: 200, Body: strings.Replace(migrationSolverDocument, `"status":"ok"`, `"secret":"synthetic-ignored","status":"ok"`, 1)}, "", false},
		{"read_failure_before_json", migrationSolverStep{Status: 200, Body: "{", ReadError: "read"}, "", false},
		{"complete_json_and_read_error", migrationSolverStep{Status: 200, Body: migrationSolverDocument, ReadError: "read"}, "", false},
		{"close_failure_ignored", migrationSolverStep{Status: 200, Body: migrationSolverDocument, CloseError: "close"}, "", false},
		{"transport_failure_safe", migrationSolverStep{TransportError: "url-cookie-proxy"}, "", false},
		{"transport_canceled_without_caller_cancel", migrationSolverStep{TransportError: "canceled"}, "", false},
		{"transport_deadline_without_caller_deadline", migrationSolverStep{TransportError: "deadline"}, "", false},
		{"caller_canceled_transport_failure", migrationSolverStep{TransportError: "external"}, "", true},
		{"caller_canceled_transport_ignores_context", migrationSolverStep{Status: 200, Body: migrationSolverDocument}, "", true},
	}
	for _, test := range tests {
		recorder := &migrationSolverRecorder{}
		ctx, cancel := migrationSolverContext(recorder)
		session, native, control := migrationSolverNewSession(t, recorder, nil, []migrationSolverStep{test.step}, &FlareSolverrOptions{URL: "http://solver.example:8191/", ProxyURL: test.proxy}, cancel)
		if test.canceled {
			cancel()
		}
		state, err := session.solveFlareSolverr(ctx)
		control.assertOwnedControl(t)
		rows = append(rows, migrationSolverRow{"control/" + test.name, "control_request", map[string]any{"step": test.step, "upstream_proxy": test.proxy, "caller_canceled": test.canceled}, map[string]any{"state": migrationSolverStateOf(state), "error": migrationSolverErrorOf(err), "native": native.view(), "control": control.view(), "session": migrationSolverSessionOf(session), "diagnostics": recorder.view()}})
		cancel()
	}
	return rows
}

func migrationSolverWorkflowRows(t *testing.T) []migrationSolverRow {
	t.Helper()
	rows := []migrationSolverRow{}
	challenge := migrationSolverStep{Status: 403, Header: http.Header{"Cf-Mitigated": {"challenge"}}, Body: "synthetic-challenge"}
	good := migrationSolverStep{Status: 200, Body: `{"body":"replayed"}`}
	solve := migrationSolverStep{Status: 200, Body: migrationSolverDocument}
	tests := []struct {
		name                      string
		native                    []migrationSolverStep
		control                   []migrationSolverStep
		url, method               string
		kind                      requestKind
		include, disabled, cached bool
		calls                     int
	}{
		{name: "challenge_replay", native: []migrationSolverStep{challenge, good, good}, control: []migrationSolverStep{solve}, include: true, calls: 2},
		{name: "cached_clearance_challenge_refresh", native: []migrationSolverStep{challenge, good}, control: []migrationSolverStep{solve}, include: true, cached: true},
		{name: "second_challenge_no_third_request", native: []migrationSolverStep{challenge, challenge}, control: []migrationSolverStep{solve}, include: true},
		{name: "solver_failed", native: []migrationSolverStep{challenge}, control: []migrationSolverStep{{Status: 503}}, include: true},
		{name: "solver_malformed", native: []migrationSolverStep{challenge}, control: []migrationSolverStep{{Status: 200, Body: "{"}}, include: true},
		{name: "solver_unavailable", native: []migrationSolverStep{challenge}, control: []migrationSolverStep{{TransportError: "external"}}, include: true},
		{name: "disabled_solver", native: []migrationSolverStep{challenge}, disabled: true, include: true},
		{name: "ordinary_json_challenge_word", native: []migrationSolverStep{{Status: 403, Body: `{"error":"challenge"}`}}, include: true},
		{name: "split_body_marker", native: []migrationSolverStep{{Status: 403, Body: "before CF_CHL after", Chunk: 1}, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "marker_after_32k", native: []migrationSolverStep{{Status: 403, Body: strings.Repeat("x", 32766) + "cf-chl-tail"}, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "html_cloudflare", native: []migrationSolverStep{{Status: 403, Header: http.Header{"Content-Type": {" Text/HTML; charset=utf-8"}, "Server": {"cloudflare"}}, Body: "html"}, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "html_ray", native: []migrationSolverStep{{Status: 403, Header: http.Header{"Content-Type": {"text/html"}, "Cf-Ray": {"synthetic-ray"}}, Body: "html"}, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "non_html_cloudflare_forbidden", native: []migrationSolverStep{{Status: 403, Header: http.Header{"Content-Type": {"application/json"}, "Server": {"cloudflare"}}, Body: "{}"}}, include: true},
		{name: "multi_header_substring", native: []migrationSolverStep{{Status: 403, Header: http.Header{"Cf-Mitigated": {"none", "preCHALLENGEpost"}}, Body: "body"}, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "challenge_read_error_prevents_solve", native: []migrationSolverStep{{Status: 403, Body: "cf-chl", ReadError: "read"}}, include: true},
		{name: "challenge_close_error_prevents_solve", native: []migrationSolverStep{{Status: 403, Body: "cf-chl", ReadError: "read", CloseError: "close"}}, include: true},
		{name: "unauthorized_does_not_scan_marker", native: []migrationSolverStep{{Status: 401, Body: "cf-chl"}}, include: true},
		{name: "status_503_does_not_scan_marker", native: []migrationSolverStep{{Status: 503, Body: "cf-chl"}}, include: true},
		{name: "creator_cookie_scope", native: []migrationSolverStep{challenge, good}, control: []migrationSolverStep{solve}, url: "https://creator.fanbox.cc/private?post=synthetic-business", include: true},
		{name: "downloads_cookie_scope", native: []migrationSolverStep{challenge, good}, control: []migrationSolverStep{solve}, url: "https://downloads.fanbox.cc/file?signature=synthetic", kind: requestKindMedia, method: " head ", include: true},
		{name: "cdn_cookie_scope", native: []migrationSolverStep{challenge, good}, control: []migrationSolverStep{solve}, url: "https://i.pximg.net/file", kind: requestKindMedia, include: true},
		{name: "explicit_no_cookie", native: []migrationSolverStep{challenge, good}, control: []migrationSolverStep{solve}},
		{name: "redirect_before_challenge_restarts_original", native: []migrationSolverStep{{Status: 302, Header: http.Header{"Location": {"https://api.fanbox.cc/redirected"}}}, challenge, good}, control: []migrationSolverStep{solve}, include: true},
		{name: "replay_redirect_strips_cookie", native: []migrationSolverStep{challenge, {Status: 302, Header: http.Header{"Location": {"/redirected"}}}, good}, control: []migrationSolverStep{solve}, include: true},
	}
	for _, test := range tests {
		recorder := &migrationSolverRecorder{}
		ctx, cancel := migrationSolverContext(recorder)
		var options *FlareSolverrOptions
		if !test.disabled {
			options = &FlareSolverrOptions{URL: "http://solver.example:8191", ProxyURL: "http://upstream-proxy.example:8080"}
		}
		session, native, control := migrationSolverNewSession(t, recorder, test.native, test.control, options, cancel)
		if test.cached {
			session.solverState = &solverState{userAgent: "synthetic-stale-agent", clearance: "synthetic-stale-clearance", expiresAt: time.Unix(4102444800, 0), hasExpiry: true}
		}
		if test.url == "" {
			test.url = "https://api.fanbox.cc/post.info?postId=synthetic-business"
		}
		if test.method == "" {
			test.method = "GET"
		}
		if test.calls == 0 {
			test.calls = 1
		}
		before := migrationSolverSessionOf(session)
		results := []any{}
		for i := 0; i < test.calls; i++ {
			response, err := session.doWithRequest(ctx, test.url, test.kind, test.include, "application/json, text/plain, */*", test.method, http.Header{"Range": {"bytes=1-9"}, "If-None-Match": {"synthetic-etag"}})
			result := map[string]any{"error": migrationSolverErrorOf(err), "response_present": response != nil, "session": migrationSolverSessionOf(session), "native_before_caller_close": native.view()}
			if response != nil {
				data, readErr := io.ReadAll(response.Body)
				closeErr := response.Body.Close()
				result["status"] = response.StatusCode
				result["body"] = string(data)
				result["caller_read_error"] = migrationSolverErrorOf(readErr)
				result["caller_close_error"] = migrationSolverErrorOf(closeErr)
			}
			results = append(results, result)
		}
		control.assertOwnedControl(t)
		rows = append(rows, migrationSolverRow{"workflow/" + test.name, "challenge_replay", map[string]any{"native_steps": test.native, "control_steps": test.control, "url": test.url, "method": test.method, "go_only_request_kind": test.kind, "include_cookie": test.include, "solver_disabled": test.disabled, "initial_cached_state": before, "calls": test.calls}, map[string]any{"results": results, "native": native.view(), "control": control.view(), "session": migrationSolverSessionOf(session), "diagnostics": recorder.view()}})
		cancel()
	}
	return rows
}

func migrationSolverAwait(t *testing.T, label string, ready func() bool) {
	t.Helper()
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()
	for !ready() {
		select {
		case <-timer.C:
			t.Fatalf("bounded barrier expired: %s", label)
		default:
			runtime.Gosched()
		}
	}
}

func migrationSolverReceive[T any](t *testing.T, label string, ch <-chan T) T {
	t.Helper()
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()
	select {
	case value := <-ch:
		return value
	case <-timer.C:
		t.Fatalf("bounded receive expired: %s", label)
		var zero T
		return zero
	}
}

type migrationSolverOutcome struct {
	State solverState
	Err   error
}

func migrationSolverOutcomeOf(outcome migrationSolverOutcome) any {
	return map[string]any{"state": migrationSolverStateOf(outcome.State), "error": migrationSolverErrorOf(outcome.Err)}
}

func migrationSolverStartWaiter(session *Session, ctx context.Context) <-chan migrationSolverOutcome {
	result := make(chan migrationSolverOutcome, 1)
	go func() { state, err := session.waitForSolver(ctx); result <- migrationSolverOutcome{state, err} }()
	return result
}

func migrationSolverActiveAt(t *testing.T, session *Session, count int) *solverCall {
	t.Helper()
	var call *solverCall
	migrationSolverAwait(t, "registered solver waiters", func() bool {
		session.solverMu.Lock()
		defer session.solverMu.Unlock()
		call = session.solverActive
		return call != nil && call.waiters == count
	})
	return call
}

func migrationSolverConcurrentRows(t *testing.T) []migrationSolverRow {
	t.Helper()
	rows := []migrationSolverRow{}
	for _, mode := range []string{"singleflight_success", "singleflight_failure", "one_waiter_cancels", "all_waiters_cancel", "all_cancel_transport_ignores_cancel", "replacement_discards_stale_completion"} {
		recorder := &migrationSolverRecorder{}
		ctx, cancel := migrationSolverContext(recorder)
		deadlineCtx, deadlineCancel := context.WithDeadline(ctx, time.Unix(4102444800, 0))
		defer deadlineCancel()
		firstCtx, firstCancel := context.WithCancel(deadlineCtx)
		secondBase := context.WithValue(ctx, migrationSolverContextKey{}, "synthetic-second-context-value")
		secondBase = diagnostics.WithScope(secondBase, recorder, diagnostics.ModuleFanboxCLI, 91)
		secondCtx, secondCancel := context.WithCancel(secondBase)
		callerContexts := map[string]any{"first": migrationSolverContextOf(firstCtx), "second": migrationSolverContextOf(secondCtx)}
		defer firstCancel()
		defer secondCancel()
		defer cancel()
		step := migrationSolverStep{Status: 200, Body: migrationSolverDocument}
		if mode == "singleflight_failure" {
			step.Status = 503
		}
		if mode == "all_waiters_cancel" {
			step.TransportError = "canceled"
		}
		session, native, control := migrationSolverNewSession(t, recorder, nil, []migrationSolverStep{step, step}, &FlareSolverrOptions{URL: "http://solver.example:8191"}, cancel)
		entered := make(chan context.Context, 2)
		release := make(chan struct{})
		replacementRelease := make(chan struct{})
		var releaseOnce, replacementOnce sync.Once
		defer releaseOnce.Do(func() { close(release) })
		defer replacementOnce.Do(func() { close(replacementRelease) })
		var blockMu sync.Mutex
		blockCalls := 0
		control.block = func(request *http.Request) {
			blockMu.Lock()
			blockCalls++
			count := blockCalls
			blockMu.Unlock()
			entered <- request.Context()
			if mode == "replacement_discards_stale_completion" && count == 2 {
				migrationSolverReceive(t, "owned replacement transport release", replacementRelease)
				return
			}
			if mode == "all_waiters_cancel" {
				migrationSolverReceive(t, "owned transport sees shared cancellation", request.Context().Done())
			}
			migrationSolverReceive(t, "owned original transport release", release)
		}
		first := migrationSolverStartWaiter(session, firstCtx)
		callContext := migrationSolverReceive(t, "first control entered", entered)
		call := migrationSolverActiveAt(t, session, 1)
		second := migrationSolverStartWaiter(session, secondCtx)
		migrationSolverActiveAt(t, session, 2)
		_, callerDeadline := firstCtx.Deadline()
		stages := []any{map[string]any{"stage": "two_registered", "session": migrationSolverSessionOf(session), "control_context": migrationSolverContextOf(callContext), "go_only_caller_deadline_present": callerDeadline}}
		outcomes := map[string]any{}
		if mode == "one_waiter_cancels" || strings.HasPrefix(mode, "all_") || mode == "replacement_discards_stale_completion" {
			firstCancel()
			outcomes["first"] = migrationSolverOutcomeOf(migrationSolverReceive(t, "first canceled waiter", first))
			stages = append(stages, map[string]any{"stage": "one_unregistered", "session": migrationSolverSessionOf(session), "control_context": migrationSolverContextOf(callContext)})
		}
		if strings.HasPrefix(mode, "all_") || mode == "replacement_discards_stale_completion" {
			secondCancel()
			outcomes["second"] = migrationSolverOutcomeOf(migrationSolverReceive(t, "second canceled waiter", second))
			stages = append(stages, map[string]any{"stage": "all_unregistered", "session": migrationSolverSessionOf(session), "control_context": migrationSolverContextOf(callContext)})
		}
		if mode == "replacement_discards_stale_completion" {
			thirdCtx := context.WithValue(ctx, migrationSolverContextKey{}, "synthetic-third-context-value")
			thirdCtx = diagnostics.WithScope(thirdCtx, recorder, diagnostics.ModuleFanboxCLI, 101)
			callerContexts["third"] = migrationSolverContextOf(thirdCtx)
			third := migrationSolverStartWaiter(session, thirdCtx)
			migrationSolverReceive(t, "replacement control entered", entered)
			newCall := migrationSolverActiveAt(t, session, 1)
			stages = append(stages, map[string]any{"stage": "replacement_registered", "session": migrationSolverSessionOf(session), "go_only_distinct_call_pointer": newCall != call})
			releaseOnce.Do(func() { close(release) })
			migrationSolverReceive(t, "stale call completed", call.done)
			session.solverMu.Lock()
			oldView := migrationSolverCallOf(call)
			same := session.solverActive == newCall
			session.solverMu.Unlock()
			stages = append(stages, map[string]any{"stage": "stale_discarded", "old_call": oldView, "go_only_replacement_still_active": same, "session": migrationSolverSessionOf(session)})
			replacementOnce.Do(func() { close(replacementRelease) })
			outcomes["third"] = migrationSolverOutcomeOf(migrationSolverReceive(t, "replacement waiter", third))
			migrationSolverReceive(t, "replacement completed", newCall.done)
		} else {
			releaseOnce.Do(func() { close(release) })
			if mode == "singleflight_success" || mode == "singleflight_failure" {
				outcomes["first"] = migrationSolverOutcomeOf(migrationSolverReceive(t, "first waiter", first))
			}
			if mode == "singleflight_success" || mode == "singleflight_failure" || mode == "one_waiter_cancels" {
				outcomes["second"] = migrationSolverOutcomeOf(migrationSolverReceive(t, "second waiter", second))
			}
			migrationSolverReceive(t, "call completed", call.done)
		}
		session.solverMu.Lock()
		finalCall := migrationSolverCallOf(call)
		session.solverMu.Unlock()
		control.assertOwnedControl(t)
		rows = append(rows, migrationSolverRow{"concurrent/" + mode, "shared_waiters", map[string]any{"mode": mode, "caller_contexts": callerContexts}, map[string]any{"stages": stages, "outcomes": outcomes, "completed_original_call": finalCall, "session": migrationSolverSessionOf(session), "native": native.view(), "control": control.view(), "diagnostics": recorder.view()}})
	}
	return rows
}

func migrationSolverCacheRows(t *testing.T) []migrationSolverRow {
	t.Helper()
	rows := []migrationSolverRow{}
	for _, mode := range []string{"absent", "no_expiry", "future", "past", "zero_expiry"} {
		session := &Session{userAgent: "synthetic-native-agent"}
		state := solverState{userAgent: "synthetic-solver-agent", clearance: "synthetic-clearance"}
		switch mode {
		case "no_expiry":
			session.solverState = &state
		case "future":
			state.hasExpiry = true
			state.expiresAt = time.Unix(4102444800, 0)
			session.solverState = &state
		case "past":
			state.hasExpiry = true
			state.expiresAt = time.Unix(1, 0)
			session.solverState = &state
		case "zero_expiry":
			state.hasExpiry = true
			session.solverState = &state
		}
		before := migrationSolverSessionOf(session)
		agent, clearance := session.nativeState()
		after := migrationSolverSessionOf(session)
		session.invalidateSolverState()
		rows = append(rows, migrationSolverRow{"cache/native_" + mode, "cache_lifecycle", before, map[string]any{"agent": agent, "clearance": clearance, "after_native_state": after, "after_invalidate": migrationSolverSessionOf(session)}})
	}
	var nilSession *Session
	agent, clearance := nilSession.nativeState()
	nilSession.invalidateSolverState()
	state, err := nilSession.waitForSolver(context.Background())
	rows = append(rows, migrationSolverRow{"cache/nil_session", "go_only_nil_receiver", nil, map[string]any{"agent": agent, "clearance": clearance, "state": migrationSolverStateOf(state), "error": migrationSolverErrorOf(err)}})
	for _, mode := range []string{"disabled_before_cancel", "configured_canceled_before_cache", "cache_value_copy", "expired_runs_new_solve"} {
		recorder := &migrationSolverRecorder{}
		ctx, cancel := migrationSolverContext(recorder)
		options := &FlareSolverrOptions{URL: "http://solver.example"}
		if mode == "disabled_before_cancel" {
			options = nil
		}
		session, native, control := migrationSolverNewSession(t, recorder, nil, []migrationSolverStep{{Status: 200, Body: migrationSolverDocument}}, options, cancel)
		session.solverState = &solverState{userAgent: "cached-agent", clearance: "cached-clearance", expiresAt: time.Unix(4102444800, 0), hasExpiry: true}
		if mode == "disabled_before_cancel" || mode == "configured_canceled_before_cache" {
			cancel()
		}
		if mode == "expired_runs_new_solve" {
			session.solverState.expiresAt = time.Unix(1, 0)
		}
		before := migrationSolverSessionOf(session)
		state, err := session.waitForSolver(ctx)
		control.assertOwnedControl(t)
		observed := migrationSolverStateOf(state)
		state.clearance = "caller-mutated-copy"
		rows = append(rows, migrationSolverRow{"cache/" + mode, "cache_wait", before, map[string]any{"state": observed, "error": migrationSolverErrorOf(err), "session_after_caller_mutates_return": migrationSolverSessionOf(session), "native": native.view(), "control": control.view(), "diagnostics": recorder.view()}})
		cancel()
	}
	for _, mode := range []string{"wrong_call_pointer", "already_zero", "one_of_two", "last_waiter"} {
		ctx, cancel := context.WithCancel(context.Background())
		call := &solverCall{ctx: ctx, cancel: cancel, waiters: 2, done: make(chan struct{})}
		session := &Session{solverActive: call}
		argument := call
		switch mode {
		case "wrong_call_pointer":
			argument = &solverCall{}
		case "already_zero":
			call.waiters = 0
		case "last_waiter":
			call.waiters = 1
		}
		before := migrationSolverSessionOf(session)
		session.unregisterSolverWaiter(argument)
		rows = append(rows, migrationSolverRow{"unregister/" + mode, "go_only_waiter_identity", before, map[string]any{"after": migrationSolverSessionOf(session), "go_only_argument_is_active_pointer": argument == call}})
		cancel()
	}
	return rows
}

func migrationSolverGuard(t *testing.T, root string) (map[string]string, map[string]string, []map[string]string) {
	t.Helper()
	sources := map[string]string{"internal/services/fanbox/protocol/solver.go": "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea", "internal/services/fanbox/protocol/protocol.go": "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3", "internal/services/fanbox/protocol/cookie.go": "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9", "internal/shared/diagnostics/diagnostics.go": "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76", "go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c", "go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e"}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("pinned Go1.27.1 required: %s", runtime.Version())
	}
	for path, want := range sources {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		frozen, err := exec.Command("git", "-C", root, "show", "4b4426487ef18bed276706daec385e0d0a6979f9:"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want || !bytes.Equal(data, frozen) {
			t.Fatalf("frozen source changed: %s", path)
		}
	}
	stdlib := map[string]string{"net/http/client.go": "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9", "net/http/request.go": "c3257079994b4e4f74f01cef72508919983ba31f91fcf599d348e9d52cec1540", "net/http/response.go": "0b32a0b4ee51e00e3410764f3b2ffe1163dccb9cad548b097f47db85893a2f44", "context/context.go": "971f00ffa375b79d3f65e6da33cca3a493cc3fce091c589233abfb4283e29b7d", "encoding/json/decode.go": "1632161a34c8286722716a48ba0b5e0c3d117a2e017e79ace676403808a16e6e", "time/time.go": "7d992810b162f65e02cea4d60fd0d90af59b3cafa2c8926ca08083d5c73451dc"}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("official Go source changed: %s", path)
		}
	}
	dependencies := []map[string]string{{"module": "github.com/bogdanfinn/fhttp", "version": "v0.6.8", "zip_sha256": "51f74cb0f96633810038f94773bdbf0356c98a23d9de9eada3008521ae97f3cb", "sum": "h1:LiQyHOY3i0QoxxNB7nq27/nGNNbtPj0fuBPozhR7Ws4="}, {"module": "github.com/bogdanfinn/tls-client", "version": "v1.15.1", "zip_sha256": "01aa0c1f09bc29397b97090a9fb03677ce22491f8a1cdd0a0dc7cdbce16cf031", "sum": "h1:KiFAlED55DJ8Fcocn+/1nX6PrDFcttIHAf/GDkV6KN8="}, {"module": "github.com/bogdanfinn/utls", "version": "v1.7.7-barnius", "zip_sha256": "6e21ca668a463b41d3a4f92449e69872dbb0056b1d41b14b760e1237894203ef", "sum": "h1:OuJ497cc7F3yKNVHRsYPQdGggmk5x6+V5ZlrCR7fOLU="}, {"module": "golang.org/x/net", "version": "v0.48.0", "zip_sha256": "cf5206797e66bbe72fc13542d53a57d069a563cebb6d045c07a870eb4fd888c9", "sum": "h1:zyQRTTrjc33Lhh0fBgT/H3oZq9WuvRR5gPC70xpDiQU="}}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("verified offline GOMODCACHE required")
	}
	sum, err := os.ReadFile(filepath.Join(root, "go.sum"))
	if err != nil {
		t.Fatal(err)
	}
	for _, dep := range dependencies {
		module, version := dep["module"], dep["version"]
		data, err := os.ReadFile(filepath.Join(cache, "cache", "download", module, "@v", version+".zip"))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != dep["zip_sha256"] || !bytes.Contains(sum, []byte(module+" "+version+" "+dep["sum"]+"\n")) {
			t.Fatalf("official dependency changed: %s", module)
		}
		archive, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
		if err != nil {
			t.Fatal(err)
		}
		for _, file := range archive.File {
			if file.FileInfo().IsDir() {
				continue
			}
			reader, err := file.Open()
			if err != nil {
				t.Fatal(err)
			}
			original, readErr := io.ReadAll(reader)
			closeErr := reader.Close()
			if readErr != nil || closeErr != nil {
				t.Fatalf("official archive read failed: %v / %v", readErr, closeErr)
			}
			extracted, err := os.ReadFile(filepath.Join(cache, filepath.FromSlash(file.Name)))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(original, extracted) {
				t.Fatalf("official extracted dependency changed: %s", file.Name)
			}
		}
	}
	return sources, stdlib, dependencies
}

func TestMigrationFanboxSolverCoreWorkflowFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..")
	sources, stdlib, dependencies := migrationSolverGuard(t, root)
	rows := migrationSolverValidationRows(t)
	rows = append(rows, migrationSolverControlRows(t)...)
	rows = append(rows, migrationSolverWorkflowRows(t)...)
	rows = append(rows, migrationSolverConcurrentRows(t)...)
	rows = append(rows, migrationSolverCacheRows(t)...)
	seen := map[string]bool{}
	families := map[string]int{}
	for _, row := range rows {
		if seen[row.Name] {
			t.Fatalf("duplicate contract row: %s", row.Name)
		}
		seen[row.Name] = true
		families[row.Family]++
		t.Logf("frozen observation %s", row.Name)
	}
	zone, offset := time.Unix(0, 0).Zone()
	fixture := map[string]any{"go_only_execution_timezone": map[string]any{"tz_environment": os.Getenv("TZ"), "local_location": time.Local.String(), "unix_epoch_zone": zone, "unix_epoch_offset_seconds": offset}, "source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "go_version": runtime.Version(), "source_sha256": sources, "go_stdlib_sha256": stdlib, "official_dependencies": dependencies, "families": families, "cases": rows, "reused_contracts": []string{"fanbox-identity-protocol.json: constructors/options/identity/public SDK mapping and protocol boundaries", "fanbox-solver-redirect.json: three actual Client.Do malformed Location/no-follow body ownership rows"}, "evidence": "Actual frozen Go validation, control Client.Do, native doWithRequest challenge replay, solver cache and shared waitForSolver/runSolver. Owned in-memory RoundTrippers and tracked bodies execute source dependencies. Bounded channel barriers and mutex-protected waiter observations establish cancellation and replacement ownership; no fixed sleeps.", "go_only_boundaries": []string{"All private solverState fields and solverCall context/cancel/waiters/done/state/error remain named typed observations", "Time values preserve String, RFC3339Nano, Unix, nanosecond, zero and location; execution timezone is explicitly observed and no clock normalization occurs", "Pointer identity is captured by explicit equality relations, never unstable pointer addresses", "Body Read call/trace boundaries and concrete Go error/context types are Go-only observations", "Concurrent registration stages are observed under solverMu; request/diagnostic ordering is source-driven, caller result order is keyed by identity"}, "limitations": []string{"No real network, account, browser, native TLS/HTTP2, media delivery, OS registration or host trust is exercised", "Injected owned transport proves logical request/body ownership; physical native stream cancellation, concurrent native Read/Close and remaining fingerprint/decompression/lifecycle debts stay separate", "Default uninjected control transport Proxy:nil is source-guarded, not network-executed; control and native injected clients prove independent request and proxy/cookie data scope", "Nil caller context panic remains a Go-only unexecuted boundary; no public Rust test API or normalization is introduced", "This fresh inventory does not reuse lost fixture bytes or historical solver113 count, and is not Rust parity or platform verification"}}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver.json")
	if *migrationCaptureSolver {
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
		t.Fatal("actual solver workflow differs from frozen Go contract")
	}
}
