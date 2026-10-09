package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"
)

type windowsInterruptSourceContract struct {
	Evidence               string   `json:"evidence"`
	StandardLibraryVersion string   `json:"standard_library_version"`
	StandardLibrarySource  string   `json:"standard_library_source"`
	StandardLibrarySHA256  string   `json:"standard_library_sha256"`
	FrozenRepositoryRef    string   `json:"frozen_repository_ref"`
	EntrySource            string   `json:"entry_source"`
	EntrySHA256            string   `json:"entry_sha256"`
	Events                 []string `json:"events"`
	RuntimeSignal          string   `json:"runtime_signal"`
	EntrySignal            string   `json:"entry_signal"`
	WindowsCompiled        bool     `json:"windows_compiled"`
	WindowsExecuted        bool     `json:"windows_executed"`
}

func TestMigrationCLIInterruptWindowsMappingMatchesInstalledSource(t *testing.T) {
	fixturePath := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "interrupt_windows_source.json")
	encoded, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var frozen windowsInterruptSourceContract
	if err := json.Unmarshal(encoded, &frozen); err != nil {
		t.Fatal(err)
	}
	source, err := os.ReadFile(filepath.Join(runtime.GOROOT(), filepath.FromSlash(frozen.StandardLibrarySource)))
	if err != nil {
		t.Fatal(err)
	}
	tree, err := parser.ParseFile(token.NewFileSet(), "os_windows.go", source, 0)
	if err != nil {
		t.Fatal(err)
	}
	events := []string{}
	for _, decl := range tree.Decls {
		fn, ok := decl.(*ast.FuncDecl)
		if !ok || fn.Name.Name != "ctrlHandler" {
			continue
		}
		ast.Inspect(fn.Body, func(node ast.Node) bool {
			clause, ok := node.(*ast.CaseClause)
			if !ok {
				return true
			}
			sendsInterrupt := false
			for _, statement := range clause.Body {
				assignment, ok := statement.(*ast.AssignStmt)
				if !ok || len(assignment.Lhs) != 1 || len(assignment.Rhs) != 1 {
					continue
				}
				variable, ok := assignment.Lhs[0].(*ast.Ident)
				if !ok || variable.Name != "s" {
					continue
				}
				signal, ok := assignment.Rhs[0].(*ast.SelectorExpr)
				if !ok || signal.Sel.Name != "SIGINT" {
					continue
				}
				owner, ok := signal.X.(*ast.Ident)
				if ok && owner.Name == "windows" {
					sendsInterrupt = true
				}
			}
			if sendsInterrupt {
				for _, expression := range clause.List {
					event, ok := expression.(*ast.SelectorExpr)
					if !ok {
						t.Fatal("SIGINT case has unexpected console event expression")
					}
					owner, ok := event.X.(*ast.Ident)
					if !ok || owner.Name != "windows" {
						t.Fatal("SIGINT case has unexpected console event owner")
					}
					events = append(events, event.Sel.Name)
				}
			}
			return true
		})
	}
	entry, err := os.ReadFile("main.go")
	if err != nil {
		t.Fatal(err)
	}
	stdlibDigest, entryDigest := sha256.Sum256(source), sha256.Sum256(entry)
	observed := windowsInterruptSourceContract{
		Evidence:               "installed-standard-library-source-only",
		StandardLibraryVersion: runtime.Version(),
		StandardLibrarySource:  "src/runtime/os_windows.go",
		StandardLibrarySHA256:  hex.EncodeToString(stdlibDigest[:]),
		FrozenRepositoryRef:    "4b4426487ef18bed276706daec385e0d0a6979f9",
		EntrySource:            "cmd/pixiv/main.go",
		EntrySHA256:            hex.EncodeToString(entryDigest[:]),
		Events:                 events,
		RuntimeSignal:          "windows.SIGINT",
		EntrySignal:            "os.Interrupt",
	}
	if !reflect.DeepEqual(observed, frozen) {
		actual, _ := json.MarshalIndent(observed, "", "  ")
		t.Fatalf("installed Go Windows source mapping differs from source-only contract:\n%s", actual)
	}
}
