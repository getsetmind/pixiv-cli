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
	"strings"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPMyPixiv = flag.Bool("migration-update-mcp-mypixiv", false, "capture fixed Go MCP detail contracts")

func TestMigrationMCPMyPixivMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		UnknownIdentity bool              `json:"unknown_identity,omitempty"`
		Name            string            `json:"name"`
		ToolName        string            `json:"tool_name"`
		Arguments       any               `json:"arguments"`
		Bodies          []json.RawMessage `json:"-"`
		BodyKeys        []string          `json:"body_keys"`
		Result          json.RawMessage   `json:"result"`
		Calls           int               `json:"calls"`
		Requests        int               `json:"requests"`
		Queries         []string          `json:"queries"`
		RPCError        string            `json:"rpc_error"`
	}
	var rows []row
	art := func(id int, kind string, tags string, views, pages int) string {
		return fmt.Sprintf(`{"id":%d,"type":%q,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"page_count":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, kind, tags, views, pages)
	}
	for _, toolName := range []string{"mypixiv_illusts", "mypixiv_novels", "mypixiv_users"} {
		artwork := strings.Contains(toolName, "illust")
		users := toolName == "mypixiv_users"
		version := "v1"
		if artwork {
			version = "v2"
		}
		entity, field := "novel", "novels"
		if artwork {
			entity, field = "illust", "illusts"
		}
		if users {
			entity, field = "user", "user_previews"
		}
		base := map[string]any{}
		query := "offset=30"
		if users {
			query += "&user_id=42"
		}
		item := func(id int, kind, tags string, views, pages int) string {
			if users {
				return fmt.Sprintf(`{"user":{"id":%d,"name":"fixture","account":"artist"},"illusts":[],"novels":[],"is_muted":false}`, id)
			}
			if artwork {
				return art(id, kind, tags, views, pages)
			}
			return fmt.Sprintf(`{"id":%d,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, tags, views)
		}
		first := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s,%s],"next_url":"https://app-api.pixiv.net/%s/%s/mypixiv?%s"}`, field, item(22, "illust", `[{"name":"cat"},{"name":"blue"}]`, 100, 2), item(22, "illust", `[{"name":"cat"}]`, 50, 1), item(23, "manga", `[{"name":"cat"}]`, 50, 1), version, entity, query))
		second := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s,%s],"next_url":null}`, field, item(22, "illust", `[{"name":"cat"}]`, 50, 1), item(22, "manga", `[{"name":"cat"},{"name":"blue"}]`, 120, 3), item(24, "ugoira", `[{"name":"cat"},{"name":"blue"}]`, 200, 1)))
		add := func(name string, extra map[string]any, bodies ...json.RawMessage) {
			args := map[string]any{}
			for key, value := range base {
				args[key] = value
			}
			for key, value := range extra {
				args[key] = value
			}
			if bodies == nil {
				bodies = []json.RawMessage{first, second}
			}
			rows = append(rows, row{Name: toolName + "-" + name, ToolName: toolName, Arguments: args, Bodies: bodies})
		}
		for index, args := range []map[string]any{{}, {"extra": true}, {"page": 2}, {"limit": 1}, {"limit": -1}, {"page": 0}, {"limit": 0}, {"page": 2, "limit": 1}, {"page": 1.5}, {"limit": "2"}, {"page": nil}, {"limit": nil}, {"limit": int64(9223372036854775807)}, {"page": int64(9223372036854775807), "limit": 2}} {
			add(fmt.Sprintf("argument-%d", index), args)
		}
		filterName := "novel_filter"
		filters := []any{nil, map[string]any{}, map[string]any{"id": 22}, map[string]any{"tags": []string{"cat", "blue"}}, map[string]any{"min_views": 100}, map[string]any{"id": 0}, map[string]any{"min_views": -1}, map[string]any{"extra": 1}, map[string]any{"tags": []any{1}}, map[string]any{"id": nil}, map[string]any{"tags": nil}, map[string]any{"min_views": nil}, map[string]any{"id": 1.5}, map[string]any{"min_views": 1.5}, map[string]any{"id": int64(9223372036854775807)}}
		if artwork {
			filterName = "illust_filter"
			filters = append(filters, map[string]any{"type": "manga"}, map[string]any{"type": "wrong"}, map[string]any{"min_pages": 2}, map[string]any{"min_pages": -1}, map[string]any{"type": nil}, map[string]any{"min_pages": nil}, map[string]any{"id": 22, "type": "manga", "tags": []string{"cat", "blue"}, "min_views": 100, "min_pages": 2})
		}
		if users {
			filterName = "user_filter"
			filters = []any{nil, map[string]any{}, map[string]any{"id": 22}, map[string]any{"id": 0}, map[string]any{"extra": 1}, map[string]any{"id": nil}, map[string]any{"id": 1.5}, map[string]any{"id": int64(9223372036854775807)}}
		}
		for index, filter := range filters {
			add(fmt.Sprintf("filter-%d", index), map[string]any{filterName: filter, "limit": 0})
		}
		if users {
			add("filter-before-logical-pagination", map[string]any{filterName: map[string]any{"id": 22}, "page": 2, "limit": 1})
		} else {
			add("filter-before-logical-pagination", map[string]any{filterName: map[string]any{"tags": []string{"cat", "blue"}}, "page": 2, "limit": 1})
		}
		if !users {
			add("filtered-stop", map[string]any{filterName: map[string]any{"min_views": 120}})
			add("filtered-refill", map[string]any{filterName: map[string]any{"min_views": 120}, "limit": 1})
		}
		empty := json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net/%s/%s/mypixiv?%s"}`, field, version, entity, query))
		add("empty-stop", nil, empty, second)
		add("empty-refill", map[string]any{"limit": 1}, empty, second)
		add("terminal-truncation", map[string]any{"limit": 1}, second)
		add("offset-past-end", map[string]any{"page": 9, "limit": 1})
		add("missing-list", nil, json.RawMessage(`{}`))
		add("null-list", nil, json.RawMessage(fmt.Sprintf(`{"%s":null}`, field)))
		add("later-malformed", map[string]any{"limit": 0}, first, json.RawMessage(`{}`))
		add("later-invalid-record", map[string]any{"limit": 0}, first, json.RawMessage(fmt.Sprintf(`{"%s":[{"id":0,"user":{"id":0}}]}`, field)))
		for index, args := range []any{nil, 7, "invalid", []any{}, true} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-root-%d", toolName, index), ToolName: toolName, Arguments: args, Bodies: []json.RawMessage{first, second}})
		}
	}
	rows = append(rows, row{Name: "mypixiv_users-unknown-identity", ToolName: "mypixiv_users", Arguments: map[string]any{}, Bodies: []json.RawMessage{}, UnknownIdentity: true})
	rows = append(rows, row{Name: "mypixiv_users-external-user-id-rejected", ToolName: "mypixiv_users", Arguments: map[string]any{"user_id": 7}, Bodies: []json.RawMessage{}})
	sharedBodies := map[string]json.RawMessage{}
	bodyIndex := map[string]string{}
	for i := range rows {
		rows[i].BodyKeys = []string{}
		for _, body := range rows[i].Bodies {
			key, ok := bodyIndex[string(body)]
			if !ok {
				key = fmt.Sprintf("body-%02d", len(sharedBodies))
				bodyIndex[string(body)] = key
				sharedBodies[key] = body
			}
			rows[i].BodyKeys = append(rows[i].BodyKeys, key)
		}
	}
	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, _, err := pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(func() string {
					return `{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}`
				}())), Request: req}, nil
			}
			bodyIndex := row.Requests
			row.Requests++
			row.Queries = append(row.Queries, req.URL.RawQuery)
			if bodyIndex >= len(row.Bodies) {
				return nil, fmt.Errorf("unexpected HTTP request %d", row.Requests)
			}
			version := "v1"
			entity := "novel"
			if strings.Contains(row.ToolName, "illust") {
				entity = "illust"
				version = "v2"
			}
			if row.ToolName == "mypixiv_users" {
				entity = "user"
			}
			if req.Method != "GET" || req.URL.Path != "/"+version+"/"+entity+"/mypixiv" || req.Header.Get("Authorization") != "Bearer fixture-access" {
				return nil, fmt.Errorf("unexpected request %s", req.URL)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		if row.UnknownIdentity {
			client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
				row.Requests++
				return nil, fmt.Errorf("unexpected request")
			})}})
			if err != nil {
				t.Fatal(err)
			}
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
				if strings.HasPrefix(item.Name, "mypixiv_") {
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
		if strings.HasSuffix(row.Name, "argument-0") || strings.HasSuffix(row.Name, "argument-6") || (row.ToolName != "mypixiv_users" && strings.HasSuffix(row.Name, "filter-before-logical-pagination")) || strings.HasSuffix(row.Name, "empty-refill") || strings.HasSuffix(row.Name, "filtered-refill") {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Records []json.RawMessage `json:"records"`
				} `json:"structuredContent"`
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			wantRequests := 2
			if strings.HasSuffix(row.Name, "argument-0") {
				wantRequests = 1
			}
			if result.IsError || len(result.Structured.Records) == 0 || row.Requests != wantRequests {
				t.Fatalf("success scenario %s result=%s requests=%d", row.Name, row.Result, row.Requests)
			}
		}
		_ = session.Close()
		cancel()
	}
	if len(tools) != 3 {
		t.Fatal("timeline tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Bodies map[string]json.RawMessage `json:"bodies"`
		Tools  map[string]json.RawMessage `json:"tools"`
		Cases  []row                      `json:"cases"`
	}{sharedBodies, tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-mypixiv.json")
	if *migrationUpdateMCPMyPixiv {
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
