package migration_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"go/ast"
	"go/format"
	"go/parser"
	"go/token"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	fanboxmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/fanbox"
	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdate = flag.Bool("migration-update-surfaces", false, "regenerate frozen SDK and MCP contracts after reviewing reference changes")

func TestMigrationReferenceManifestPinsSnapshots(t *testing.T) {
	root := filepath.Join("..", "..", "..", "docs", "migration", "reference")
	content, err := os.ReadFile(filepath.Join(root, "manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var manifest struct {
		ReferenceCommit string `json:"reference_commit"`
		Captures        []struct {
			File   string `json:"file"`
			SHA256 string `json:"sha256"`
		} `json:"captures"`
	}
	if err := json.Unmarshal(content, &manifest); err != nil {
		t.Fatal(err)
	}
	if manifest.ReferenceCommit != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("snapshot baseline changed")
	}
	seen := map[string]bool{}
	for _, capture := range manifest.Captures {
		if filepath.Base(capture.File) != capture.File || seen[capture.File] {
			t.Fatalf("invalid or repeated snapshot file: %s", capture.File)
		}
		seen[capture.File] = true
		data, err := os.ReadFile(filepath.Join(root, capture.File))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(data)
		if hex.EncodeToString(digest[:]) != capture.SHA256 {
			t.Errorf("frozen snapshot changed without a reviewed manifest update: %s", capture.File)
		}
	}
	for _, required := range []string{"sdk.json", "mcp-pixiv.json", "mcp-fanbox.json", "cli.windows-amd64.json"} {
		if !seen[required] {
			t.Errorf("required reference snapshot missing: %s", required)
		}
	}
	for file := range seen {
		path := "docs/migration/reference/" + file
		cmd := exec.Command("git", "check-attr", "eol", "--", path)
		cmd.Dir = filepath.Join("..", "..", "..")
		attributes, err := cmd.Output()
		if err != nil {
			t.Fatal(err)
		}
		if strings.TrimSpace(string(attributes)) != path+": eol: lf" {
			t.Errorf("snapshot checkout must preserve LF bytes: %s", path)
		}
	}
}

type sdkDeclaration struct {
	ID          string `json:"id"`
	Kind        string `json:"kind"`
	Source      string `json:"source"`
	Line        int    `json:"line"`
	Declaration string `json:"declaration"`
}

func compareReference(t *testing.T, name string, value any) {
	t.Helper()
	content, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	content = append(content, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "reference", name+".json")
	if *migrationUpdate {
		if err := os.WriteFile(path, content, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(content, want) {
		t.Fatalf("%s contract differs from the frozen reference; review the difference before changing the snapshot", name)
	}
}

func receiverType(expr ast.Expr) string {
	switch value := expr.(type) {
	case *ast.Ident:
		return value.Name
	case *ast.StarExpr:
		return receiverType(value.X)
	case *ast.IndexExpr:
		return receiverType(value.X)
	case *ast.IndexListExpr:
		return receiverType(value.X)
	default:
		return ""
	}
}

func TestMigrationSDKContractIncludesTypesFunctionsAndMethods(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	declarations := make([]sdkDeclaration, 0)
	for _, pkg := range []string{"sdk", "sdk/pixiv", "sdk/fanbox"} {
		entries, err := os.ReadDir(filepath.Join(root, filepath.FromSlash(pkg)))
		if err != nil {
			t.Fatal(err)
		}
		for _, entry := range entries {
			if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") || strings.HasSuffix(entry.Name(), "_test.go") {
				continue
			}
			source := pkg + "/" + entry.Name()
			fset := token.NewFileSet()
			file, err := parser.ParseFile(fset, filepath.Join(root, filepath.FromSlash(source)), nil, 0)
			if err != nil {
				t.Fatal(err)
			}
			add := func(name, kind string, node ast.Node) {
				var formatted bytes.Buffer
				if err := format.Node(&formatted, fset, node); err != nil {
					t.Fatal(err)
				}
				declarations = append(declarations, sdkDeclaration{
					ID: pkg + ":" + name, Kind: kind, Source: source,
					Line: fset.Position(node.Pos()).Line, Declaration: formatted.String(),
				})
			}
			for _, declaration := range file.Decls {
				switch node := declaration.(type) {
				case *ast.FuncDecl:
					if !node.Name.IsExported() {
						continue
					}
					name, kind := node.Name.Name, "function"
					if node.Recv != nil {
						receiver := receiverType(node.Recv.List[0].Type)
						name, kind = receiver+"."+name, "method"
					}
					node.Body = nil
					add(name, kind, node)
				case *ast.GenDecl:
					for _, spec := range node.Specs {
						switch value := spec.(type) {
						case *ast.TypeSpec:
							if value.Name.IsExported() {
								add(value.Name.Name, "type", value)
							}
						case *ast.ValueSpec:
							for _, name := range value.Names {
								if name.IsExported() {
									add(name.Name, node.Tok.String(), value)
								}
							}
						}
					}
				}
			}
		}
	}
	sort.Slice(declarations, func(i, j int) bool {
		if declarations[i].ID == declarations[j].ID {
			return declarations[i].Source < declarations[j].Source
		}
		return declarations[i].ID < declarations[j].ID
	})
	compareReference(t, "sdk", declarations)
}

func TestMigrationMCPContractKeepsAllToolSchemas(t *testing.T) {
	for _, service := range []struct {
		name   string
		server *mcp.Server
	}{
		{name: "pixiv", server: pixivmcp.New(nil, nil)},
		{name: "fanbox", server: fanboxmcp.New(fanboxmcp.SDKPorts{})},
	} {
		t.Run(service.name, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			clientTransport, serverTransport := mcp.NewInMemoryTransports()
			serverDone := make(chan error, 1)
			go func() { serverDone <- service.server.Run(ctx, serverTransport) }()
			client := mcp.NewClient(&mcp.Implementation{Name: "migration-contract", Version: "0"}, nil)
			session, err := client.Connect(ctx, clientTransport, nil)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() {
				_ = session.Close()
				cancel()
				select {
				case <-serverDone:
				case <-time.After(5 * time.Second):
					t.Error("MCP server did not stop")
				}
			})
			tools := make([]*mcp.Tool, 0)
			for tool, err := range session.Tools(ctx, nil) {
				if err != nil {
					t.Fatal(err)
				}
				tools = append(tools, tool)
			}
			sort.Slice(tools, func(i, j int) bool { return tools[i].Name < tools[j].Name })
			compareReference(t, "mcp-"+service.name, tools)
		})
	}
}

func TestMigrationLedgerTracksEveryFrozenPublicContract(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	read := func(path string, target any) {
		t.Helper()
		content, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(content, target); err != nil {
			t.Fatal(err)
		}
	}
	expected := map[string]bool{}
	var declarations []sdkDeclaration
	read("docs/migration/reference/sdk.json", &declarations)
	for _, declaration := range declarations {
		expected[declaration.ID] = true
	}
	cliFiles, err := filepath.Glob(filepath.Join(root, "docs", "migration", "reference", "cli.*.json"))
	if err != nil || len(cliFiles) == 0 {
		t.Fatalf("CLI snapshots missing: %v", err)
	}
	for _, path := range cliFiles {
		var commands []struct {
			Path string `json:"path"`
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			t.Fatal(err)
		}
		read(relative, &commands)
		for _, command := range commands {
			expected["cli:"+command.Path] = true
		}
	}
	for _, service := range []string{"pixiv", "fanbox"} {
		var tools []*mcp.Tool
		read("docs/migration/reference/mcp-"+service+".json", &tools)
		for _, tool := range tools {
			expected["mcp:"+service+":"+tool.Name] = true
		}
	}
	var ledger struct {
		ReferenceCommit string `json:"reference_commit"`
		Entries         []struct {
			ID                string   `json:"id"`
			Status            string   `json:"status"`
			RustSources       []string `json:"rust_sources"`
			Tests             []string `json:"tests"`
			VerifiedPlatforms []string `json:"verified_platforms"`
			Differences       []string `json:"differences"`
		} `json:"entries"`
	}
	read("docs/migration/ledger.json", &ledger)
	if ledger.ReferenceCommit != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("ledger reference commit changed; review the baseline migration")
	}
	seen := map[string]bool{}
	for _, entry := range ledger.Entries {
		if seen[entry.ID] || !expected[entry.ID] {
			t.Errorf("duplicate or unknown contract: %s", entry.ID)
		}
		seen[entry.ID] = true
		switch entry.Status {
		case "pending", "in_progress":
		case "verified":
			if len(entry.RustSources) == 0 || len(entry.Tests) == 0 || len(entry.VerifiedPlatforms) == 0 || len(entry.Differences) != 0 {
				t.Errorf("verified contract lacks evidence or has unresolved differences: %s", entry.ID)
			}
		default:
			t.Errorf("invalid migration status for %s: %s", entry.ID, entry.Status)
		}
		for _, path := range append(append([]string{}, entry.RustSources...), entry.Tests...) {
			file := strings.SplitN(path, "#", 2)[0]
			if filepath.IsAbs(file) || strings.Contains(file, "..") {
				t.Errorf("evidence must use repository-relative paths: %s", path)
				continue
			}
			if _, err := os.Stat(filepath.Join(root, filepath.FromSlash(file))); err != nil {
				t.Errorf("missing evidence for %s: %s", entry.ID, path)
			}
		}
	}
	for id := range expected {
		if !seen[id] {
			t.Errorf("public contract missing from migration ledger: %s", id)
		}
	}
}
