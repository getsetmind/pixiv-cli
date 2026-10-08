package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPProxy = flag.Bool("migration-update-mcp-proxy", false, "capture MCP proxy startup and tool invocation contracts")

func TestMigrationMCPProxyFlagsPreserveStartupAndToolOverrides(t *testing.T) {
	oldCleanup, oldSupported, oldStdio := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, runMCPStdio
	t.Cleanup(func() {
		cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, runMCPStdio = oldCleanup, oldSupported, oldStdio
	})
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Name     string            `json:"name"`
		Config   string            `json:"config"`
		Flags    []string          `json:"flags"`
		Results  []json.RawMessage `json:"results"`
		Stdout   string            `json:"stdout"`
		Stderr   string            `json:"stderr"`
		Exit     int               `json:"exit"`
		Database bool              `json:"database"`
	}
	configured := "[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n"
	inputs := []row{
		{Name: "configured_invalid", Config: configured, Flags: []string{}},
		{Name: "clear_configured", Config: configured, Flags: []string{"--no-proxy"}},
		{Name: "explicit_empty", Config: configured, Flags: []string{"--proxy="}},
		{Name: "explicit_override", Config: configured, Flags: []string{"--proxy=http://override.invalid"}},
		{Name: "no_proxy_false", Config: configured, Flags: []string{"--no-proxy=false"}},
		{Name: "invalid_override", Flags: []string{"--proxy=ftp://fixture-user:fixture-secret@override.invalid"}},
		{Name: "conflict", Flags: []string{"--proxy=", "--no-proxy"}},
		{Name: "conflict_false", Flags: []string{"--proxy=", "--no-proxy=false"}},
		{Name: "invalid_config_before_conflict", Config: "[unfinished\n", Flags: []string{"--proxy=", "--no-proxy"}},
		{Name: "last_proxy_wins", Config: configured, Flags: []string{"--proxy=ftp://override.invalid", "--proxy="}},
	}
	rows := []row{}
	for _, input := range inputs {
		t.Run(input.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("HTTPS_PROXY", "")
			t.Setenv("REQUEST_INTERVAL", "0")
			directory := filepath.Join(home, ".pixiv-cli")
			if err := os.MkdirAll(directory, 0700); err != nil {
				t.Fatal(err)
			}
			path := filepath.Join(directory, "config.toml")
			if err := os.WriteFile(path, []byte(input.Config), 0600); err != nil {
				t.Fatal(err)
			}
			input.Results = []json.RawMessage{}
			runMCPStdio = func(ctx context.Context, server *mcp.Server) error {
				ctx, cancel := context.WithCancel(ctx)
				defer cancel()
				clientTransport, serverTransport := mcp.NewInMemoryTransports()
				done := make(chan error, 1)
				go func() { done <- server.Run(ctx, serverTransport) }()
				capture := &accountRPCCapture{}
				session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, accountRPCTransport{clientTransport, capture}, nil)
				if err != nil {
					return err
				}
				for _, tool := range []string{"illust_detail", "search_illust"} {
					args := map[string]any{"illust_id": 42}
					if tool == "search_illust" {
						args = map[string]any{"word": "fixture"}
					}
					_, _ = session.CallTool(ctx, &mcp.CallToolParams{Name: tool, Arguments: args})
					capture.mu.Lock()
					input.Results = append(input.Results, append(json.RawMessage(nil), capture.result...))
					capture.mu.Unlock()
				}
				_ = session.Close()
				cancel()
				select {
				case <-done:
					return nil
				case <-time.After(5 * time.Second):
					t.Fatal("MCP proxy session did not stop")
					return nil
				}
			}
			var out, diagnostics bytes.Buffer
			input.Exit = Run(append([]string{"pixiv", "mcp"}, input.Flags...), strings.NewReader(""), &out, &diagnostics)
			input.Stdout, input.Stderr = out.String(), diagnostics.String()
			if strings.Contains(input.Stderr, "fixture-secret") {
				t.Fatal("proxy credential exposed")
			}
			after, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(after, []byte(input.Config)) {
				t.Fatal("MCP proxy override changed configuration")
			}
			_, err = os.Stat(filepath.Join(directory, "pixiv-cli.db"))
			if err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			input.Database = err == nil
			rows = append(rows, input)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "mcp-proxy.json")
	if *updateMCPProxy {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP proxy behavior differs from fixed Go reference")
	}
}
