package fanbox_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxPublicSolver = flag.Bool("migration-capture-fanbox-solver-public", false, "capture frozen Go public FANBOX solver integration")

const migrationPublicSolverDocument = `{"status":"ok","solution":{"userAgent":"synthetic-solver-agent","cookies":[{"name":"cf_clearance","value":"synthetic-clearance"}]}}`

type migrationPublicSolverInput struct {
	Mode        string                `json:"mode"`
	Native      []migrationFanboxStep `json:"native_steps"`
	Control     []migrationFanboxStep `json:"control_steps"`
	Calls       int                   `json:"calls"`
	Disabled    bool                  `json:"solver_disabled"`
	Unavailable bool                  `json:"control_unavailable"`
	Proxy       string                `json:"solver_proxy"`
}
type migrationPublicSolverRow struct {
	Name        string                     `json:"name"`
	Input       migrationPublicSolverInput `json:"input"`
	Observation map[string]any             `json:"observation"`
}
type migrationPublicSolverRecorder struct {
	mu     sync.Mutex
	events []diagnostics.Event
}

func (r *migrationPublicSolverRecorder) Emit(event diagnostics.Event) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, event)
}
func (r *migrationPublicSolverRecorder) view() []diagnostics.Event {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]diagnostics.Event{}, r.events...)
}

type migrationPublicSolverContext struct {
	context.Context
	mu         sync.Mutex
	armed      bool
	once       sync.Once
	registered chan struct{}
}

func (c *migrationPublicSolverContext) Done() <-chan struct{} {
	c.mu.Lock()
	armed := c.armed
	c.mu.Unlock()
	if armed {
		c.once.Do(func() { close(c.registered) })
	}
	return c.Context.Done()
}
func (c *migrationPublicSolverContext) arm() { c.mu.Lock(); c.armed = true; c.mu.Unlock() }

type migrationPublicSolverNative struct {
	mu         sync.Mutex
	t          *testing.T
	steps      []migrationFanboxStep
	requests   []map[string]any
	bodies     []*migrationPublicSolverBody
	concurrent bool
	seen       map[string]int
	idle       int
}
type migrationPublicSolverBody struct {
	mu                   sync.Mutex
	reader               *strings.Reader
	step                 migrationFanboxStep
	ctx                  *migrationPublicSolverContext
	bytes, reads, closes int
}

func (b *migrationPublicSolverBody) Read(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.reads++
	if b.step.Chunk > 0 && len(p) > b.step.Chunk {
		p = p[:b.step.Chunk]
	}
	n, err := b.reader.Read(p)
	b.bytes += n
	if b.step.ReadError != "" && b.reader.Len() == 0 {
		err = migrationFanboxInjectedError(b.step.ReadError)
	}
	return n, err
}
func (b *migrationPublicSolverBody) Close() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.closes++
	if b.ctx != nil && b.step.Status == 403 {
		b.ctx.arm()
	}
	return migrationFanboxInjectedError(b.step.CloseError)
}
func (r *migrationPublicSolverNative) RoundTrip(req *http.Request) (*http.Response, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	label, _ := req.Context().Value(migrationFanboxContextKey{}).(string)
	headers := req.Header.Clone()
	headers.Set("Cookie", migrationFanboxCookieProjection(headers.Get("Cookie")))
	if headers.Get("Cookie") == "" {
		headers.Del("Cookie")
	}
	_, deadline := req.Context().Deadline()
	index := len(r.requests)
	r.requests = append(r.requests, map[string]any{"caller": label, "method": req.Method, "url": req.URL.String(), "headers": headers, "context_canceled": req.Context().Err() != nil, "context_has_deadline": deadline, "request_has_body": req.Body != nil})
	var step migrationFanboxStep
	if r.concurrent {
		if r.seen[label] == 0 {
			step = r.steps[0]
		} else {
			step = r.steps[1]
		}
		r.seen[label]++
	} else {
		if index >= len(r.steps) {
			r.t.Errorf("unexpected native request %d", index)
			return nil, fmt.Errorf("unexpected owned request")
		}
		step = r.steps[index]
	}
	body := &migrationPublicSolverBody{reader: strings.NewReader(step.Body), step: step}
	body.ctx, _ = req.Context().(*migrationPublicSolverContext)
	r.bodies = append(r.bodies, body)
	header := step.Headers.Clone()
	if header == nil {
		header = make(http.Header)
	}
	if step.Location != "" {
		header.Set("Location", step.Location)
	}
	return &http.Response{StatusCode: step.Status, Header: header, Body: body, Request: req, ContentLength: step.ContentLength}, nil
}
func (r *migrationPublicSolverNative) CloseIdleConnections() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.idle++
}
func (r *migrationPublicSolverNative) view() map[string]any {
	r.mu.Lock()
	defer r.mu.Unlock()
	bodies := []map[string]any{}
	for _, body := range r.bodies {
		body.mu.Lock()
		bodies = append(bodies, map[string]any{"bytes_read": body.bytes, "close_calls": body.closes, "go_only_read_calls": body.reads})
		body.mu.Unlock()
	}
	return map[string]any{"requests": append([]map[string]any{}, r.requests...), "bodies": bodies, "idle_calls": r.idle}
}

type migrationPublicSolverControl struct {
	mu       sync.Mutex
	t        *testing.T
	steps    []migrationFanboxStep
	requests []map[string]any
	entered  chan int
	release  []chan struct{}
	canceled chan int
	finished chan int
}

func (r *migrationPublicSolverControl) handler(w http.ResponseWriter, req *http.Request) {
	payload, err := io.ReadAll(req.Body)
	closeErr := req.Body.Close()
	if err != nil || closeErr != nil {
		r.t.Error("owned control request body failed")
		return
	}
	r.mu.Lock()
	index := len(r.requests)
	header := req.Header.Clone()
	request := map[string]any{"method": req.Method, "path": req.URL.RequestURI(), "body": string(payload), "content_length": req.ContentLength, "accept": header.Get("Accept"), "content_type": header.Get("Content-Type"), "cookie": header.Get("Cookie"), "origin": header.Get("Origin"), "referer": header.Get("Referer"), "go_only_protocol": req.Proto, "go_only_default_user_agent": header.Get("User-Agent")}
	r.requests = append(r.requests, request)
	if index >= len(r.steps) {
		r.mu.Unlock()
		r.t.Errorf("unexpected solver request %d", index)
		return
	}
	step := r.steps[index]
	r.mu.Unlock()
	if r.entered != nil {
		r.entered <- index
		select {
		case <-r.release[index]:
		case <-req.Context().Done():
			r.canceled <- index
			<-r.release[index]
		}
	}
	for key, values := range step.Headers {
		for _, value := range values {
			w.Header().Add(key, value)
		}
	}
	if step.Location != "" {
		w.Header().Set("Location", step.Location)
	}
	w.WriteHeader(step.Status)
	_, _ = io.WriteString(w, step.Body)
	if r.finished != nil {
		r.finished <- index
	}
}
func (r *migrationPublicSolverControl) view() []map[string]any {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]map[string]any{}, r.requests...)
}
func migrationPublicSolverReceive[T any](t *testing.T, name string, ch <-chan T) T {
	t.Helper()
	select {
	case result := <-ch:
		return result
	case <-time.After(5 * time.Second):
		t.Fatalf("owned barrier failed: %s", name)
		var zero T
		return zero
	}
}
func migrationPublicSolverOutcome(user fanbox.User, err error) map[string]any {
	return map[string]any{"dto": fanbox.ToUserDTO(user), "error": migrationFanboxPublicHTMLError(err)}
}
func migrationPublicSolverCall(client *fanbox.Client, ctx context.Context) <-chan map[string]any {
	result := make(chan map[string]any, 1)
	go func() {
		user, err := client.CurrentUser(ctx, fanbox.CurrentUserRequest{})
		result <- migrationPublicSolverOutcome(user, err)
	}()
	return result
}
func migrationPublicSolverObserve(t *testing.T, input migrationPublicSolverInput) map[string]any {
	t.Helper()
	native := &migrationPublicSolverNative{t: t, steps: input.Native, requests: []map[string]any{}, bodies: []*migrationPublicSolverBody{}, concurrent: input.Mode != "", seen: map[string]int{}}
	control := &migrationPublicSolverControl{t: t, steps: input.Control, requests: []map[string]any{}}
	server := httptest.NewServer(http.HandlerFunc(control.handler))
	defer server.Close()
	if input.Unavailable {
		server.Close()
	}
	options := fanbox.Options{HTTPClient: &http.Client{Transport: native}, ProxyURL: "http://native-proxy.example:8888", UserAgent: "synthetic-native-agent"}
	if !input.Disabled {
		options.FlareSolverr = &fanbox.FlareSolverrOptions{URL: server.URL + "/", ProxyURL: input.Proxy}
	}
	client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationFanboxSessionValue}, options)
	if err != nil {
		t.Fatal(err)
	}
	recorder := &migrationPublicSolverRecorder{events: []diagnostics.Event{}}
	outcomes := map[string]any{}
	if input.Mode == "" {
		ctx := diagnostics.WithScope(context.WithValue(context.Background(), migrationFanboxContextKey{}, "single"), recorder, diagnostics.ModuleFanboxCLI, 71)
		results := []map[string]any{}
		for i := 0; i < input.Calls; i++ {
			user, err := client.CurrentUser(ctx, fanbox.CurrentUserRequest{})
			results = append(results, migrationPublicSolverOutcome(user, err))
		}
		outcomes["single"] = results
	} else {
		control.entered = make(chan int, 3)
		control.canceled = make(chan int, 3)
		control.finished = make(chan int, 3)
		control.release = []chan struct{}{make(chan struct{}), make(chan struct{})}
		defer func() {
			for _, release := range control.release {
				select {
				case <-release:
				default:
					close(release)
				}
			}
		}()
		firstBase := diagnostics.WithScope(context.WithValue(context.Background(), migrationFanboxContextKey{}, "first"), recorder, diagnostics.ModuleFanboxCLI, 71)
		firstDeadline, stopDeadline := context.WithDeadline(firstBase, time.Unix(4102444800, 0))
		defer stopDeadline()
		firstContext, firstCancel := context.WithCancel(firstDeadline)
		defer firstCancel()
		first := &migrationPublicSolverContext{Context: firstContext, registered: make(chan struct{})}
		secondBase := diagnostics.WithScope(context.WithValue(context.Background(), migrationFanboxContextKey{}, "second"), recorder, diagnostics.ModuleFanboxCLI, 91)
		secondContext, secondCancel := context.WithCancel(secondBase)
		defer secondCancel()
		second := &migrationPublicSolverContext{Context: secondContext, registered: make(chan struct{})}
		a := migrationPublicSolverCall(client, first)
		migrationPublicSolverReceive(t, "first solver entered", control.entered)
		migrationPublicSolverReceive(t, "first registered", first.registered)
		b := migrationPublicSolverCall(client, second)
		migrationPublicSolverReceive(t, "second registered", second.registered)
		if input.Mode == "one_waiter_cancels" || input.Mode == "all_waiters_cancel_replacement" {
			firstCancel()
			outcomes["first"] = migrationPublicSolverReceive(t, "first canceled", a)
		}
		if input.Mode == "all_waiters_cancel_replacement" {
			secondCancel()
			outcomes["second"] = migrationPublicSolverReceive(t, "second canceled", b)
			migrationPublicSolverReceive(t, "control canceled", control.canceled)
			close(control.release[0])
			migrationPublicSolverReceive(t, "canceled handler finished", control.finished)
			third := diagnostics.WithScope(context.WithValue(context.Background(), migrationFanboxContextKey{}, "third"), recorder, diagnostics.ModuleFanboxCLI, 101)
			c := migrationPublicSolverCall(client, third)
			migrationPublicSolverReceive(t, "replacement solver entered", control.entered)
			close(control.release[1])
			outcomes["third"] = migrationPublicSolverReceive(t, "replacement result", c)
			migrationPublicSolverReceive(t, "replacement handler finished", control.finished)
		} else {
			close(control.release[0])
			if input.Mode != "one_waiter_cancels" {
				outcomes["first"] = migrationPublicSolverReceive(t, "first result", a)
			}
			outcomes["second"] = migrationPublicSolverReceive(t, "second result", b)
			migrationPublicSolverReceive(t, "shared handler finished", control.finished)
		}
	}
	client.CloseIdleConnections()
	client.CloseIdleConnections()
	result := map[string]any{"outcomes": outcomes, "native": native.view(), "control_requests": control.view()}
	events := recorder.view()
	if input.Mode != "" {
		groups := map[string][]diagnostics.Event{}
		for _, event := range events {
			key := fmt.Sprint(event.RequestID)
			groups[key] = append(groups[key], event)
		}
		result["events_by_caller_scope"] = groups
	} else {
		result["events"] = events
	}
	for _, request := range control.view() {
		if request["cookie"] != "" || request["origin"] != "" || request["referer"] != "" || request["path"] != "/v1" || request["method"] != "POST" {
			t.Fatal("solver control leaked native scope")
		}
		var payload map[string]any
		if json.Unmarshal([]byte(request["body"].(string)), &payload) != nil || payload["cmd"] != "request.get" || payload["url"] != "https://www.fanbox.cc/" || strings.Contains(request["body"].(string), migrationFanboxSessionValue) || strings.Contains(request["body"].(string), "native-proxy") {
			t.Fatal("solver control changed anonymous homepage contract")
		}
	}
	if input.Mode != "" {
		view := result["native"].(map[string]any)
		groups := map[string][]map[string]any{}
		for _, request := range view["requests"].([]map[string]any) {
			label := request["caller"].(string)
			groups[label] = append(groups[label], request)
		}
		view["requests_by_caller"] = groups
		delete(view, "requests")
		bodies := view["bodies"].([]map[string]any)
		bytes, closes := 0, 0
		for _, body := range bodies {
			bytes += body["bytes_read"].(int)
			closes += body["close_calls"].(int)
		}
		view["body_totals"] = map[string]any{"bytes_read": bytes, "close_calls": closes}
		delete(view, "bodies")
	}
	return result
}
func TestMigrationFanboxSolverPublicFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	protected := map[string]string{"internal/services/fanbox/protocol/migration_solver_test.go": "c9a27c01aafbf271dc7a6b33cab8959a7b8e721b9b3d0edc6d41941d7aae1f56", "crates/pixiv-sdk/tests/fixtures/fanbox-solver.json": "45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382", "internal/services/fanbox/protocol/migration_solver_redirect_test.go": "40a96c8b674c9f5f9185f9b9be49616b6ccd108c9375c76531d54de7f4896da2", "crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json": "6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651"}
	var solver struct {
		Cases []struct {
			Name   string          `json:"name"`
			Family string          `json:"family"`
			Input  json.RawMessage `json:"input"`
		} `json:"cases"`
	}
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("published solver evidence changed: %s", path)
		}
		if strings.HasSuffix(path, "fanbox-solver.json") {
			if json.Unmarshal(data, &solver) != nil {
				t.Fatal("solver fixture invalid")
			}
		}
	}
	var identity string
	for _, row := range reference.Cases {
		if row.Name == "user_agent/00" {
			identity = row.Input.Steps[0].Body
		}
	}
	if identity == "" {
		t.Fatal("reused public identity body missing")
	}
	challenge := migrationFanboxStep{Status: 403, Body: "synthetic-challenge", Headers: http.Header{"Cf-Mitigated": {"challenge"}}}
	success := migrationFanboxStep{Status: 200, Body: identity}
	valid := migrationFanboxStep{Status: 200, Body: migrationPublicSolverDocument}
	rows := []migrationPublicSolverRow{}
	add := func(name string, input migrationPublicSolverInput) {
		if input.Calls == 0 {
			input.Calls = 1
		}
		var result map[string]any
		t.Run(name, func(t *testing.T) { result = migrationPublicSolverObserve(t, input) })
		rows = append(rows, migrationPublicSolverRow{name, input, result})
	}
	controls := map[string]bool{}
	for _, name := range []string{"valid", "upstream_proxy", "status_299", "status_300", "status_500", "empty_body", "invalid_json", "status_case", "missing_status", "null_document", "missing_solution", "wrong_solution_type", "trailing_second_json_ignored", "trailing_garbage_ignored", "duplicate_status_last_wins", "unknown_fields"} {
		controls["control/"+name] = true
	}
	workflows := map[string]bool{}
	for _, name := range []string{"disabled_solver", "ordinary_json_challenge_word", "split_body_marker", "marker_after_32k", "html_cloudflare", "html_ray", "non_html_cloudflare_forbidden", "multi_header_substring", "challenge_read_error_prevents_solve", "challenge_close_error_prevents_solve", "unauthorized_does_not_scan_marker", "status_503_does_not_scan_marker", "second_challenge_no_third_request", "redirect_before_challenge_restarts_original", "replay_redirect_strips_cookie"} {
		workflows["workflow/"+name] = true
	}
	for _, original := range solver.Cases {
		if original.Family == "solution_validation" {
			document := `{"status":"ok","solution":` + string(original.Input) + `}`
			add(original.Name, migrationPublicSolverInput{Native: []migrationFanboxStep{challenge, success, success}, Control: []migrationFanboxStep{{Status: 200, Body: document}}, Calls: 2})
		} else if original.Family == "expiry" {
			var raw string
			if json.Unmarshal(original.Input, &raw) != nil {
				t.Fatal("expiry raw missing")
			}
			document := migrationPublicSolverDocument
			if raw != "" {
				document = `{"status":"ok","solution":{"userAgent":"synthetic-solver-agent","cookies":[{"name":"cf_clearance","value":"synthetic-clearance","expiry":` + raw + `}]}}`
			}
			add(original.Name, migrationPublicSolverInput{Native: []migrationFanboxStep{challenge, success, success}, Control: []migrationFanboxStep{{Status: 200, Body: document}}, Calls: 2})
		} else if controls[original.Name] {
			var input struct {
				Step     migrationFanboxStep `json:"step"`
				Upstream string              `json:"upstream_proxy"`
			}
			if json.Unmarshal(bytes.ReplaceAll(original.Input, []byte(`"header":`), []byte(`"headers":`)), &input) != nil {
				t.Fatal("control input invalid")
			}
			add(original.Name, migrationPublicSolverInput{Native: []migrationFanboxStep{challenge, success}, Control: []migrationFanboxStep{input.Step}, Proxy: input.Upstream})
		} else if original.Name == "cache/native_future" {
			var input struct {
				State struct {
					Expiry struct {
						Unix int64 `json:"unix_seconds"`
					} `json:"private_expires_at"`
				} `json:"private_solver_state"`
			}
			if json.Unmarshal(original.Input, &input) != nil || input.State.Expiry.Unix != 4102444800 {
				t.Fatal("published future cache expiry changed")
			}
			document := fmt.Sprintf(`{"status":"ok","solution":{"userAgent":"synthetic-solver-agent","cookies":[{"name":"cf_clearance","value":"synthetic-clearance","expiry":%d}]}}`, input.State.Expiry.Unix)
			add(original.Name, migrationPublicSolverInput{Native: []migrationFanboxStep{challenge, success, success}, Control: []migrationFanboxStep{{Status: 200, Body: document}}, Calls: 2})
		} else if workflows[original.Name] {
			var input struct {
				Native   []migrationFanboxStep `json:"native_steps"`
				Control  []migrationFanboxStep `json:"control_steps"`
				Disabled bool                  `json:"solver_disabled"`
			}
			if json.Unmarshal(bytes.ReplaceAll(original.Input, []byte(`"header":`), []byte(`"headers":`)), &input) != nil {
				t.Fatal("workflow input invalid")
			}
			for i := range input.Native {
				if input.Native[i].Status == 200 {
					input.Native[i].Body = identity
				}
			}
			add(original.Name, migrationPublicSolverInput{Native: input.Native, Control: input.Control, Disabled: input.Disabled})
		}
	}
	add("control/unavailable", migrationPublicSolverInput{Native: []migrationFanboxStep{challenge}, Unavailable: true})
	add("control/malformed_location", migrationPublicSolverInput{Native: []migrationFanboxStep{challenge}, Control: []migrationFanboxStep{{Status: 302, Location: "https://%"}}})
	add("control/valid_location_no_follow", migrationPublicSolverInput{Native: []migrationFanboxStep{challenge}, Control: []migrationFanboxStep{{Status: 302, Location: "https://solver.example/next"}}})
	add("cache/challenge_refresh", migrationPublicSolverInput{Native: []migrationFanboxStep{challenge, success, challenge, success, success}, Control: []migrationFanboxStep{valid, {Status: 200, Body: strings.ReplaceAll(migrationPublicSolverDocument, "synthetic-clearance", "replacement-clearance")}}, Calls: 3})
	for _, mode := range []string{"singleflight_success", "singleflight_failure", "one_waiter_cancels", "all_waiters_cancel_replacement"} {
		step := valid
		if mode == "singleflight_failure" {
			step.Status = 503
		}
		add("concurrent/"+mode, migrationPublicSolverInput{Mode: mode, Native: []migrationFanboxStep{challenge, success}, Control: []migrationFanboxStep{step, valid}})
	}
	fixture := map[string]any{"source_commit": reference.SourceCommit, "go_version": reference.GoVersion, "source_sha256": reference.SourceSHA256, "published_solver_sha256": protected, "source_fixture_case_count": len(solver.Cases), "public_operation": "fanbox.Client.CurrentUser", "cases": rows, "evidence": "Actual frozen public OpenWith/CurrentUser, genuine owned native/API http.Client transport and configured anonymous HTTP/1 loopback service exercise default direct control, challenge, solve, native replay, cached clearance/expiry, refresh, public error mapping and shared caller cancellation. All payload/expiry/challenge inputs are reused from published solver124/identity309 fixtures; only successful native bodies become the existing public identity document. Custom caller context Done observes shared selection after challenge body Close, with no private session access or fixed sleeps.", "go_only_boundaries": []string{"Original solver124 private state/call/time and redirect3 evidence remains byte-identical and is not projected into guessed public error expectations", "Numeric expiry syntax is preserved as original raw JSON, including fraction/exponent and int64 domain; actual public decoder whitespace/null behavior is captured rather than inferred from private helper rows", "Native body read topology, server default User-Agent/HTTP protocol, and custom context Done registration are Go-only; DTO/error/source, request payload/header scope, per-caller events, bytes read and Close counts are semantic", "Concurrent observations are grouped by caller scope and request identity, retaining within-caller order without asserting scheduler order"}, "limitations": []string{"Only owned anonymous HTTP/1 loopback control and injected native/API transport are executed; no native HTTP/2, external authentication, media, HEAD, upload, browser, account or host trust work", "Internal SolverHTTPClient remains solely the existing Go test seam; no public control transport or private Session/Solver test API is introduced", "Loopback validates ordinary direct control and independence from native proxy; environment proxy bypass remains source-guarded because Go also exempts loopback hosts from environment proxy", "Physical solver control response Close errors, ignored native transport cancellation, and stale transport-ignoring completion remain in unchanged private/source evidence, not claimed as public loopback parity"}}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-public.json")
	if *migrationCaptureFanboxPublicSolver {
		if os.WriteFile(path, data, 0600) != nil {
			t.Fatal("fixture write failed")
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public solver observation differs from actual frozen Go fixture")
	}
	t.Logf("public solver captured %d cases, bytes=%d sha256=%x", len(rows), len(data), sha256.Sum256(data))
}
