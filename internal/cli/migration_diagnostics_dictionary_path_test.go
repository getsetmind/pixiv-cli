//go:build linux && amd64

package cli

import (
	"bytes"
	"crypto/sha256"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

var captureDiagnosticsDictionaryPath = flag.Bool("migration-capture-diagnostics-dictionary-path", false, "capture frozen dictionary command paths around scalar flag values")

type diagnosticsDictionaryPathCase struct {
	Name          string                `json:"name"`
	EvidenceClass string                `json:"evidence_class"`
	Input         updaterCLIInput       `json:"input"`
	Observation   updaterCLIObservation `json:"observation"`
	HTTPAttempts  int                   `json:"http_attempts"`
}
type diagnosticsDictionaryPathFixture struct {
	Schema      int                             `json:"schema_version"`
	Frozen      string                          `json:"frozen_go"`
	Toolchain   string                          `json:"toolchain"`
	Sources     map[string]string               `json:"source_sha256"`
	Grammar     []string                        `json:"source_grammar"`
	Limitations []string                        `json:"limitations"`
	Cases       []diagnosticsDictionaryPathCase `json:"cases"`
}

func diagnosticsDictionaryPathRows() []diagnosticsDictionaryPathCase {
	rows := []diagnosticsDictionaryPathCase{}
	for _, item := range []struct {
		name string
		args []string
	}{
		{"article/preleaf-lang-search", []string{"dic", "--lang", "search", "article", "owned", "--json"}},
		{"article/preleaf-lang-article", []string{"dic", "--lang", "article", "article", "owned", "--json"}},
		{"article/preleaf-lang-equals-search", []string{"dic", "--lang=search", "article", "owned", "--json"}},
		{"article/postleaf-lang-search", []string{"dic", "article", "owned", "--lang", "search", "--json"}},
		{"search/preleaf-page-article", []string{"dic", "--page", "article", "search", "owned", "--json"}},
		{"search/preleaf-limit-search", []string{"dic", "--limit", "search", "search", "owned", "--json"}},
		{"search/preleaf-lang-article", []string{"dic", "--lang", "article", "search", "owned", "--json"}},
		{"search/postleaf-lang-search", []string{"dic", "search", "owned", "--lang", "search", "--json"}},
	} {
		config := strings.ReplaceAll(updaterCLIConfig, "check_enabled=true", "check_enabled=false")
		rows = append(rows, diagnosticsDictionaryPathCase{Name: item.name, EvidenceClass: "behavioral", Input: updaterCLIInput{Boundary: "actual-root-RunContext", Args: item.args, Config: &config, Version: updaterCLICurrent, Environment: map[string]string{"PIXIV_LOG_LEVEL": "debug", "PIXIV_LOG_FORMAT": "json"}}})
	}
	return rows
}

func diagnosticsDictionaryPathObserve(t *testing.T, row *diagnosticsDictionaryPathCase) {
	t.Helper()
	oldTransport := http.DefaultTransport
	http.DefaultTransport = updaterCLITransport(func(*http.Request) (*http.Response, error) {
		row.HTTPAttempts++
		return nil, errors.New("owned dictionary HTTP is forbidden")
	})
	t.Cleanup(func() { http.DefaultTransport = oldTransport })
	row.Observation = updaterCLIObserve(t, row.Input)
	if row.HTTPAttempts != 0 {
		t.Fatal("dictionary validation unexpectedly reached an HTTP request; transport denied it before any network")
	}
}

func diagnosticsDictionaryPathNormalize(t *testing.T, row diagnosticsDictionaryPathCase) diagnosticsDictionaryPathCase {
	t.Helper()
	o := row.Observation
	root := diagnosticsRootCase{Observation: diagnosticsRootObservation{Target: o.Target, Stderr: o.Stderr, ErrorWrites: diagnosticsRootAcceptedWrites(t, o.Stderr, o.ErrorWrites)}}
	root = diagnosticsRootNormalize(t, root)
	row.Observation.Stderr = root.Observation.Stderr
	row.Observation.ErrorWrites = []updaterCLIWrite{}
	for _, write := range root.Observation.ErrorWrites {
		row.Observation.ErrorWrites = append(row.Observation.ErrorWrites, updaterCLIWrite{Length: write.Length, N: write.N, Error: write.Error})
	}
	return row
}

func diagnosticsDictionaryPathAssert(t *testing.T, row diagnosticsDictionaryPathCase) {
	t.Helper()
	o := row.Observation
	article := strings.HasPrefix(row.Name, "article/")
	wantTarget := "pixiv dic search"
	wantExit := 2
	if article {
		wantTarget = "pixiv dic article"
	}
	if strings.Contains(row.Name, "page-") || strings.Contains(row.Name, "limit-") {
		wantExit = 1
	}
	if o.Target != wantTarget || o.Exit != wantExit || o.Stdout != "" || row.HTTPAttempts != 0 || o.AccountFactories != 0 || o.SDKFactories != 0 || o.FanboxFactories != 0 || o.DatabaseAfter || len(o.AutomaticProxies) != 0 || o.StdinReads != 0 {
		t.Fatalf("actual dictionary scalar path/startup contract changed: %+v", row)
	}
	if !o.Lifecycle.StartupHooks || !o.Lifecycle.EnsureConfig || !o.Lifecycle.AutomaticUpdate || o.Lifecycle.MCP {
		t.Fatalf("dictionary nearest owner lifecycle changed: %+v", o.Lifecycle)
	}
	if article {
		if o.ParserError != "" || !strings.Contains(o.Stderr, `"operation":"pixiv dic article"`) || !strings.Contains(o.Stderr, `"kind":"started"`) || !strings.Contains(o.Stderr, `"kind":"failed"`) || !strings.HasSuffix(o.Stderr, "error: --lang must be one of: ja, en\n") {
			t.Fatalf("article language did not fail after canonical root diagnostics: %+v", o)
		}
		if len(o.ErrorWrites) != 3 {
			t.Fatal("article validation root writes changed")
		}
	} else {
		if o.ParserError == "" || o.RuntimeCalls != 0 || len(o.ErrorWrites) != 1 {
			t.Fatalf("parser failure reached root diagnostic startup: %+v", o)
		}
		for _, entry := range o.Trace {
			if strings.HasPrefix(entry, "startup.") || strings.HasPrefix(entry, "runtime.") {
				t.Fatal("ParseFlags failure reached startup or runtime")
			}
		}
	}
}

func TestMigrationDiagnosticsDictionaryCanonicalPathFrozen(t *testing.T) {
	repo, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	sources, _, _, _, _ := updaterCLISources(t, repo)
	for _, path := range []string{"internal/cli/commands/pixiv/dic/dic.go", "internal/cli/commands/pixiv/dic/search.go", "internal/cli/commands/pixiv/dic/article.go", "internal/services/dic/transport.go", "internal/cli/diagnostics/diagnostics.go", "internal/shared/diagnostics/diagnostics.go", "internal/cli/migration_diagnostics_dictionary_path_test.go", "internal/cli/migration_diagnostics_connected_test.go", "internal/cli/migration_diagnostics_joined_test.go", "crates/pixiv-cli/tests/fixtures/diagnostics-root.json", "crates/pixiv-cli/tests/fixtures/diagnostics-root-joined.json"} {
		b, err := os.ReadFile(filepath.Join(repo, path))
		if err != nil {
			t.Fatal(err)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(b))
	}
	cobraPath := filepath.Join(os.Getenv("GOMODCACHE"), "github.com", "spf13", "cobra@v1.10.1", "command.go")
	cobraSource, err := os.ReadFile(cobraPath)
	if err != nil {
		t.Fatal(err)
	}
	sources["module:github.com/spf13/cobra@v1.10.1/command.go"] = fmt.Sprintf("%x", sha256.Sum256(cobraSource))
	f := diagnosticsDictionaryPathFixture{Schema: 1, Frozen: updaterCLIReference, Toolchain: runtime.Version(), Sources: sources, Cases: diagnosticsDictionaryPathRows(), Grammar: []string{
		"Pinned Cobra v1.10.1 Command.Find walks subcommands after stripFlags removes --flag value pairs and --flag=value tokens. argsMinusFirstX skips scalar flag values when removing the actual subcommand token, so a value equal to search/article is not removed as the leaf.",
		"dic.New binds Normal to the group; unannotated search/article leaves inherit StartupHooks=true, EnsureConfig=true, AutomaticUpdate=true, MCP=false. Both leaves require exactly one positional argument before root pre-run hooks.",
		"Article owns scalar --lang (default ja), --no-counters and --json/-j. Its RunE checks ja/en and returns usageError before requesting its anonymous dictionary reader. Invalid language therefore fails after startup/config and started diagnostics with canonical path pixiv dic article, then failed and usage exit 2.",
		"Search owns integer --page and --limit/-n, --json/-j and --ndjson. Invalid integer scalar values fail ParseFlags before positional validation/startup/config/diagnostics. --lang is not a search flag, and search rejects it as an unknown option before startup. Each parsed target and Changed flag state are independently captured by a fresh production Cobra root.",
	}, Limitations: []string{
		"Eight actual Go RunContext/root dictionary failure rows use owned config, debug JSON diagnostics and disabled automatic update. The existing final updater setup/observer isolates home and stubs startup/account/update ports. http.DefaultTransport is additionally replaced by a fail-closed in-memory RoundTripper before roots are constructed; zero HTTP attempts are asserted. There is no HTTP, DNS, TLS, account, media, browser, registration, HKCU or Cargo action.",
		"All four article rows stop at language validation before reader use; all four search rows stop at flag parsing before startup. These capture ordered grammar/canonical diagnostic path and failure bytes, not dictionary HTTP success or post-success automatic checking.",
		"Root presenter clock is unchanged. Exact raw accepted stderr and writer observations are retained separately. Only validated canonical root JSON time positions are normalized by the already sealed primary normalizer; byte-length changes come solely from that timestamp substitution. Ordinary parser/usage errors, target, requirements, flag values/Changed state, stdout, trace, counters and config remain exact.",
		"Primary root19 and joined-source6 producers/fixtures remain byte-identical. Existing updater source guard verifies the unchanged 434 frozen Go production/module files and 110 preexisting published fixtures. The Cobra source is version-pinned in unchanged go.mod/go.sum and its actual cached source hash is captured explicitly.",
	}}
	raw := []diagnosticsDictionaryPathCase{}
	for i := range f.Cases {
		row := &f.Cases[i]
		t.Run(row.Name, func(t *testing.T) {
			diagnosticsDictionaryPathObserve(t, row)
			raw = append(raw, *row)
			*row = diagnosticsDictionaryPathNormalize(t, *row)
			diagnosticsDictionaryPathAssert(t, *row)
		})
	}
	if t.Failed() {
		return
	}
	data := diagnosticsRootJSON(t, f)
	path := filepath.Join(repo, "crates/pixiv-cli/tests/fixtures/diagnostics-dictionary-path.json")
	rawPath := filepath.Join(repo, "crates/pixiv-cli/tests/support/diagnostics-dictionary-path-evidence/raw-capture.json")
	if *captureDiagnosticsDictionaryPath {
		if err := os.MkdirAll(filepath.Dir(rawPath), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(rawPath, diagnosticsRootJSON(t, raw), 0644); err != nil {
			t.Fatal(err)
		}
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
		t.Fatal("dictionary scalar command path contract changed")
	}
}
