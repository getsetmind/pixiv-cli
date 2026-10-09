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
	"regexp"
	"strings"
	"testing"
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPRanking = flag.Bool("migration-update-mcp-ranking", false, "capture artwork ranking MCP tool schema and results")

func TestMigrationMCPArtworkRankingPreservesFiltersPagesAndSchema(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "artwork-ranking.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Name   string            `json:"name"`
		Mode   string            `json:"mode"`
		Date   string            `json:"date"`
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Input   map[string]any        `json:"input"`
		Bodies  []json.RawMessage     `json:"bodies"`
		Result  json.RawMessage       `json:"result"`
		Queries []map[string][]string `json:"queries"`
	}
	fixture := struct {
		Schema json.RawMessage `json:"schema"`
		Cases  []row           `json:"cases"`
	}{Cases: []row{}}
	type candidate struct {
		Input  map[string]any
		Bodies []json.RawMessage
	}
	var candidates []candidate
	modes := map[string]bool{}
	for _, mode := range []string{"", "day", "day_male", "day_female", "week", "week_original", "week_rookie", "month", "day_manga", "week_manga", "month_manga", "week_rookie_manga", "day_r18", "day_male_r18", "day_female_r18", "week_r18", "week_r18g"} {
		modes[mode] = true
	}
	var base []json.RawMessage
	for _, source := range sources {
		if source.Name == "mode:" {
			base = source.Bodies
		}
		if !modes[source.Mode] || strings.HasPrefix(source.Name, "cursor:") || strings.HasPrefix(source.Name, "binding:") {
			continue
		}
		if source.Date != "" && !regexp.MustCompile(`^[0-9]{4}-[0-9]{2}-[0-9]{2}$`).MatchString(source.Date) {
			continue
		}
		input := map[string]any{}
		if source.Mode != "" {
			input["mode"] = source.Mode
		}
		if source.Date != "" {
			input["date"] = source.Date
		}
		candidates = append(candidates, candidate{input, source.Bodies})
	}
	for _, input := range []map[string]any{
		{"limit": 0}, {"limit": 1}, {"limit": 2, "page": 2}, {"page": 1}, {"page": 2, "limit": 0}, {"page": 4000000000000000000, "limit": 3},
		{"limit": 2, "illust_filter": map[string]any{"type": "manga"}},
		{"limit": 0, "illust_filter": map[string]any{"tags": []string{"missing"}}},
		{"illust_filter": map[string]any{"id": 1}},
		{"illust_filter": map[string]any{"min_views": 20, "min_pages": 1}},
	} {
		candidates = append(candidates, candidate{input, base})
	}
	for _, source := range candidates {
		current := row{Input: source.Input, Bodies: source.Bodies, Queries: []map[string][]string{}}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
			current.Queries = append(current.Queries, req.URL.Query())
			if req.Method != "GET" || req.URL.Path != "/v1/illust/ranking" {
				t.Fatal("unexpected ranking MCP request")
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(func() json.RawMessage {
				if len(source.Bodies) > 1 && req.URL.Query().Get("offset") != "" {
					return source.Bodies[1]
				}
				return source.Bodies[0]
			}())), Request: req}, nil
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
				if tool.Name == "illust_ranking" {
					fixture.Schema, err = json.Marshal(tool)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
			if fixture.Schema == nil {
				t.Fatal("missing ranking MCP schema")
			}
		}
		if _, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "illust_ranking", Arguments: source.Input}); err != nil {
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
			t.Fatal("ranking MCP session did not stop")
		}
		fixture.Cases = append(fixture.Cases, current)
	}
	data, err = json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "mcp-ranking.json")
	if *updateMCPRanking {
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
		t.Fatal("ranking MCP differs from Go reference")
	}
}
