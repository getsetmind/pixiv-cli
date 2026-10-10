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
	"os"
	"path/filepath"
	"runtime"
	"syscall"
	"testing"
	"time"

	clidiagnostics "github.com/FlanChanXwO/pixiv-cli/internal/cli/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	core "github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
)

var captureDiagnosticsJoined = flag.Bool("migration-capture-diagnostics-joined", false, "capture frozen joined pipeline/startup diagnostic classification")

type diagnosticsJoinedInput struct {
	Business string `json:"business_error"`
	Sink     string `json:"diagnostic_writer"`
	NDJSON   bool   `json:"ndjson_output"`
	Machine  bool   `json:"machine_output"`
}
type diagnosticsJoinedObservation struct {
	Exit                int                    `json:"exit"`
	Stderr              string                 `json:"stderr"`
	Trace               []string               `json:"trace"`
	Writes              []diagnosticsRootWrite `json:"error_writes"`
	Error               string                 `json:"final_error"`
	ErrorType           string                 `json:"final_error_type"`
	DiagnosticError     string                 `json:"diagnostic_error"`
	DiagnosticErrorType string                 `json:"diagnostic_error_type"`
	BusinessIdentity    bool                   `json:"business_identity_preserved"`
	BusinessCause       bool                   `json:"business_cause_preserved"`
	DiagnosticCause     bool                   `json:"diagnostic_cause_preserved"`
	Startup             bool                   `json:"errors_as_startup"`
	Pipeline            bool                   `json:"errors_as_pipeline"`
	Usage               bool                   `json:"errors_as_usage"`
	EPIPE               bool                   `json:"errors_is_epipe"`
}
type diagnosticsJoinedCase struct {
	Name          string                       `json:"name"`
	EvidenceClass string                       `json:"evidence_class"`
	Input         diagnosticsJoinedInput       `json:"input"`
	Observation   diagnosticsJoinedObservation `json:"observation"`
}
type diagnosticsJoinedFixture struct {
	Schema      int                     `json:"schema_version"`
	Frozen      string                  `json:"frozen_go"`
	Toolchain   string                  `json:"toolchain"`
	Sources     map[string]string       `json:"source_sha256"`
	Limitations []string                `json:"limitations"`
	Cases       []diagnosticsJoinedCase `json:"cases"`
}

func diagnosticsJoinedObserve(t *testing.T, input diagnosticsJoinedInput) diagnosticsJoinedObservation {
	t.Helper()
	trace := []string{}
	writer := &diagnosticsRootWriter{mode: input.Sink, name: "stderr", trace: &trace, writes: []diagnosticsRootWrite{}}
	presenter := clidiagnostics.NewPresenterWithFormat(writer, "json", func() time.Time { return time.Date(2000, 1, 2, 3, 4, 5, 0, time.UTC) })
	ctx := core.WithScope(context.Background(), presenter, core.ModulePixivCLI, 0)
	a := app{errOut: writer, diagnostics: &diagnosticState{ctx: ctx, operation: "pixiv download", presenter: presenter}}
	core.Emit(ctx, core.Event{Kind: core.EventStarted, Operation: "pixiv download"})
	var business error
	var businessCause error
	if input.Business == "pipeline" {
		business = &pipeline.PipelineDiagnosticError{}
		businessCause = business
	} else {
		businessCause = errors.New("owned startup cause")
		business = &startupError{err: businessCause}
	}
	err := a.finishDiagnostics(business)
	var startup *startupError
	var pipelineErr *pipeline.PipelineDiagnosticError
	var usage *usageError
	diagnosticErr := presenter.Err()
	if err == nil || diagnosticErr == nil {
		t.Fatal("joined source causes missing")
	}
	o := diagnosticsJoinedObservation{Error: err.Error(), ErrorType: fmt.Sprintf("%T", err), DiagnosticError: diagnosticErr.Error(), DiagnosticErrorType: fmt.Sprintf("%T", diagnosticErr), BusinessIdentity: errors.Is(err, business), BusinessCause: errors.Is(err, businessCause), DiagnosticCause: errors.Is(err, diagnosticErr), Startup: errors.As(err, &startup), Pipeline: errors.As(err, &pipelineErr), Usage: errors.As(err, &usage), EPIPE: errors.Is(err, syscall.EPIPE)}
	o.Exit = a.exitWithNDJSONScope(err, input.NDJSON, input.Machine)
	o.Stderr, o.Trace, o.Writes = writer.buf.String(), trace, writer.writes
	return o
}

func diagnosticsJoinedAssert(t *testing.T, row diagnosticsJoinedCase) {
	t.Helper()
	o := row.Observation
	wantExit := 1
	if row.Input.NDJSON {
		wantExit = 0
	}
	wantOutput := ""
	if row.Input.Business == "startup" && !row.Input.NDJSON {
		wantOutput = "owned startup cause\n"
	}
	if o.Exit != wantExit || o.Stderr != wantOutput {
		t.Fatalf("joined root classification changed: %+v", o)
	}
	if !o.BusinessIdentity || !o.BusinessCause || !o.DiagnosticCause || o.Usage || o.Startup != (row.Input.Business == "startup") || o.Pipeline != (row.Input.Business == "pipeline") || o.EPIPE != (row.Input.Sink == "root-diagnostic-epipe") {
		t.Fatalf("joined source causes changed: %+v", o)
	}
	if o.ErrorType != "*errors.joinError" {
		t.Fatalf("join representation changed: %s", o.ErrorType)
	}
	if len(o.Writes) < 2 {
		t.Fatal("root started/failed diagnostic attempts missing")
	}
	wantStarted := "{\"time\":\"2000-01-02T03:04:05Z\",\"level\":\"DEBUG\",\"module\":\"Pixiv CLI\",\"kind\":\"started\",\"operation\":\"pixiv download\"}\n"
	wantFailed := "{\"time\":\"2000-01-02T03:04:05Z\",\"level\":\"DEBUG\",\"module\":\"Pixiv CLI\",\"kind\":\"failed\",\"operation\":\"pixiv download\",\"reason\":\"command failed\"}\n"
	if o.Writes[0].Payload != wantStarted || o.Writes[1].Payload != wantFailed || o.Writes[0].N != 0 || o.Writes[1].N != 0 {
		t.Fatalf("fixed-clock diagnostic payload changed: %+v", o.Writes)
	}
	if row.Input.Business == "pipeline" || row.Input.NDJSON {
		if len(o.Writes) != 2 {
			t.Fatal("root repeated an already-classified pipeline/startup error")
		}
	} else if len(o.Writes) != 3 || o.Writes[2].Payload != "owned startup cause\n" {
		t.Fatal("startup error lost its bare cause output")
	}
}

func TestMigrationDiagnosticsJoinedClassificationFrozen(t *testing.T) {
	repo, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	sources, _, _, _, _ := updaterCLISources(t, repo)
	for _, path := range []string{"internal/cli/diagnostics/diagnostics.go", "internal/shared/diagnostics/diagnostics.go", "internal/cli/pipeline/records.go", "internal/cli/migration_diagnostics_connected_test.go", "internal/cli/migration_diagnostics_joined_test.go", "crates/pixiv-cli/tests/fixtures/diagnostics-root.json"} {
		b, err := os.ReadFile(filepath.Join(repo, path))
		if err != nil {
			t.Fatal(err)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(b))
	}
	f := diagnosticsJoinedFixture{Schema: 1, Frozen: updaterCLIReference, Toolchain: runtime.Version(), Sources: sources, Cases: []diagnosticsJoinedCase{}, Limitations: []string{
		"Six Go-only same-package lifecycle witnesses invoke the unchanged actual app.finishDiagnostics and exitWithNDJSONScope with real source error types and a real fixed-clock Presenter. They are supplemental source classification evidence, not six actual RunContext/startup/download business executions.",
		"A pipeline business error means its own record diagnostics have already been emitted. This witness starts with that error and observes that the root does not append another ordinary error or envelope after diagnostics are joined. It does not synthesize a record processor or claim full download/mutation parity.",
		"The startup business error is supplied after Presenter start solely to freeze recursive errors.As and its bare-cause exit formatting. Normal production startup hooks run before diagnostic start; this is a joined-error classification witness, not evidence that ordinary startup is diagnosed after beginning a command.",
		"NDJSON errors.Is(EPIPE) wins before both startup and pipeline errors.As classification. Diagnostic EPIPE is a source-wrapped cause joined with the business error; stdout is never written. Ordinary writer failure and diagnostic EPIPE cases retain exact source identity/type flags, attempted bytes, write counts and final stderr. Fixed clock means no normalization is applied.",
		"Primary diagnostics-root19 fixture, its producer and its prior four fixed-clock cause assertions remain byte-identical. The unchanged 434 frozen Go production/module files and 110 preexisting published fixtures are verified by the existing updater source guard. No Cargo, external network, real account, media, browser, native registration, or host settings action occurs.",
	}}
	for _, business := range []string{"pipeline", "startup"} {
		for _, variant := range []struct {
			name   string
			sink   string
			ndjson bool
		}{{"diagnostic-error", "root-diagnostic-error", false}, {"diagnostic-epipe-ordinary", "root-diagnostic-epipe", false}, {"diagnostic-epipe-ndjson", "root-diagnostic-epipe", true}} {
			row := diagnosticsJoinedCase{Name: business + "/" + variant.name, EvidenceClass: "go-only", Input: diagnosticsJoinedInput{Business: business, Sink: variant.sink, NDJSON: variant.ndjson, Machine: true}}
			t.Run(row.Name, func(t *testing.T) {
				row.Observation = diagnosticsJoinedObserve(t, row.Input)
				diagnosticsJoinedAssert(t, row)
			})
			f.Cases = append(f.Cases, row)
		}
	}
	if t.Failed() {
		return
	}
	data := diagnosticsRootJSON(t, f)
	path := filepath.Join(repo, "crates/pixiv-cli/tests/fixtures/diagnostics-root-joined.json")
	if *captureDiagnosticsJoined {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("supplemental joined root diagnostic contract changed")
	}
	var decoded diagnosticsJoinedFixture
	if err := json.Unmarshal(want, &decoded); err != nil {
		t.Fatal(err)
	}
	if len(decoded.Cases) != 6 {
		t.Fatal("supplemental source classification row count changed")
	}
}
