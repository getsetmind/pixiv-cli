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
	"runtime"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureMediaSolver = flag.Bool("migration-capture-fanbox-media-solver", false, "capture frozen public SDK media challenge recovery")

type migrationMediaSolverInput struct {
	Resource    migrationResourceInput `json:"resource"`
	SolverSteps []migrationFanboxStep  `json:"solver_steps"`
	SolverProxy string                 `json:"solver_proxy"`
}
type migrationMediaSolverRow struct {
	Name        string                    `json:"name"`
	Input       migrationMediaSolverInput `json:"input"`
	Observation map[string]any            `json:"observation"`
}
type migrationMediaSolverTransport struct {
	owned  *migrationResourceTransport
	record func(string)
}

func (r *migrationMediaSolverTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	r.record("resource:" + request.URL.String())
	response, err := r.owned.RoundTrip(request)
	if response != nil && response.Body != nil {
		source := response.Body.(*migrationResourceBody)
		response.Body = &migrationMediaSolverBody{ReadCloser: response.Body, name: source.name, record: r.record}
	}
	return response, err
}
func (r *migrationMediaSolverTransport) CloseIdleConnections() { r.owned.CloseIdleConnections() }

type migrationMediaSolverBody struct {
	io.ReadCloser
	name   string
	record func(string)
}

func (b *migrationMediaSolverBody) Close() error {
	b.record("close:" + b.name)
	return b.ReadCloser.Close()
}
func migrationMediaSolverObserve(t *testing.T, scenario migrationMediaSolverInput) map[string]any {
	input := scenario.Resource
	t.Helper()
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationResourceContextKey{}, "resource-context"))
	defer cancel()
	var stopDeadline context.CancelFunc = func() {}
	producerTransport := &migrationResourceTransport{t: t, role: "producer", document: input.ProducerDocument, input: input, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: cancel}
	consumerTransport := &migrationResourceTransport{t: t, role: "consumer", document: input.ReopenDocument, input: input, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: cancel}
	var traceMu sync.Mutex
	trace := []string{}
	record := func(event string) { traceMu.Lock(); defer traceMu.Unlock(); trace = append(trace, event) }
	control := &migrationPublicSolverControl{t: t, steps: scenario.SolverSteps, requests: []map[string]any{}}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		record("solver_control:" + r.Method + " " + r.URL.RequestURI())
		control.handler(w, r)
	}))
	defer server.Close()
	open := func(transport *migrationResourceTransport) *fanbox.Client {
		client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationResourceSession}, fanbox.Options{HTTPClient: &http.Client{Transport: &migrationMediaSolverTransport{owned: transport, record: record}}, UserAgent: "resource-injected-agent", FlareSolverr: &fanbox.FlareSolverrOptions{URL: server.URL + "/", ProxyURL: scenario.SolverProxy}})
		if err != nil {
			t.Fatal(err)
		}
		return client
	}
	producer := open(producerTransport)
	client := producer
	var consumer *fanbox.Client
	if input.Mode == "fresh" {
		consumer = open(consumerTransport)
		client = consumer
	}
	out := map[string]any{"generated_resources": []map[string]any{}, "generation_error": migrationResourceError(nil), "reference_error": migrationResourceError(nil), "outcomes": []map[string]any{}}
	var ref sdk.ResourceRef
	if input.Kind != "" {
		resource, all, err := migrationResourceGenerate(t, ctx, producer, input.Kind)
		out["generation_error"] = migrationResourceError(err)
		out["generated_resources"] = all
		ref = resource.Ref
		if input.MutateResourceHeader && resource.RequestHeaders != nil {
			resource.RequestHeaders["Cookie"] = migrationResourceSecret
			resource.RequestHeaders["Referer"] = "https://untrusted.invalid/"
		}
		if err != nil {
			goto finished
		}
	} else if input.RefPayload != "" {
		var err error
		ref, err = sdk.NewResourceRef(input.RefProduct, []byte(input.RefPayload))
		out["reference_error"] = migrationResourceError(err)
		if err != nil {
			goto finished
		}
	} else if input.ParseOnly || input.ParseText != "" {
		var err error
		ref, err = sdk.ParseResourceRef(input.ParseText)
		out["reference_error"] = migrationResourceError(err)
		if err != nil || input.ParseOnly {
			goto finished
		}
	}
	out["selected_ref"] = ref.String()
	if input.Context == "canceled" {
		cancel()
	}
	if input.Context == "deadline" {
		ctx, stopDeadline = context.WithDeadline(ctx, time.Unix(1, 0))
		defer stopDeadline()
	}
	{
		outcomes := []map[string]any{}
		repeat := input.Repeat
		if repeat == 0 {
			repeat = 1
		}
		for index := 0; index < repeat; index++ {
			record("action:open")
			response, err := client.OpenResource(ctx, sdk.OpenResourceRequest{Ref: ref, Method: sdk.ResourceMethod(input.Method), Range: input.Range, IfNoneMatch: input.IfNoneMatch, IfModifiedSince: input.IfModifiedSince, IfRange: input.IfRange})
			result := map[string]any{"returned": response != nil, "open_error": migrationResourceError(err), "steps": []map[string]any{}, "ownership_at_return": migrationResourceOwnership(producerTransport, consumerTransport)}
			if response != nil {
				header := response.Header()
				result["response"] = map[string]any{"status": response.StatusCode, "headers": header.Clone(), "content_type": response.ContentType(), "content_length": response.ContentLength(), "content_range": response.ContentRange(), "accept_ranges": response.AcceptRanges(), "etag": response.ETag(), "last_modified": response.LastModified(), "cache_control": response.CacheControl(), "body_non_nil": response.Body != nil}
				if values := header["Content-Type"]; len(values) > 0 {
					values[0] = "mutated-header"
				}
				header.Set("Set-Cookie", migrationResourceSecret)
				result["headers_after_caller_copy_mutation"] = response.Header()
				steps := []map[string]any{}
				for _, action := range input.Actions {
					record("action:" + action.Kind)
					step := map[string]any{"action": action, "data": "", "bytes": 0, "error": migrationResourceError(nil)}
					switch action.Kind {
					case "read":
						p := make([]byte, action.Size)
						n, readErr := response.Body.Read(p)
						step["data_bytes"] = append([]byte{}, p[:n]...)
						step["data_hex"] = fmt.Sprintf("%x", p[:n])
						step["data_sha256"] = fmt.Sprintf("%x", sha256.Sum256(p[:n]))
						if utf8.Valid(p[:n]) {
							step["data"] = string(p[:n])
						} else {
							step["data"] = nil
						}
						step["bytes"] = n
						step["error"] = migrationResourceError(readErr)
					case "close":
						step["error"] = migrationResourceError(response.Body.Close())
					case "cancel":
						cancel()
					case "idle":
						client.CloseIdleConnections()
					default:
						t.Fatalf("unknown resource action %s", action.Kind)
					}
					step["ownership"] = migrationResourceOwnership(producerTransport, consumerTransport)
					steps = append(steps, step)
				}
				result["steps"] = steps
			}
			outcomes = append(outcomes, result)
		}
		out["outcomes"] = outcomes
	}
finished:
	producer.CloseIdleConnections()
	producer.CloseIdleConnections()
	if consumer != nil {
		consumer.CloseIdleConnections()
		consumer.CloseIdleConnections()
	}
	out["requests"] = append(producerTransport.requests, consumerTransport.requests...)
	out["final_ownership"] = migrationResourceOwnership(producerTransport, consumerTransport)
	out["producer_close_idle_calls"] = producerTransport.idleCalls
	out["consumer_close_idle_calls"] = consumerTransport.idleCalls
	out["control_requests"] = control.view()
	traceMu.Lock()
	out["trace"] = append([]string{}, trace...)
	traceMu.Unlock()
	return out
}

func migrationMediaSolverCases(t *testing.T, root string) []migrationMediaSolverRow {
	seed, err := os.ReadFile(filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-public.json"))
	if err != nil {
		t.Fatal(err)
	}
	var frozen struct {
		Cases []struct {
			Name  string `json:"name"`
			Input struct {
				ControlSteps []migrationFanboxStep `json:"control_steps"`
			} `json:"input"`
		} `json:"cases"`
	}
	if err = json.Unmarshal(seed, &frozen); err != nil {
		t.Fatal(err)
	}
	var solution []migrationFanboxStep
	for _, row := range frozen.Cases {
		if row.Name == "solution/valid" {
			solution = row.Input.ControlSteps
		}
	}
	if len(solution) != 1 {
		t.Fatal("frozen valid solution seed unavailable")
	}
	challenge := migrationResourceBodySpec{Status: 403, Header: http.Header{"Cf-Mitigated": {"challenge"}}, Data: "cf-chl owned media challenge"}
	success := migrationResourceBodySpec{Status: 200, Header: http.Header{"Content-Type": {"application/octet-stream"}, "Content-Length": {"5"}}, Data: "media"}
	base := func() migrationResourceInput {
		input := migrationResourceDefault("post_file")
		input.Range = "bytes=1-4"
		input.IfNoneMatch = "\"owned-etag\""
		input.IfModifiedSince = "Wed, 21 Oct 2015 07:28:00 GMT"
		input.IfRange = "\"owned-range\""
		input.Actions = append(input.Actions, migrationResourceAction{Kind: "close"})
		return input
	}
	rows := []migrationMediaSolverRow{}
	add := func(name string, input migrationResourceInput, steps []migrationFanboxStep) {
		rows = append(rows, migrationMediaSolverRow{Name: name, Input: migrationMediaSolverInput{Resource: input, SolverSteps: steps, SolverProxy: "http://browser-proxy.example:8080"}})
	}
	input := base()
	input.Repeat = 2
	input.Media = []migrationResourceBodySpec{challenge, success, success}
	add("solved_conditional_replay_then_cached_clearance", input, solution)
	input = base()
	input.Media = []migrationResourceBodySpec{challenge, {Status: 302, Header: http.Header{"Location": {"https://i.pximg.net/redirect-one"}}, Data: "redirect"}, {Status: 302, Header: http.Header{"Location": {"https://downloads.fanbox.cc/redirect-two"}}, Data: "redirect"}, success}
	add("solved_redirect_then_downloads_keeps_cookie_dropped", input, solution)
	input = base()
	input.Repeat = 2
	input.Media = []migrationResourceBodySpec{challenge, challenge, challenge, success}
	add("second_challenge_invalidates_before_next_reopen", input, append(append([]migrationFanboxStep{}, solution...), solution...))
	input = base()
	challenge.CancelOnClose = true
	input.Media = []migrationResourceBodySpec{challenge}
	add("challenge_close_cancels_before_solver_start", input, []migrationFanboxStep{})
	return rows
}
func TestMigrationFanboxMediaSolverFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	production := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	sources, stdlib, preserved := migrationResourceVerifySources(t, root)
	rows := migrationMediaSolverCases(t, root)
	for index := range rows {
		row := &rows[index]
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationMediaSolverObserve(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	seed, err := os.ReadFile(filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-public.json"))
	if err != nil {
		t.Fatal(err)
	}
	fixture := map[string]any{"source_commit": reference.SourceCommit, "published_base": "f56f27e178dbbbcc1020fabb0a7f60745aae8dfd", "go_version": runtime.Version(), "frozen_go_production_guard": production, "source_sha256": sources, "go_stdlib_sha256": stdlib, "protected_published_sdk_fixtures_sha256": preserved, "solver_seed_sha256": fmt.Sprintf("%x", sha256.Sum256(seed)), "cases": rows,
		"evidence":            "Actual frozen public Client.Post and OpenResource with in-memory API/media transport and owned anonymous ordinary HTTP/1 solver POST. Stable generated ref, controlled conditional GET, one solve/replay, cached clearance, redirected credential removal, second-challenge invalidation, explicit repeated close and cancellation before solve are observed. No fabricated endpoint DTO replaces API decoding.",
		"go_only_projections": []string{"Concrete error-tree types and source read-call topology remain Go-only; complete public error text/classification, request order, byte counts and close order/counts are retained.", "Owned loopback solver protocol/default User-Agent strings are Go-only; endpoint/method/body/content-length and absence of business cookie, URL, origin and referer are retained."},
		"limitations":         []string{"Only injected API/media and owned anonymous HTTP/1 solver; no external media, account, auth/browser, native peer/HEAD/multiplex/upload or system changes.", "No physically concurrent body Read/Close, native compressed-wire, external solver or cross-platform execution is claimed. All sealed existing fixtures and 434 Go production/module paths remain unchanged."}}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-media-solver.json")
	if *migrationCaptureMediaSolver {
		if err = os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("public media solver observation changed")
	}
	t.Logf("captured/replayed %d public media solver rows; fixture=%d bytes SHA256=%x", len(rows), len(data), sha256.Sum256(data))
}
