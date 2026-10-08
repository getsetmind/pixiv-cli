package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPAccounts = flag.Bool("migration-update-mcp-accounts", false, "capture MCP saved account failure results")

type accountRPCCapture struct {
	mu     sync.Mutex
	result json.RawMessage
}
type accountRPCTransport struct {
	mcp.Transport
	capture *accountRPCCapture
}
type accountRPCConnection struct {
	mcp.Connection
	capture *accountRPCCapture
}

func (t accountRPCTransport) Connect(ctx context.Context) (mcp.Connection, error) {
	connection, err := t.Transport.Connect(ctx)
	if err != nil {
		return nil, err
	}
	return accountRPCConnection{connection, t.capture}, nil
}
func (c accountRPCConnection) Read(ctx context.Context) (jsonrpc.Message, error) {
	message, err := c.Connection.Read(ctx)
	if err == nil {
		if _, ok := message.(*jsonrpc.Response); ok {
			data, err := jsonrpc.EncodeMessage(message)
			if err != nil {
				return nil, err
			}
			var response struct {
				Result json.RawMessage `json:"result"`
			}
			if err := json.Unmarshal(data, &response); err != nil {
				return nil, err
			}
			c.capture.mu.Lock()
			c.capture.result = response.Result
			c.capture.mu.Unlock()
		}
	}
	return message, err
}

func TestMigrationMCPUsesSavedAccountErrorsThroughRealComposition(t *testing.T) {
	type row struct {
		Name      string          `json:"name"`
		Config    string          `json:"config"`
		Tool      string          `json:"tool"`
		Arguments any             `json:"arguments"`
		Result    json.RawMessage `json:"result"`
	}
	inputs := []row{
		{Name: "no_account"},
		{Name: "missing_default", Config: "[pixiv.auth]\ndefault_user_id = 43\n"},
		{Name: "invalid_default", Config: "[pixiv.auth]\ndefault_user_id = 0\n"},
		{Name: "empty_pool", Config: "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n"},
	}
	rows := []row{}
	for _, input := range inputs {
		for _, tool := range []string{"illust_detail", "search_illust"} {
			t.Run(input.Name+"_"+tool, func(t *testing.T) {
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				directory := filepath.Join(home, ".pixiv-cli")
				if err := os.MkdirAll(directory, 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(directory, "config.toml"), []byte(input.Config), 0600); err != nil {
					t.Fatal(err)
				}
				app := app{closeState: &closeState{}}
				ports, err := app.newPixivSDKPorts()
				if err != nil {
					t.Fatal(err)
				}
				server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{Execute: func(ctx context.Context, account pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					return ports.run(ctx, pixivdeps.Request{UserID: account.UserID, HTTPSProxyOverride: account.HTTPSProxyOverride}, invoke)
				}}, pixivmcp.Account{})
				ctx, cancel := context.WithCancel(context.Background())
				clientTransport, serverTransport := mcp.NewInMemoryTransports()
				done := make(chan error, 1)
				go func() { done <- server.Run(ctx, serverTransport) }()
				capture := &accountRPCCapture{}
				session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, accountRPCTransport{clientTransport, capture}, nil)
				if err != nil {
					cancel()
					t.Fatal(err)
				}
				input.Tool = tool
				input.Arguments = map[string]any{"illust_id": 42}
				if tool == "search_illust" {
					input.Arguments = map[string]any{"word": "fixture"}
				}
				_, _ = session.CallTool(ctx, &mcp.CallToolParams{Name: tool, Arguments: input.Arguments})
				capture.mu.Lock()
				input.Result = append(json.RawMessage(nil), capture.result...)
				capture.mu.Unlock()
				if len(input.Result) == 0 {
					t.Fatal("MCP account call returned no wire result")
				}
				_ = session.Close()
				cancel()
				select {
				case <-done:
				case <-time.After(5 * time.Second):
					t.Fatal("MCP account session did not stop")
				}
				if err := app.closeState.close(); err != nil {
					t.Fatal(err)
				}
				rows = append(rows, input)
			})
		}
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "mcp-accounts.json")
	if *updateMCPAccounts {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP account errors differ from Go reference")
	}
}
