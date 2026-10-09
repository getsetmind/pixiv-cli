package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPRecommended = flag.Bool("migration-update-mcp-recommended", false, "capture fixed Go MCP detail contracts")

func TestMigrationMCPRecommendedMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "crates", "pixiv-mcp", "tests", "fixtures")
	type row struct {
		Name      string            `json:"name"`
		ToolName  string            `json:"tool_name"`
		Arguments map[string]any    `json:"arguments"`
		Bodies    []json.RawMessage `json:"bodies"`
		Result    json.RawMessage   `json:"result"`
		Calls     int               `json:"calls"`
		Requests  int               `json:"requests"`
		Queries   []string          `json:"queries"`
		Paths     []string          `json:"paths"`
		RPCError  string            `json:"rpc_error"`
	}
	var rows []row
	art := func(id int, kind string, tags string, views, pages int) string {
		return fmt.Sprintf(`{"id":%d,"type":%q,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"page_count":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, kind, tags, views, pages)
	}
	nov := func(id, views int, tags string) string {
		return fmt.Sprintf(`{"id":%d,"title":"novel","user":{"id":7},"tags":%s,"total_view":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, tags, views)
	}
	usr := func(id int) string {
		return fmt.Sprintf(`{"user":{"id":%d,"name":"fixture"},"illusts":[],"novels":[],"is_muted":false}`, id)
	}
	firstArt := json.RawMessage(fmt.Sprintf(`{"illusts":[%s,%s,%s],"next_url":"https://app-api.pixiv.net/v1/illust/recommended?offset=30&viewed[0]=22"}`, art(22, "illust", `[{"name":"cat"},{"name":"blue"}]`, 100, 2), art(23, "manga", `[{"name":"cat"}]`, 50, 1), art(24, "ugoira", `[{"name":"cat"},{"name":"blue"}]`, 200, 1)))
	lastArt := json.RawMessage(fmt.Sprintf(`{"illusts":[%s,%s,%s],"next_url":null}`, art(22, "illust", `[{"name":"cat"},{"name":"blue"}]`, 100, 2), art(22, "manga", `[{"name":"cat"},{"name":"blue"}]`, 120, 3), art(25, "illust", `[{"name":"cat"},{"name":"blue"}]`, 300, 2)))
	firstNov := json.RawMessage(fmt.Sprintf(`{"novels":[%s,%s],"next_url":"https://app-api.pixiv.net/v1/novel/recommended?offset=30"}`, nov(32, 20, `[{"name":"cat"}]`), nov(33, 200, `[{"name":"cat"},{"name":"blue"}]`)))
	lastNov := json.RawMessage(fmt.Sprintf(`{"novels":[%s,%s],"next_url":null}`, nov(33, 200, `[{"name":"cat"},{"name":"blue"}]`), nov(34, 300, `[{"name":"cat"},{"name":"blue"}]`)))
	firstUser := json.RawMessage(fmt.Sprintf(`{"user_previews":[%s,%s],"next_url":"https://app-api.pixiv.net/v1/user/recommended?offset=30"}`, usr(42), usr(43)))
	lastUser := json.RawMessage(fmt.Sprintf(`{"user_previews":[%s,%s],"next_url":null}`, usr(42), usr(44)))
	for _, kind := range []string{"all", "illust", "manga", "novel", "user"} {
		add := func(name string, extra map[string]any, bodies ...json.RawMessage) {
			args := map[string]any{"kind": kind}
			for key, value := range extra {
				args[key] = value
			}
			if bodies == nil {
				if kind == "all" || kind == "illust" || kind == "manga" {
					bodies = append(bodies, firstArt, lastArt)
				}
				if kind == "all" || kind == "novel" {
					bodies = append(bodies, firstNov, lastNov)
				}
				if kind == "all" || kind == "user" {
					bodies = append(bodies, firstUser, lastUser)
				}
				// Each one-batch call needs only the first response of each selected stream.
				if _, explicit := args["limit"]; !explicit {
					bodies = nil
					if kind == "all" || kind == "illust" || kind == "manga" {
						bodies = append(bodies, firstArt)
					}
					if kind == "all" || kind == "novel" {
						bodies = append(bodies, firstNov)
					}
					if kind == "all" || kind == "user" {
						bodies = append(bodies, firstUser)
					}
				}
				if limit, ok := args["limit"].(int); ok && limit > 0 && kind == "all" && (args["page"] == nil || args["page"] == 2) && limit <= 2 {
					bodies = []json.RawMessage{firstArt, firstNov, firstUser}
				}
			}
			rows = append(rows, row{Name: kind + "-" + name, ToolName: "recommended", Arguments: args, Bodies: bodies})
		}
		add("default", nil)
		for index, args := range []map[string]any{{"limit": 0}, {"limit": 1}, {"page": 2, "limit": 1}, {"page": 9, "limit": 1}, {"page": 2}, {"page": 1, "limit": 0}, {"page": 0}, {"limit": -1}, {"page": nil}, {"limit": nil}, {"page": 1.5}, {"limit": "2"}, {"extra": true}, {"limit": int64(9223372036854775807)}, {"page": int64(9223372036854775807), "limit": 2}, {"kind": "wrong"}, {"kind": nil}, {"kind": 1}} {
			add(fmt.Sprintf("argument-%d", index), args)
		}
		for _, filterKind := range []string{"illust", "novel", "user"} {
			for index, filter := range []any{nil, map[string]any{}, map[string]any{"id": 22}, map[string]any{"id": 0}, map[string]any{"id": nil}, map[string]any{"id": 1.5}, map[string]any{"extra": true}} {
				add(fmt.Sprintf("%s-filter-%d", filterKind, index), map[string]any{filterKind + "_filter": filter, "limit": 0})
			}
		}
		if kind == "illust" || kind == "manga" || kind == "all" {
			for index, filter := range []any{map[string]any{"type": "illust"}, map[string]any{"type": "manga"}, map[string]any{"type": "ugoira"}, map[string]any{"tags": []string{"cat", "blue"}}, map[string]any{"min_views": 100}, map[string]any{"min_pages": 2}, map[string]any{"type": "wrong"}, map[string]any{"tags": nil}, map[string]any{"min_views": nil}, map[string]any{"min_pages": nil}, map[string]any{"type": nil}, map[string]any{"min_views": -1}, map[string]any{"min_pages": -1}} {
				add(fmt.Sprintf("visual-filter-%d", index), map[string]any{"illust_filter": filter, "limit": 0})
			}
		}
		if kind == "novel" {
			for index, filter := range []any{map[string]any{"tags": []string{"cat", "blue"}}, map[string]any{"min_views": 100}, map[string]any{"tags": nil}, map[string]any{"min_views": nil}, map[string]any{"min_views": -1}} {
				add(fmt.Sprintf("novel-filter-extra-%d", index), map[string]any{"novel_filter": filter, "limit": 0})
			}
			add("filter-before-window", map[string]any{"novel_filter": map[string]any{"tags": []string{"cat", "blue"}}, "page": 2, "limit": 1})
		}
		if kind == "user" {
			add("filter-before-window", map[string]any{"user_filter": map[string]any{"id": 44}, "limit": 1})
		}
		if kind == "illust" || kind == "manga" {
			add("raw-before-subtype", map[string]any{"page": 2, "limit": 1})
			add("raw-before-filter", map[string]any{"illust_filter": map[string]any{"min_views": 100}, "limit": 1})
		}
		if kind != "all" {
			var first, last json.RawMessage
			key := "illusts"
			endpoint := "illust"
			switch kind {
			case "novel":
				first, last, key, endpoint = firstNov, lastNov, "novels", "novel"
			case "user":
				first, last, key, endpoint = firstUser, lastUser, "user_previews", "user"
			default:
				first, last = firstArt, lastArt
			}
			empty := json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net/v1/%s/recommended?offset=30"}`, key, endpoint))
			add("empty-batch-stop", nil, empty, last)
			add("empty-batch-refill", map[string]any{"limit": 1}, empty, last)
			add("terminal-truncation", map[string]any{"limit": 1}, last)
			add("malformed", nil, json.RawMessage(`{}`))
			add("later-malformed", map[string]any{"limit": 0}, first, json.RawMessage(`{}`))
		}
	}
	rows = append(rows, row{Name: "missing-kind", ToolName: "recommended", Arguments: map[string]any{}})
	rows = append(rows, row{Name: "cross-feed-novel-failure", ToolName: "recommended", Arguments: map[string]any{"kind": "all"}, Bodies: []json.RawMessage{firstArt, json.RawMessage(`{}`)}})
	rows = append(rows, row{Name: "cross-feed-user-failure", ToolName: "recommended", Arguments: map[string]any{"kind": "all"}, Bodies: []json.RawMessage{firstArt, firstNov, json.RawMessage(`{}`)}})
	for _, nested := range []struct {
		Filter string
		Field  string
	}{{"illust_filter", "id"}, {"illust_filter", "min_views"}, {"illust_filter", "min_pages"}, {"novel_filter", "id"}, {"novel_filter", "min_views"}, {"user_filter", "id"}} {
		rows = append(rows, row{Name: "nested-overflow-" + nested.Filter + "-" + nested.Field, ToolName: "recommended", Arguments: map[string]any{"kind": "all", nested.Filter: map[string]any{nested.Field: int64(9223372036854775807)}}})
	}
	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			bodyIndex := row.Requests
			row.Requests++
			row.Queries = append(row.Queries, req.URL.RawQuery)
			row.Paths = append(row.Paths, req.URL.Path)
			if bodyIndex >= len(row.Bodies) {
				t.Fatalf("unexpected HTTP request %d", row.Requests)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			row.Calls++
			_, err := invoke(ctx, client)
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		clientTransport, serverTransport := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, serverTransport) }()
		capture := &migrationMCPCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationMCPCaptureTransport{clientTransport, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if index == 0 {
			listing, err := session.ListTools(context.Background(), nil)
			if err != nil {
				t.Fatal(err)
			}
			for _, item := range listing.Tools {
				if item.Name == "recommended" {
					tools[item.Name], err = json.Marshal(item)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
		}
		_, callErr := session.CallTool(context.Background(), &mcp.CallToolParams{Name: row.ToolName, Arguments: row.Arguments})
		if callErr != nil {
			row.RPCError = callErr.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		if row.Name == "all-default" || row.Name == "all-argument-2" || row.Name == "all-visual-filter-2" || row.Name == "novel-filter-before-window" || row.Name == "user-filter-before-window" {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Records []struct {
						ID   string `json:"id"`
						Type string `json:"type"`
					} `json:"records"`
					Pagination map[string]struct {
						Returned int `json:"returned"`
					} `json:"pagination"`
				} `json:"structuredContent"`
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			if result.IsError || row.RPCError != "" {
				t.Fatalf("intended success %s failed: %s", row.Name, row.Result)
			}
			switch row.Name {
			case "all-default":
				if len(result.Structured.Records) != 6 || row.Requests != 3 {
					t.Fatalf("one visual fetch and separate novel/user batches were not preserved: %s requests=%d", row.Result, row.Requests)
				}
			case "all-argument-2":
				if len(result.Structured.Records) != 3 || result.Structured.Pagination["illust"].Returned != 0 || result.Structured.Pagination["manga"].Returned != 1 || row.Requests != 3 {
					t.Fatalf("raw artwork logical window was not preserved: %s", row.Result)
				}
			case "all-visual-filter-2":
				if result.Structured.Pagination["illust"].Returned != 1 || result.Structured.Pagination["manga"].Returned != 1 || result.Structured.Records[0].ID != result.Structured.Records[1].ID {
					t.Fatalf("explicit visual type was not reused across sections: %s", row.Result)
				}
			case "novel-filter-before-window":
				if len(result.Structured.Records) != 1 || result.Structured.Records[0].ID != "34" || row.Requests != 2 {
					t.Fatalf("novel filter did not precede logical page: %s", row.Result)
				}
			case "user-filter-before-window":
				if len(result.Structured.Records) != 1 || result.Structured.Records[0].ID != "44" || row.Requests != 2 {
					t.Fatalf("user filter did not precede logical limit: %s", row.Result)
				}
			}
		}
		_ = session.Close()
		cancel()
	}
	if len(tools) != 1 {
		t.Fatal("artwork feed tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-recommended.json")
	if *migrationUpdateMCPRecommended {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP detail differs from fixed Go reference")
	}
}
