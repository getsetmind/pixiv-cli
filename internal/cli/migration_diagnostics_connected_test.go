//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	clidiagnostics "github.com/FlanChanXwO/pixiv-cli/internal/cli/diagnostics"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	core "github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var captureDiagnosticsRoot = flag.Bool("migration-capture-diagnostics-root", false, "capture frozen actual root diagnostics lifecycle")

const diagnosticsRootClock = "2000-01-02T03:04:05Z"

type diagnosticsRootInput struct {
	updaterCLIInput
	SyntheticSDK bool `json:"synthetic_sdk_execution,omitempty"`
	WriteOutput  bool `json:"write_owned_output,omitempty"`
}
type diagnosticsRootWrite struct {
	Payload string `json:"payload"`
	Length  int    `json:"length"`
	N       int    `json:"n"`
	Error   string `json:"error"`
}
type diagnosticsRootContext struct {
	Scope             bool   `json:"scope_present"`
	Value             string `json:"value"`
	Deadline          string `json:"deadline"`
	DeadlineEqual     bool   `json:"deadline_preserved"`
	DoneEqual         bool   `json:"done_preserved"`
	Canceled          bool   `json:"canceled"`
	StateContextEqual bool   `json:"state_context_equals_execution"`
	ParentSinkCalls   int    `json:"parent_sink_calls"`
}
type diagnosticsRootObservation struct {
	Exit           int                     `json:"exit"`
	Stdout         string                  `json:"stdout"`
	Stderr         string                  `json:"stderr"`
	Trace          []string                `json:"trace"`
	OutputWrites   []diagnosticsRootWrite  `json:"output_writes"`
	ErrorWrites    []diagnosticsRootWrite  `json:"error_writes"`
	RuntimeCalls   int                     `json:"runtime_calls"`
	Target         string                  `json:"target"`
	Context        *diagnosticsRootContext `json:"context,omitempty"`
	DatabaseBefore bool                    `json:"database_before"`
	DatabaseAfter  bool                    `json:"database_after"`
	Accounts       []map[string]any        `json:"accounts_after"`
	Requests       []updaterCLIRequest     `json:"requests"`
}
type diagnosticsRootCase struct {
	Name                string                     `json:"name"`
	EvidenceClass       string                     `json:"evidence_class"`
	RepresentationScope string                     `json:"representation_scope"`
	Input               diagnosticsRootInput       `json:"input"`
	Observation         diagnosticsRootObservation `json:"observation"`
}
type diagnosticsRootFixture struct {
	Schema      int                   `json:"schema_version"`
	Frozen      string                `json:"frozen_go"`
	Head        string                `json:"capture_head"`
	Toolchain   string                `json:"toolchain"`
	Sources     map[string]string     `json:"source_sha256"`
	Limitations []string              `json:"limitations"`
	Cases       []diagnosticsRootCase `json:"cases"`
}

func diagnosticsRootRows() []diagnosticsRootCase {
	rows := []diagnosticsRootCase{}
	config := strings.ReplaceAll(updaterCLIConfig, "check_enabled=true", "check_enabled=false")
	add := func(name string, args ...string) *diagnosticsRootInput {
		copy := config
		rows = append(rows, diagnosticsRootCase{Name: name, EvidenceClass: "behavioral", RepresentationScope: "Actual RunContext/root/Cobra/startup/config/finish/exit with existing updater producer's synthetic external constructors; business command owner remains real", Input: diagnosticsRootInput{updaterCLIInput: updaterCLIInput{Boundary: "actual-root-RunContext", Args: args, Version: updaterCLICurrent, Config: &copy, Environment: map[string]string{"PIXIV_LOG_LEVEL": "debug", "PIXIV_LOG_FORMAT": "text"}, Source: update.InstallSourceRelease, Release: "v1.3.0"}}})
		return &rows[len(rows)-1].Input
	}
	add("update/text-success", "update", "--check")
	add("update/json-success", "update", "--check", "--json").Environment["PIXIV_LOG_FORMAT"] = "json"
	add("update/text-business-failure", "update", "--check").Failure = "checker"
	in := add("update/json-business-failure", "update", "--check", "--json")
	in.Failure, in.Environment["PIXIV_LOG_FORMAT"] = "checker", "json"
	add("exclude/quiet-config", "config", "get", "request_interval")
	add("exclude/non-debug", "update", "--check").Environment["PIXIV_LOG_LEVEL"] = "info"
	in = add("fanbox/module", "fanbox", "creators", "--kind=invalid", "--json")
	in.Environment["PIXIV_LOG_FORMAT"] = "json"
	add("cleanup/success-before-completed", "auth", "list", "--json").Seed = true
	in = add("cleanup/failure-before-failed", "auth", "list", "--json")
	in.Seed, in.CloseError = true, true
	add("epipe/ordinary-stdout", "update", "--check").Writer = "epipe"
	for _, item := range []struct {
		name     string
		machine  string
		stdout   bool
		sink     bool
		failure  string
		canceled bool
	}{
		{"scope/deadline-value", "--ndjson", true, false, "", false},
		{"scope/canceled-deadline-value", "--ndjson", false, false, "", true},
		{"epipe/ndjson-stdout-and-diagnostics", "--ndjson", true, true, "", false},
		{"epipe/explicit-json-false-stdout-and-diagnostics", "--json=false", true, true, "", false},
		{"epipe/ndjson-diagnostics-only", "--ndjson", false, true, "", false},
		{"epipe/explicit-json-false-diagnostics-only", "--json=false", false, true, "", false},
		{"epipe/ndjson-business-plus-diagnostics", "--ndjson", false, true, "owned-business", false},
		{"epipe/explicit-json-false-business-plus-diagnostics", "--json=false", false, true, "owned-business", false},
		{"join/usage-plus-diagnostics", "--json=false", false, true, "owned-usage", false},
	} {
		in = add(item.name, "search", "owned", item.machine)
		in.SyntheticSDK, in.WriteOutput, in.Cancel, in.Failure = true, item.stdout, item.canceled, item.failure
		in.Environment["PIXIV_LOG_FORMAT"] = "json"
		if item.sink {
			in.ErrorWriter = "root-diagnostic-epipe"
		}
		if item.failure == "owned-usage" {
			in.ErrorWriter = "root-diagnostic-error"
		}
		if item.stdout && item.sink {
			in.Writer = "epipe"
		}
		rows[len(rows)-1].RepresentationScope = "Actual RunContext/root/search owner with existing SDK execution constructor seam; synthetic execution writes owned output or returns owned business failure without calling the SDK callback; observes root context, resource-close, diagnostics, join and exit policy only"
	}
	return rows
}

func diagnosticsRootAcceptedWrites(t *testing.T, body string, writes []updaterCLIWrite) []diagnosticsRootWrite {
	t.Helper()
	result := []diagnosticsRootWrite{}
	for _, w := range writes {
		if w.N > len(body) {
			t.Fatal("accepted writer lengths exceed recorded bytes")
		}
		result = append(result, diagnosticsRootWrite{Payload: body[:w.N], Length: w.Length, N: w.N, Error: w.Error})
		body = body[w.N:]
	}
	if body != "" {
		t.Fatal("recorded bytes exceed accepted writer lengths")
	}
	return result
}

func diagnosticsRootFromUpdater(t *testing.T, input diagnosticsRootInput) diagnosticsRootObservation {
	o := updaterCLIObserve(t, input.updaterCLIInput)
	return diagnosticsRootObservation{Exit: o.Exit, Stdout: o.Stdout, Stderr: o.Stderr, Trace: o.Trace,
		OutputWrites: diagnosticsRootAcceptedWrites(t, o.Stdout, o.OutputWrites), ErrorWrites: diagnosticsRootAcceptedWrites(t, o.Stderr, o.ErrorWrites), RuntimeCalls: o.RuntimeCalls, Target: o.Target,
		DatabaseBefore: o.DatabaseBefore, DatabaseAfter: o.DatabaseAfter, Accounts: o.Accounts, Requests: o.Requests}
}

type diagnosticsRootWriter struct {
	mode, name string
	buf        bytes.Buffer
	writes     []diagnosticsRootWrite
	trace      *[]string
}

func (w *diagnosticsRootWriter) Write(p []byte) (int, error) {
	n := len(p)
	var err error
	if w.mode == "epipe" || (w.mode == "root-diagnostic-epipe" && bytes.HasPrefix(p, []byte(`{"time":`))) {
		n, err = 0, syscall.EPIPE
	}
	if w.mode == "root-first-epipe-then-error" && bytes.HasPrefix(p, []byte(`{"time":`)) {
		if len(w.writes) == 0 {
			n, err = 0, syscall.EPIPE
		} else {
			n, err = 0, errors.New("owned later diagnostic failure")
		}
	}
	if w.mode == "root-diagnostic-error" && bytes.HasPrefix(p, []byte(`{"time":`)) {
		n, err = 0, errors.New("owned diagnostic writer failure")
	}
	if n > 0 {
		w.buf.Write(p[:n])
	}
	message := ""
	if err != nil {
		message = err.Error()
	}
	w.writes = append(w.writes, diagnosticsRootWrite{string(p), len(p), n, message})
	*w.trace = append(*w.trace, w.name+".write")
	return n, err
}

type diagnosticsRootValueKey struct{}

func diagnosticsRootSyntheticObserve(t *testing.T, input diagnosticsRootInput) diagnosticsRootObservation {
	t.Helper()
	updaterCLISetup(t, input.updaterCLIInput)
	o := diagnosticsRootObservation{Trace: []string{}, OutputWrites: []diagnosticsRootWrite{}, ErrorWrites: []diagnosticsRootWrite{}, Accounts: []map[string]any{}, Requests: []updaterCLIRequest{}}
	o.Target, _, _, _ = updaterCLIFlags(input.updaterCLIInput)
	out := &diagnosticsRootWriter{mode: input.Writer, name: "stdout", trace: &o.Trace, writes: []diagnosticsRootWrite{}}
	errOut := &diagnosticsRootWriter{mode: input.ErrorWriter, name: "stderr", trace: &o.Trace, writes: []diagnosticsRootWrite{}}
	oldVersion, oldCleanup, oldSupported := buildinfo.Version, cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	oldRuntime, oldSDK, oldAutomatic := loadCLIRuntimeConfig, newCLIPixivSDKPorts, newCLIAutomaticUpdateChecker
	t.Cleanup(func() {
		buildinfo.Version, cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldVersion, oldCleanup, oldSupported
		loadCLIRuntimeConfig, newCLIPixivSDKPorts, newCLIAutomaticUpdateChecker = oldRuntime, oldSDK, oldAutomatic
	})
	buildinfo.Version = input.Version
	cleanupPendingWindowsUpdate = func() error { o.Trace = append(o.Trace, "startup.cleanup"); return nil }
	automaticPersistentHandlerSupported = func() bool { o.Trace = append(o.Trace, "startup.supported"); return false }
	loadCLIRuntimeConfig = func() (settings.RuntimeConfig, error) {
		o.RuntimeCalls++
		o.Trace = append(o.Trace, "runtime.load")
		return defaultCLIRuntimeConfig()
	}
	newCLIAutomaticUpdateChecker = func(string) (*update.AutomaticUpdateChecker, error) {
		t.Fatal("disabled automatic updater constructor was reached")
		return nil, nil
	}
	deadline := time.Date(2099, 1, 2, 3, 4, 5, 0, time.UTC)
	incoming, deadlineCancel := context.WithDeadline(context.WithValue(context.Background(), diagnosticsRootValueKey{}, "owned context value"), deadline)
	defer deadlineCancel()
	incoming, cancel := context.WithCancel(incoming)
	defer cancel()
	if input.Cancel {
		cancel()
	}
	parentCalls := 0
	incoming = core.WithScope(incoming, core.SinkFunc(func(core.Event) { parentCalls++ }), core.ModulePixivMCP, 88)
	newCLIPixivSDKPorts = func(a app) (pixivSDKPorts, error) {
		o.Trace = append(o.Trace, "sdk.synthetic.factory")
		a.closeState.add(func() error { o.Trace = append(o.Trace, "sdk.synthetic.close"); return nil })
		return pixivSDKPorts{
			jsonOut: func(value *bool) (bool, error) {
				if value != nil {
					return *value, nil
				}
				return false, nil
			},
			execute: func(ctx context.Context, _ pixivdeps.Request, _ func(context.Context, *pixiv.Client) (bool, error)) error {
				o.Trace = append(o.Trace, "sdk.synthetic.execute")
				_, scoped := core.ScopeFromContext(ctx)
				actualDeadline, ok := ctx.Deadline()
				value, _ := ctx.Value(diagnosticsRootValueKey{}).(string)
				o.Context = &diagnosticsRootContext{Scope: scoped, Value: value, Deadline: actualDeadline.Format(time.RFC3339Nano), DeadlineEqual: ok && actualDeadline.Equal(deadline), DoneEqual: ctx.Done() == incoming.Done(), Canceled: errors.Is(ctx.Err(), context.Canceled), StateContextEqual: a.diagnostics.ctx == ctx}
				var err error
				if input.WriteOutput {
					_, err = io.WriteString(a.out, "{\"owned\":true}\n")
				}
				if input.Failure == "owned-business" {
					err = errors.Join(err, errors.New("owned business failure"))
				}
				if input.Failure == "owned-usage" {
					err = errors.Join(err, newUsageError(errors.New("owned business usage")))
				}
				return err
			},
		}, nil
	}
	o.Exit = RunContext(incoming, append([]string{"pixiv"}, input.Args...), strings.NewReader(""), out, errOut)
	if o.Context == nil {
		t.Fatal("search owner did not reach synthetic SDK execution boundary")
	}
	o.Context.ParentSinkCalls = parentCalls
	o.Stdout, o.Stderr, o.OutputWrites, o.ErrorWrites = out.buf.String(), errOut.buf.String(), out.writes, errOut.writes
	_, err := os.Stat(".pixiv-cli/pixiv-cli.db")
	o.DatabaseAfter = err == nil
	if o.DatabaseAfter {
		t.Fatal("synthetic SDK lifecycle unexpectedly opened account database")
	}
	return o
}

func diagnosticsRootNormalizeLine(t *testing.T, target, line string) string {
	t.Helper()
	module := string(core.ModulePixivCLI)
	if strings.HasPrefix(target, "pixiv fanbox") {
		module = string(core.ModuleFanboxCLI)
	}
	if strings.HasPrefix(line, "["+module+"] ") {
		prefix := "[" + module + "] "
		if len(line) < len(prefix)+9 {
			t.Fatal("truncated root diagnostic record")
		}
		stamp := line[len(prefix) : len(prefix)+8]
		if _, err := time.Parse("15:04:05", stamp); err != nil {
			t.Fatal(err)
		}
		message := line[len(prefix)+9:]
		capitalized := "Pixiv" + strings.TrimPrefix(target, "pixiv")
		if message != "Started "+target+".\n" && message != capitalized+" completed successfully.\n" && message != capitalized+" failed: the command returned an error.\n" {
			t.Fatalf("unrecognized root narrative %q", message)
		}
		return prefix + "03:04:05 " + message
	}
	if strings.HasPrefix(line, `{"time":`) {
		var fields map[string]json.RawMessage
		if err := json.Unmarshal([]byte(line), &fields); err != nil {
			t.Fatal(err)
		}
		var actualModule, operation, kind, level, stamp, reason string
		for key, target := range map[string]*string{"time": &stamp, "module": &actualModule, "operation": &operation, "kind": &kind, "level": &level, "reason": &reason} {
			if value, exists := fields[key]; exists {
				if err := json.Unmarshal(value, target); err != nil {
					t.Fatal(err)
				}
			}
		}
		if actualModule != module || operation != target || level != "DEBUG" || (kind != "started" && kind != "completed" && kind != "failed") {
			t.Fatalf("unrecognized root JSON diagnostic %q", line)
		}
		if (kind == "failed" && reason != "command failed") || (kind != "failed" && reason != "") {
			t.Fatalf("root reason changed: %q", line)
		}
		for key := range fields {
			if key != "time" && key != "module" && key != "operation" && key != "kind" && key != "level" && key != "reason" {
				t.Fatalf("unexpected root diagnostic field %q", key)
			}
		}
		if _, err := time.Parse(time.RFC3339Nano, stamp); err != nil {
			t.Fatal(err)
		}
		quoted, _ := json.Marshal(stamp)
		prefix := `{"time":` + string(quoted)
		if !strings.HasPrefix(line, prefix) {
			t.Fatal("root presenter timestamp position changed")
		}
		return `{"time":"` + diagnosticsRootClock + `"` + strings.TrimPrefix(line, prefix)
	}
	return line
}

func diagnosticsRootNormalize(t *testing.T, row diagnosticsRootCase) diagnosticsRootCase {
	t.Helper()
	body := ""
	for i, write := range row.Observation.ErrorWrites {
		payload := ""
		for _, line := range strings.SplitAfter(write.Payload, "\n") {
			if line != "" {
				payload += diagnosticsRootNormalizeLine(t, row.Observation.Target, line)
			}
		}
		delta := len(payload) - len(write.Payload)
		write.Payload, write.Length = payload, write.Length+delta
		if write.N > 0 {
			if write.N != len(row.Observation.ErrorWrites[i].Payload) {
				t.Fatal("partial diagnostic acceptance is outside this capture boundary")
			}
			write.N += delta
			body += payload
		}
		row.Observation.ErrorWrites[i] = write
	}
	row.Observation.Stderr = body
	return row
}

func diagnosticsRootAssert(t *testing.T, row diagnosticsRootCase) {
	t.Helper()
	o := row.Observation
	expectedExit := 0
	switch row.Name {
	case "fanbox/module", "update/text-business-failure", "update/json-business-failure", "cleanup/failure-before-failed", "epipe/ordinary-stdout", "epipe/explicit-json-false-stdout-and-diagnostics", "epipe/explicit-json-false-diagnostics-only", "epipe/explicit-json-false-business-plus-diagnostics":
		expectedExit = 1
	}
	if row.Name == "join/usage-plus-diagnostics" {
		expectedExit = 2
	}
	if o.Exit != expectedExit {
		t.Fatalf("exit changed: got %d want %d; %+v", o.Exit, expectedExit, o)
	}
	if strings.HasPrefix(row.Name, "exclude/") {
		if o.Stderr != "" || len(o.ErrorWrites) != 0 {
			t.Fatalf("excluded diagnostics wrote stderr: %+v", o)
		}
		return
	}
	if len(o.ErrorWrites) < 2 {
		t.Fatalf("started and finish writes missing: %+v", o)
	}
	start, finish := o.ErrorWrites[0].Payload, ""
	for _, w := range o.ErrorWrites {
		if strings.Contains(w.Payload, "completed successfully") || strings.Contains(w.Payload, "failed: the command") || strings.Contains(w.Payload, `"kind":"completed"`) || strings.Contains(w.Payload, `"kind":"failed"`) {
			finish = w.Payload
		}
	}
	if !strings.Contains(start, "Started "+o.Target+".") && !strings.Contains(start, `"kind":"started"`) {
		t.Fatalf("start diagnostic missing: %q", start)
	}
	if finish == "" {
		t.Fatal("finish diagnostic missing")
	}
	businessFailed := row.Name == "fanbox/module" || row.Input.Failure == "checker" || row.Input.Failure == "owned-business" || row.Input.Failure == "owned-usage" || row.Input.CloseError || row.Input.Writer == "epipe"
	if businessFailed && !strings.Contains(finish, "failed") {
		t.Fatalf("business/close/output failure emitted completed: %q", finish)
	}
	if !businessFailed && !strings.Contains(finish, "completed") {
		t.Fatalf("sink-only failure must emit completed: %q", finish)
	}
	if row.Name == "fanbox/module" && !strings.Contains(start, `"module":"FANBOX CLI"`) {
		t.Fatal("FANBOX module was not selected")
	}
	if strings.HasPrefix(row.Name, "cleanup/") || row.Input.SyntheticSDK {
		closeIndex, finishIndex := -1, -1
		for i, entry := range o.Trace {
			if entry == "database.close" || entry == "sdk.synthetic.close" {
				closeIndex = i
			}
			if closeIndex >= 0 && i > closeIndex && entry == "stderr.write" {
				finishIndex = i
				break
			}
		}
		if closeIndex < 0 || finishIndex <= closeIndex {
			t.Fatalf("resource close did not precede finish: %v", o.Trace)
		}
	}
	if row.Input.SyntheticSDK {
		c := o.Context
		if c == nil || !c.Scope || c.Value != "owned context value" || !c.DeadlineEqual || !c.DoneEqual || !c.StateContextEqual || c.Canceled != row.Input.Cancel || c.ParentSinkCalls != 0 {
			t.Fatalf("root diagnostic scope lost parent context or leaked parent sink: %+v", c)
		}
		if row.Input.ErrorWriter == "root-diagnostic-epipe" {
			if o.ErrorWrites[0].Error != "broken pipe" || o.ErrorWrites[0].N != 0 {
				t.Fatal("diagnostic EPIPE not observed")
			}
			if expectedExit == 0 && o.Stderr != "" {
				t.Fatal("NDJSON EPIPE path emitted final error")
			}
			if expectedExit == 1 && !strings.Contains(o.Stderr, "write diagnostics: broken pipe") {
				t.Fatal("ordinary final error omitted diagnostic EPIPE")
			}
		}
	}
}

func diagnosticsRootSources(t *testing.T, repo string) map[string]string {
	t.Helper()
	sources, _, _, _, _ := updaterCLISources(t, repo)
	for _, path := range []string{"internal/cli/diagnostics/diagnostics.go", "internal/shared/diagnostics/diagnostics.go", "internal/cli/migration_diagnostics_connected_test.go"} {
		b, err := os.ReadFile(filepath.Join(repo, path))
		if err != nil {
			t.Fatal(err)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(b))
	}
	return sources
}

func diagnosticsRootJSON(t *testing.T, value any) []byte {
	t.Helper()
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	return append(data, '\n')
}

func TestMigrationDiagnosticsRootConnectedFrozen(t *testing.T) {
	repo, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	sources := diagnosticsRootSources(t, repo)
	f := diagnosticsRootFixture{Schema: 1, Frozen: updaterCLIReference, Head: "40faef3f48af16be4acf3044f103532d518449c1", Toolchain: runtime.Version(), Sources: sources, Cases: diagnosticsRootRows(), Limitations: []string{
		"Linux/amd64 actual Go RunContext/root lifecycle; unchanged frozen production/module bytes and existing published fixture inventory are verified by the existing updater source guard. No Cargo, real accounts, external HTTP, media, browser, registration, host settings, or HKCU actions occur.",
		"Ten ordinary rows reuse updaterCLIObserve/setup and its final guarded synthetic constructors. Update retains the real coordinator/release checker with a validated official-endpoint in-memory HTTP response. Cleanup rows retain real command owners/account services and owned temporary SQLite. FANBOX creators invalid-kind validation selects the FANBOX root module without creating services or reaching HTTP. Automatic checks are disabled by the owned config. Existing updater fixture's four runtime/diagnostic-writer/join rows remain unchanged and are not duplicated here.",
		"Nine search rows use the production root and command owner through the existing SDK execution constructor seam. The synthetic execution port does not invoke its SDK callback, open accounts, or fetch data; it writes a bounded owned marker or returns an owned business error. These rows establish connected root scope/cleanup/finish/join/exit semantics, not complete search business execution or output encoding parity. Explicit --json=false ordinary rows retain the root machine-error envelope policy while avoiding the JSON spool whose real execution callback must initialize it.",
		"startDiagnostics uses the real clock and provides no clock injection. Raw accepted stderr bytes and all captured write payloads are retained separately in diagnostics-root-evidence/raw-capture.json. Comparison replaces only validated root text HH:MM:SS spans or the first validated root JSON time property. All module/kind/operation/reason bytes, stdout, ordinary errors, write order/results and context observations remain exact. JSON write length and accepted length changes are derived solely from that timestamp replacement. No duration, trace, error, account or subsystem record is normalized.",
		"Updater producer write payloads are reconstructed only from accepted byte counts, so rejected stdout EPIPE has an empty payload and retains its exact attempted length/error. Synthetic SDK rows record full attempted writer payloads including failed diagnostic EPIPE writes. The root JSON diagnostic-only EPIPE still emits completed; finish then returns wrapped EPIPE. Current NDJSON scope suppresses any joined EPIPE, including a simultaneous ordinary business failure; this behavior is frozen rather than improved in the expectation.",
	}}
	rawCases := []diagnosticsRootCase{}
	for i := range f.Cases {
		row := &f.Cases[i]
		t.Run(row.Name, func(t *testing.T) {
			if row.Input.SyntheticSDK {
				row.Observation = diagnosticsRootSyntheticObserve(t, row.Input)
			} else {
				row.Observation = diagnosticsRootFromUpdater(t, row.Input)
			}
			rawCases = append(rawCases, *row)
			row.Observation.ErrorWrites = append([]diagnosticsRootWrite{}, row.Observation.ErrorWrites...)
			*row = diagnosticsRootNormalize(t, *row)
			diagnosticsRootAssert(t, *row)
		})
	}
	if t.Failed() {
		return
	}
	if !bytes.Equal(diagnosticsRootJSON(t, sources), diagnosticsRootJSON(t, diagnosticsRootSources(t, repo))) {
		t.Fatal("source anchors changed during capture")
	}
	fixturePath := filepath.Join(repo, "crates/pixiv-cli/tests/fixtures/diagnostics-root.json")
	rawPath := filepath.Join(repo, "crates/pixiv-cli/tests/support/diagnostics-root-evidence/raw-capture.json")
	data := diagnosticsRootJSON(t, f)
	if *captureDiagnosticsRoot {
		if err := os.MkdirAll(filepath.Dir(rawPath), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(rawPath, diagnosticsRootJSON(t, rawCases), 0644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(fixturePath, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		actual := filepath.Join(t.TempDir(), "diagnostics-root-actual.json")
		_ = os.WriteFile(actual, data, 0600)
		t.Fatalf("root diagnostic contract changed; actual %s", actual)
	}
	rawData, err := os.ReadFile(rawPath)
	if err != nil {
		t.Fatal(err)
	}
	var captured []diagnosticsRootCase
	if err := json.Unmarshal(rawData, &captured); err != nil {
		t.Fatal(err)
	}
	if len(captured) != len(f.Cases) {
		t.Fatal("raw evidence row count changed")
	}
	for i, row := range captured {
		if !bytes.Equal(diagnosticsRootJSON(t, diagnosticsRootNormalize(t, row)), diagnosticsRootJSON(t, f.Cases[i])) {
			t.Fatalf("raw evidence no longer normalizes to fixture: %s", row.Name)
		}
	}
}

func TestMigrationDiagnosticsRootFinishPreservesJoinedCauses(t *testing.T) {
	for _, row := range []struct {
		name         string
		business     error
		ndjson       bool
		expectedExit int
	}{
		{"diagnostic-only-ordinary", nil, false, 1},
		{"diagnostic-only-ndjson", nil, true, 0},
		{"joined-usage-ordinary", newUsageError(errors.New("owned business usage")), false, 2},
		{"joined-usage-ndjson", newUsageError(errors.New("owned business usage")), true, 0},
	} {
		t.Run(row.name, func(t *testing.T) {
			trace := []string{}
			writer := &diagnosticsRootWriter{mode: "root-first-epipe-then-error", name: "stderr", trace: &trace, writes: []diagnosticsRootWrite{}}
			presenter := clidiagnostics.NewPresenterWithFormat(writer, "json", func() time.Time { return time.Date(2000, 1, 2, 3, 4, 5, 0, time.UTC) })
			ctx := core.WithScope(context.Background(), presenter, core.ModulePixivCLI, 0)
			a := app{errOut: writer, diagnostics: &diagnosticState{ctx: ctx, operation: "pixiv detail", presenter: presenter}}
			core.Emit(ctx, core.Event{Kind: core.EventStarted, Operation: "pixiv detail"})
			err := a.finishDiagnostics(row.business)
			if !errors.Is(err, syscall.EPIPE) || !errors.Is(presenter.Err(), syscall.EPIPE) {
				t.Fatalf("diagnostic cause lost: %v", err)
			}
			if row.business != nil && !errors.Is(err, row.business) {
				t.Fatalf("business identity lost: %v", err)
			}
			var usage *usageError
			if errors.As(err, &usage) != (row.business != nil) {
				t.Fatalf("joined usage classification changed: %v", err)
			}
			if a.exitWithNDJSONScope(err, row.ndjson, true) != row.expectedExit {
				t.Fatalf("joined source-wrapped EPIPE/usage exit changed: %v", err)
			}
			if len(writer.writes) < 2 || writer.writes[0].Error != "broken pipe" || writer.writes[1].Error != "owned later diagnostic failure" {
				t.Fatal("root attempts did not preserve first EPIPE")
			}
			kind := "completed"
			reason := ""
			if row.business != nil {
				kind, reason = "failed", `,"reason":"command failed"`
			}
			want := `{"time":"2000-01-02T03:04:05Z","level":"DEBUG","module":"Pixiv CLI","kind":"` + kind + `","operation":"pixiv detail"` + reason + "}\n"
			if writer.writes[1].Payload != want {
				t.Fatalf("finish bytes changed: %q", writer.writes[1].Payload)
			}
		})
	}
}
