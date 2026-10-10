package fanbox_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxSolverStreamCompletion = flag.Bool("migration-capture-fanbox-solver-stream-completion", false, "capture frozen Go public solver completion before response release")

type migrationSolverStreamBytes struct {
	Hex    string `json:"hex"`
	Length int    `json:"length"`
	SHA256 string `json:"sha256"`
}

type migrationSolverStreamInput struct {
	Native            []migrationFanboxStep      `json:"native_steps"`
	Prefix            migrationSolverStreamBytes `json:"flushed_prefix"`
	Remainder         migrationSolverStreamBytes `json:"held_remainder"`
	PrefixGlobalDepth int                        `json:"prefix_global_depth"`
}

type migrationSolverStreamRow struct {
	Name        string                     `json:"name"`
	Input       migrationSolverStreamInput `json:"input"`
	Observation map[string]any             `json:"observation"`
}

func migrationSolverStreamEncode(raw []byte) migrationSolverStreamBytes {
	return migrationSolverStreamBytes{Hex: hex.EncodeToString(raw), Length: len(raw), SHA256: fmt.Sprintf("%x", sha256.Sum256(raw))}
}

func migrationSolverStreamDecode(t *testing.T, descriptor migrationSolverStreamBytes) []byte {
	t.Helper()
	raw, err := hex.DecodeString(descriptor.Hex)
	if err != nil || migrationSolverStreamEncode(raw) != descriptor {
		t.Fatal("stream response descriptor is not canonical exact-byte evidence")
	}
	return raw
}

type migrationSolverStreamFlush struct {
	Bytes int
	Error error
}

type migrationSolverStreamControl struct {
	mu        sync.Mutex
	prefix    []byte
	remainder []byte
	requests  []map[string]any
	flushed   chan migrationSolverStreamFlush
	release   chan struct{}
	finished  chan struct{}
	once      sync.Once
}

func (c *migrationSolverStreamControl) releaseHeld() {
	c.once.Do(func() { close(c.release) })
}

func (c *migrationSolverStreamControl) handler(w http.ResponseWriter, req *http.Request) {
	defer close(c.finished)
	payload, err := io.ReadAll(req.Body)
	closeErr := req.Body.Close()
	if err != nil || closeErr != nil {
		c.flushed <- migrationSolverStreamFlush{Error: fmt.Errorf("owned request body failed")}
		return
	}
	header := req.Header.Clone()
	c.mu.Lock()
	c.requests = append(c.requests, map[string]any{
		"method": req.Method, "path": req.URL.RequestURI(), "body": string(payload), "content_length": req.ContentLength,
		"accept": header.Get("Accept"), "content_type": header.Get("Content-Type"), "cookie": header.Get("Cookie"),
		"origin": header.Get("Origin"), "referer": header.Get("Referer"), "go_only_protocol": req.Proto,
		"go_only_default_user_agent": header.Get("User-Agent"),
	})
	c.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	n, writeErr := w.Write(c.prefix)
	if writeErr == nil {
		writeErr = http.NewResponseController(w).Flush()
	}
	c.flushed <- migrationSolverStreamFlush{Bytes: n, Error: writeErr}
	// Client cancellation must not release the withheld remainder or EOF.
	<-c.release
	_, _ = w.Write(c.remainder)
}

func (c *migrationSolverStreamControl) view() []map[string]any {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]map[string]any{}, c.requests...)
}

func migrationSolverStreamObserve(t *testing.T, input migrationSolverStreamInput) map[string]any {
	t.Helper()
	prefix := migrationSolverStreamDecode(t, input.Prefix)
	remainder := migrationSolverStreamDecode(t, input.Remainder)
	if len(prefix)+len(remainder) > 128*1024 || migrationFanboxJSONBytesDepth(prefix) != input.PrefixGlobalDepth {
		t.Fatal("bounded response or global prefix depth differs")
	}
	native := &migrationPublicSolverNative{t: t, steps: input.Native, requests: []map[string]any{}, bodies: []*migrationPublicSolverBody{}, seen: map[string]int{}}
	control := &migrationSolverStreamControl{
		prefix: prefix, remainder: remainder, requests: []map[string]any{},
		flushed: make(chan migrationSolverStreamFlush, 1), release: make(chan struct{}), finished: make(chan struct{}),
	}
	server := httptest.NewServer(http.HandlerFunc(control.handler))
	defer func() {
		control.releaseHeld()
		server.CloseClientConnections()
		server.Close()
	}()
	client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationFanboxSessionValue}, fanbox.Options{
		HTTPClient: &http.Client{Transport: native}, ProxyURL: "http://native-proxy.example:8888", UserAgent: "synthetic-native-agent",
		FlareSolverr: &fanbox.FlareSolverrOptions{URL: server.URL + "/"},
	})
	if err != nil {
		t.Fatal(err)
	}
	recorder := &migrationPublicSolverRecorder{events: []diagnostics.Event{}}
	base := diagnostics.WithScope(context.WithValue(context.Background(), migrationFanboxContextKey{}, "single"), recorder, diagnostics.ModuleFanboxCLI, 71)
	ctx, cancel := context.WithCancel(base)
	defer cancel()
	guard := time.NewTimer(5 * time.Second)
	defer guard.Stop()
	result := migrationPublicSolverCall(client, ctx)
	consumed := false
	defer func() {
		if !consumed {
			control.releaseHeld()
			cancel()
			server.CloseClientConnections()
			select {
			case <-result:
			case <-time.After(5 * time.Second):
				t.Error("public caller did not terminate during failure cleanup")
			}
		}
	}()
	var flush migrationSolverStreamFlush
	select {
	case flush = <-control.flushed:
	case <-guard.C:
		t.Fatal("five-second failure guard: owned prefix was not flushed")
	}
	if flush.Error != nil || flush.Bytes != len(prefix) {
		t.Fatal("owned solver prefix flush failed")
	}
	var outcome map[string]any
	select {
	case outcome = <-result:
		consumed = true
	case <-guard.C:
		t.Fatal("five-second failure guard: CurrentUser waited for held remainder or EOF")
	}
	select {
	case <-control.release:
		t.Fatal("remainder released before public completion")
	default:
	}
	select {
	case <-control.finished:
		t.Fatal("response EOF preceded public completion")
	default:
	}
	client.CloseIdleConnections()
	client.CloseIdleConnections()
	requests := control.view()
	if len(requests) != 1 {
		t.Fatal("stream completion changed single control request contract")
	}
	request := requests[0]
	if request["cookie"] != "" || request["origin"] != "" || request["referer"] != "" || request["path"] != "/v1" || request["method"] != "POST" || request["go_only_protocol"] != "HTTP/1.1" {
		t.Fatal("stream control leaked native scope or changed ordinary HTTP/1 POST")
	}
	if request["body"] != `{"cmd":"request.get","url":"https://www.fanbox.cc/"}` || request["accept"] != "application/json" || request["content_type"] != "application/json" {
		t.Fatal("stream control changed anonymous homepage payload")
	}
	return map[string]any{
		"outcomes": map[string]any{"single": []map[string]any{outcome}}, "native": native.view(), "events": recorder.view(), "control_requests": requests,
		"stream_completion": map[string]any{
			"prefix_flushed": true, "prefix_bytes_written": flush.Bytes, "return_before_release": true,
			"remainder_released_before_return": false, "handler_finished_before_return": false,
		},
	}
}

func TestMigrationFanboxSolverStreamCompletionFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	frozenProduction := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	protected := map[string]string{
		"sdk/fanbox/migration_identity_json_bytes_test.go":                "2d2d92da866945ce995c880e1e5490e7798916e1ef19fcccf1916abe831932db",
		"sdk/fanbox/migration_solver_json_dates_test.go":                  "eab317ca10b81c7b3003e8e614b5e640a6e1d778cdeb3b07d380ef4fca569ee1",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-json-bytes.json": "8baf77f9538cb08d5a5b6bc0766e06b49dd8b3ef7f88f192027e7d58c9bdeb60",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-json-dates.json":   "6793ce0d58bca871bb3b081808ebdff1aa6af87f06404110107dfebfc0f9572e",
		"sdk/fanbox/migration_solver_public_test.go":                      "a21a5a150c8a644d7874f5e94cbcf78716742279048a41f680d1ede0a5d6fa34",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public.json":       "a908d47882462e698b67d80f2f48cfab578964e675029afe973a8a024711ba80",
	}
	var original struct {
		Cases []migrationPublicSolverRow `json:"cases"`
	}
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("sealed public evidence changed: %s", path)
		}
		if strings.HasSuffix(path, "/fanbox-solver-public.json") {
			if err := json.Unmarshal(data, &original); err != nil {
				t.Fatal(err)
			}
		}
	}
	stdlib := map[string]string{
		"encoding/json/decode.go":  "1632161a34c8286722716a48ba0b5e0c3d117a2e017e79ace676403808a16e6e",
		"encoding/json/scanner.go": "2b16dd215274dfa8e0806b53cca144684d5b9c8a02319cf5ce10120c50e6eef2",
		"encoding/json/stream.go":  "065501364e4954cf1c8f1887900248d4c0983593b8c058b2fe7997d9635b793d",
	}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("frozen JSON streaming source changed: %s", path)
		}
	}
	if len(original.Cases) != 93 {
		t.Fatal("public93 input inventory changed")
	}
	var baseline migrationPublicSolverInput
	for _, row := range original.Cases {
		if row.Name == "cache/native_future" {
			baseline = row.Input
		}
	}
	if len(baseline.Native) != 3 || len(baseline.Control) != 1 || baseline.Calls != 2 {
		t.Fatal("reused public challenge/recovery input missing")
	}
	const addedDepth = 9997
	prefix := `{"status":"ok","solution":{"userAgent":"synthetic-solver-agent","cookies":[{"name":"cf_clearance","value":"synthetic-clearance","ignored":` + strings.Repeat("[", addedDepth)
	inputs := []migrationSolverStreamRow{
		{Name: "stream_completion/depth_10001_before_remainder", Input: migrationSolverStreamInput{
			Native: append([]migrationFanboxStep{}, baseline.Native[:1]...), Prefix: migrationSolverStreamEncode([]byte(prefix)),
			Remainder: migrationSolverStreamEncode([]byte("0" + strings.Repeat("]", addedDepth) + "}]}}")), PrefixGlobalDepth: 10001,
		}},
		{Name: "stream_completion/first_object_before_invalid_deep_tail", Input: migrationSolverStreamInput{
			Native: append([]migrationFanboxStep{}, baseline.Native[:2]...), Prefix: migrationSolverStreamEncode([]byte(baseline.Control[0].Body)),
			Remainder: migrationSolverStreamEncode([]byte(" " + strings.Repeat("[", 10001) + "0" + strings.Repeat("]", 10001) + " \xff")), PrefixGlobalDepth: 4,
		}},
	}
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-stream-completion.json")
	if !*migrationCaptureFanboxSolverStreamCompletion {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var saved struct {
			Cases []migrationSolverStreamRow `json:"cases"`
		}
		if err := json.Unmarshal(data, &saved); err != nil {
			t.Fatal(err)
		}
		if len(saved.Cases) != len(inputs) {
			t.Fatal("bounded streaming inventory changed")
		}
		for index, row := range saved.Cases {
			got, gotErr := json.Marshal(row.Input)
			want, wantErr := json.Marshal(inputs[index].Input)
			if gotErr != nil || wantErr != nil || !bytes.Equal(got, want) || row.Name != inputs[index].Name {
				t.Fatal("saved exact-byte streaming input differs")
			}
			inputs[index].Input = row.Input
		}
	}
	for index := range inputs {
		row := &inputs[index]
		t.Run(row.Name, func(t *testing.T) {
			row.Observation = migrationSolverStreamObserve(t, row.Input)
			outcome := row.Observation["outcomes"].(map[string]any)["single"].([]map[string]any)[0]
			publicError := outcome["error"].(map[string]any)
			requests := row.Observation["native"].(map[string]any)["requests"].([]map[string]any)
			completed, replay := 0, 0
			for _, event := range row.Observation["events"].([]diagnostics.Event) {
				if event.Kind == diagnostics.EventSolverCompleted {
					completed++
				}
				if event.Kind == diagnostics.EventReplay {
					replay++
				}
			}
			if index == 0 {
				if publicError["code"] != "malformed_upstream_response" || publicError["source_message"] != "fanbox: malformed FlareSolverr response" || len(requests) != 1 || completed != 0 || replay != 0 {
					t.Fatal("captured eager depth failure or absence of replay changed")
				}
			} else if publicError["code"] != "" || len(requests) != 2 || completed != 1 || replay != 1 {
				t.Fatal("captured first-object successful recovery changed")
			}
		})
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{
		"source_commit": reference.SourceCommit, "published_baseline": "3dee232bf2571abbb9835fcd5ba9d5042aa0ec03", "go_version": reference.GoVersion,
		"source_sha256": reference.SourceSHA256, "go_json_stdlib_sha256": stdlib, "protected_sealed_files_sha256": protected,
		"frozen_go_production_guard":       frozenProduction,
		"source_public_fixture_case_count": 93, "reused_public_case": "cache/native_future", "public_operation": "fanbox.Client.CurrentUser",
		"response_encoding": "lowercase hexadecimal exact prefix and withheld remainder bytes, with length and SHA256", "cases": inputs,
		"evidence": "Actual frozen public OpenWith/CurrentUser with injected native homepage challenge and genuine configured anonymous ordinary HTTP/1 loopback solver POST. Each handler writes and flushes the exact recorded prefix, then blocks the remainder and response EOF on a test-owned release. The real public call returns while that release has not been signaled and the handler is unfinished. Only afterward does cleanup release the remainder and close all server connections and handlers. The depth row reaches global JSON object/array depth 10001 in an ignored cookie member. The success row flushes a complete first solver object while withholding a depth-10001 second value and invalid UTF-8 tail. The five-second outer timer is solely a test-failure guard; timeouts are never persisted as observations or substituted for public errors.",
		"source_decisions": []string{
			"encoding/json/scanner.go pushParseState rejects the 10001st total object/array opener",
			"encoding/json/stream.go Decoder.readValue returns scanError before response EOF",
			"encoding/json/stream.go Decoder.readValue invents a space after a completed top-level object/array, avoiding another source read",
			"internal/services/fanbox/protocol/solver.go uses exactly one Decoder.Decode and maps its failure to ErrMalformedSolverResponse",
		},
		"limitations": []string{
			"Two finite public streaming-completion cases, separate from sealed47/79; no general streaming parser or JSON/HTML parity claim",
			"Synthetic response bytes are bounded to 128 KiB; this is a test bound and introduces no production response-size limit",
			"No native execution, HTTP/2, HEAD, upload, multiplex, retry, external authentication, private Solver/session injection or clock API",
			"Native body read-call topology, concrete Go error/sentinel identity and default control User-Agent/protocol remain Go-only projections",
			"Flush and completion/release ordering are observed; TCP packet/chunk topology, withheld-tail consumption and physical solver-response Close error identity are not claimed",
		},
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *migrationCaptureFanboxSolverStreamCompletion {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public streaming completion observations differ from frozen Go fixture")
	}
	t.Logf("public solver streaming completion: %d rows, bytes=%d sha256=%x", len(inputs), len(data), sha256.Sum256(data))
}
