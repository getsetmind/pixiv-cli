package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPTrending = flag.Bool("migration-update-mcp-trending", false, "capture trending MCP tool schema and results")

func TestMigrationMCPTrendingTagsPreservesSchemaTextAndSamples(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "trending-tags.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Body json.RawMessage `json:"body"`
		Args []string        `json:"args"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Body     json.RawMessage `json:"body"`
		Result   json.RawMessage `json:"result"`
		Requests int             `json:"requests"`
	}
	fixture := struct {
		Schema json.RawMessage `json:"schema"`
		Cases  []row           `json:"cases"`
	}{Cases: []row{}}
	for _, source := range sources {
		if len(source.Args) != 1 {
			continue
		}
		current := row{Body: source.Body}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
			current.Requests++
			if req.URL.Path != "/v1/trending-tags/illust" || len(req.URL.Query()) != 0 {
				t.Fatal("unexpected trending MCP request")
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(source.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			_, err := invoke(ctx, client)
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		peer, local := mcp.NewInMemoryTransports()
		done := make(chan error, 1)
		go func() { done <- server.Run(ctx, local) }()
		capture := &accountRPCCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, accountRPCTransport{peer, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if fixture.Schema == nil {
			tools, err := session.ListTools(ctx, &mcp.ListToolsParams{})
			if err != nil {
				t.Fatal(err)
			}
			for _, tool := range tools.Tools {
				if tool.Name == "trending_tags_illust" {
					fixture.Schema, err = json.Marshal(tool)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
			if fixture.Schema == nil {
				t.Fatal("missing trending MCP schema")
			}
		}
		if _, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "trending_tags_illust", Arguments: map[string]any{}}); err != nil {
			t.Fatal(err)
		}
		capture.mu.Lock()
		current.Result = append(json.RawMessage(nil), capture.result...)
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("trending MCP session did not stop")
		}
		fixture.Cases = append(fixture.Cases, current)
	}
	data, err = json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "mcp-trending.json")
	if *updateMCPTrending {
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
		t.Fatal("trending MCP differs from Go reference")
	}
}
