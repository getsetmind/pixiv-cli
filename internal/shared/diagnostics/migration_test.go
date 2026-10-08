package diagnostics_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
)

var updateDiagnostics = flag.Bool("migration-update-diagnostics", false, "update diagnostic scope contracts")

type migrationDiagnosticCase struct {
	Scope    bool                `json:"scope"`
	NilSink  bool                `json:"nil_sink"`
	Child    bool                `json:"child"`
	Explicit bool                `json:"explicit"`
	Direct   bool                `json:"direct"`
	Canceled bool                `json:"canceled"`
	Events   []diagnostics.Event `json:"events"`
}

func TestMigrationDiagnosticsPreserveScopeAndEventFields(t *testing.T) {
	var cases []migrationDiagnosticCase
	for _, scope := range []bool{false, true} {
		for _, nilSink := range []bool{false, true} {
			for _, child := range []bool{false, true} {
				for _, explicit := range []bool{false, true} {
					for _, direct := range []bool{false, true} {
						for _, canceled := range []bool{false, true} {
							row := migrationDiagnosticCase{Scope: scope, NilSink: nilSink, Child: child, Explicit: explicit, Direct: direct, Canceled: canceled}
							ctx, cancel := context.WithCancel(context.Background())
							var sink diagnostics.Sink = diagnostics.SinkFunc(func(event diagnostics.Event) { row.Events = append(row.Events, event) })
							if nilSink {
								sink = nil
							}
							if scope {
								ctx = diagnostics.WithScope(ctx, sink, diagnostics.ModulePixivCLI, 7)
							}
							if child {
								ctx = diagnostics.WithChildScope(ctx, diagnostics.ModuleFanboxSolver, 8)
							}
							if canceled {
								cancel()
							}
							event := diagnostics.Event{Kind: diagnostics.EventAccount, Operation: "selected", Resource: "uid 1", Route: "/safe", Target: "synthetic.file", Proxy: "synthetic", UserAgent: "synthetic-agent", Reason: diagnostics.ReasonAccountFrozen, Status: 429, Count: 3, RequestID: 999, Duration: 123456789}
							if explicit {
								event.Module = diagnostics.ModulePixivAccount
							}
							if direct {
								s, _ := diagnostics.ScopeFromContext(ctx)
								s.Emit(event)
							} else {
								diagnostics.Emit(ctx, event)
							}
							cancel()
							cases = append(cases, row)
						}
					}
				}
			}
		}
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "diagnostics.json")
	if *updateDiagnostics {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, encoded) {
		t.Fatal("diagnostic scope contracts differ")
	}
}
