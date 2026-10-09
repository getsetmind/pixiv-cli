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

var migrationUpdateMCPBookmarkLists = flag.Bool("migration-update-mcp-bookmark-lists", false, "capture fixed Go MCP bookmark lists contracts")

func TestMigrationMCPBookmarkListsMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		NoIdentity bool              `json:"no_identity,omitempty"`
		Name       string            `json:"name"`
		ToolName   string            `json:"tool_name"`
		Arguments  any               `json:"arguments"`
		Bodies     []json.RawMessage `json:"bodies"`
		Result     json.RawMessage   `json:"result"`
		Calls      int               `json:"calls"`
		Requests   int               `json:"requests"`
		Paths      []string          `json:"paths"`
		Queries    []string          `json:"queries"`
		RPCError   string            `json:"rpc_error"`
	}
	var rows []row
	art := func(id int, kind string, tags string, views, pages int) string {
		return fmt.Sprintf(`{"id":%d,"type":%q,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"page_count":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, kind, tags, views, pages)
	}
	for _, toolName := range []string{"user_bookmarks", "user_novel_bookmarks"} {
		field, endpoint := "illusts", "bookmarks/illust"
		if toolName == "user_novel_bookmarks" {
			field, endpoint = "novels", "bookmarks/novel"
		}
		item := func(id int, kind string, tags string, views, pages int) string {
			if endpoint == "bookmarks/illust" {
				return art(id, kind, tags, views, pages)
			}
			return fmt.Sprintf(`{"id":%d,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, tags, views)
		}
		first := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s,%s],"next_url":"https://app-api.pixiv.net/v1/user/%s?user_id=42&max_bookmark_id=30"}`, field, item(22, "illust", `[{"name":"cat"},{"name":"blue"}]`, 100, 2), item(22, "illust", `[{"name":"cat"}]`, 50, 1), item(23, "manga", `[{"name":"cat"}]`, 50, 1), endpoint))
		second := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s,%s],"next_url":null}`, field, item(22, "illust", `[{"name":"cat"}]`, 50, 1), item(22, "manga", `[{"name":"cat"},{"name":"blue"}]`, 120, 3), item(24, "ugoira", `[{"name":"cat"},{"name":"blue"}]`, 200, 1)))
		add := func(name string, args map[string]any, bodies ...json.RawMessage) {
			if args == nil {
				args = map[string]any{}
			}
			if bodies == nil {
				bodies = []json.RawMessage{first, second}
			}
			rows = append(rows, row{Name: toolName + "-" + name, ToolName: toolName, Arguments: args, Bodies: bodies})
		}
		for index, args := range []map[string]any{{}, {"user_id": 7}, {"extra": true}, {"user_id": 0}, {"user_id": nil}, {"user_id": -1}, {"user_id": 1.5}, {"user_id": "42"}, {"user_id": int64(9223372036854775807)}, {"page": 2}, {"limit": 1}, {"limit": -1}, {"page": 0}, {"page": -1}, {"limit": 0}, {"page": 2, "limit": 1}, {"page": 1.5}, {"limit": "2"}, {"page": nil}, {"limit": nil}, {"limit": int64(9223372036854775807)}, {"page": int64(9223372036854775807), "limit": 2}} {
			add(fmt.Sprintf("argument-%d", index), args)
		}
		if endpoint == "bookmarks/illust" {
			for index, filter := range []any{nil, map[string]any{}, map[string]any{"id": 22}, map[string]any{"tags": []string{"cat", "blue"}}, map[string]any{"min_views": 100}, map[string]any{"type": "manga"}, map[string]any{"type": "wrong"}, map[string]any{"min_pages": 2}, map[string]any{"id": 0}, map[string]any{"min_views": -1}, map[string]any{"extra": 1}, map[string]any{"tags": []any{1}}, map[string]any{"id": nil}, map[string]any{"tags": nil}, map[string]any{"id": 1.5}, map[string]any{"id": int64(9223372036854775807)}} {
				add(fmt.Sprintf("filter-%d", index), map[string]any{"illust_filter": filter, "limit": 0})
			}
			for _, limit := range []int{0, 1, 2} {
				for _, page := range []int{1, 2, 3} {
					if limit == 0 && page > 1 {
						continue
					}
					args := map[string]any{"limit": limit, "illust_filter": map[string]any{"tags": []string{"cat", "blue"}}}
					if limit > 0 {
						args["page"] = page
					}
					add(fmt.Sprintf("window-%d-%d", limit, page), args)
				}
			}
		} else {
			add("unknown-novel-filter", map[string]any{"novel_filter": map[string]any{"id": 22}})
		}
		for index, restrict := range []any{"public", "private", "wrong", "", nil, 7} {
			add(fmt.Sprintf("restrict-%d", index), map[string]any{"restrict": restrict})
		}
		for index, tag := range []any{"猫 blue", "", nil, 7} {
			add(fmt.Sprintf("tag-%d", index), map[string]any{"tag": tag})
		}

		empty := json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net/v1/user/%s?user_id=42&max_bookmark_id=30"}`, field, endpoint))
		add("empty-stop", nil, empty, second)
		add("empty-refill", map[string]any{"limit": 1}, empty, second)
		add("later-malformed", map[string]any{"limit": 0}, first, json.RawMessage(`{}`))
		add("missing-list", nil, json.RawMessage(`{}`))
		add("null-list", nil, json.RawMessage(fmt.Sprintf(`{"%s":null}`, field)))
		add("later-invalid-record", map[string]any{"limit": 0}, first, json.RawMessage(fmt.Sprintf(`{"%s":[{"id":0,"user":{"id":7}}]}`, field)))
		rows = append(rows, row{Name: toolName + "-missing-current-identity", ToolName: toolName, Arguments: map[string]any{}, Bodies: []json.RawMessage{first}, NoIdentity: true})
		add("identity-vs-page-validation", map[string]any{"user_id": -1, "page": 0})
		add("identity-vs-limit-validation", map[string]any{"user_id": -1, "limit": -1})
		add("identity-vs-offset-overflow", map[string]any{"user_id": -1, "page": int64(9223372036854775807), "limit": 2})
		add("precision-record", map[string]any{"limit": 0}, json.RawMessage(fmt.Sprintf(`{"%s":[%s]}`, field, item(9007199254740993, "illust", `[]`, 9007199254740993, 3))))
		rows = append(rows, row{Name: toolName + "-missing-identity-vs-page-validation", ToolName: toolName, Arguments: map[string]any{"page": 0}, Bodies: []json.RawMessage{first}, NoIdentity: true})
		rows = append(rows, row{Name: toolName + "-missing-identity-vs-limit-validation", ToolName: toolName, Arguments: map[string]any{"limit": -1}, Bodies: []json.RawMessage{first}, NoIdentity: true})
		if toolName == "user_bookmarks" {
			add("page-vs-filter-vs-identity-validation", map[string]any{"user_id": -1, "page": 0, "illust_filter": map[string]any{"type": "wrong"}})
			add("filter-vs-identity-validation", map[string]any{"user_id": -1, "illust_filter": map[string]any{"type": "wrong"}})
		}

		for index, args := range []any{nil, 7, "invalid", []any{}, true} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-root-%d", toolName, index), ToolName: toolName, Arguments: args, Bodies: []json.RawMessage{first, second}})
		}
	}
	artwork := json.RawMessage(`{"illusts":[{"id":22,"type":"illust","title":"artwork","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":22,"type":"illust","title":"duplicate","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":22,"type":"manga","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`)
	novel := json.RawMessage(`{"novels":[{"id":22,"title":"novel","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":22,"title":"duplicate novel","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":24,"title":"last","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`)
	for index, args := range []map[string]any{{}, {"user_id": 7}, {"user_id": 0}, {"user_id": -1}, {"user_id": nil}, {"user_id": 1.5}, {"user_id": "42"}, {"restrict": "public"}, {"restrict": "private"}, {"restrict": "wrong"}, {"tag": "猫 blue"}, {"extra": true}, {"illust_filter": map[string]any{"id": 22}}, {"novel_filter": map[string]any{"id": 22}}, {"limit": 0}, {"limit": 1}, {"limit": 2}, {"limit": 3}, {"limit": 4}, {"limit": 6}, {"limit": 7}, {"page": 2, "limit": 2}, {"page": 3, "limit": 2}, {"page": 4, "limit": 2}, {"page": 2}, {"page": 0}, {"limit": -1}, {"limit": int64(9223372036854775807)}, {"page": int64(9223372036854775807), "limit": 2}} {
		rows = append(rows, row{Name: fmt.Sprintf("bookmark_list_all-argument-%d", index), ToolName: "bookmark_list_all", Arguments: args, Bodies: []json.RawMessage{artwork, novel}})
	}
	for _, fixture := range []struct {
		name   string
		bodies []json.RawMessage
	}{
		{"empty-artwork", []json.RawMessage{json.RawMessage(`{"illusts":[],"next_url":null}`), novel}},
		{"empty-all", []json.RawMessage{json.RawMessage(`{"illusts":[],"next_url":null}`), json.RawMessage(`{"novels":[],"next_url":null}`)}},
		{"novel-malformed", []json.RawMessage{artwork, json.RawMessage(`{}`)}},
		{"novel-invalid-record", []json.RawMessage{artwork, json.RawMessage(`{"novels":[{"id":0,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}]}`)}},
		{"artwork-invalid-record", []json.RawMessage{json.RawMessage(`{"illusts":[{"id":0,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}]}`), novel}},
		{"two-artwork-pages", []json.RawMessage{json.RawMessage(`{"illusts":[{"id":11,"type":"illust","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":"https://app-api.pixiv.net/v1/user/bookmarks/illust?user_id=42&max_bookmark_id=30"}`), artwork, novel}},
	} {
		rows = append(rows, row{Name: "bookmark_list_all-" + fixture.name, ToolName: "bookmark_list_all", Arguments: map[string]any{"limit": 0}, Bodies: fixture.bodies})
	}
	rows = append(rows, row{Name: "bookmark_list_all-missing-current-identity", ToolName: "bookmark_list_all", Arguments: map[string]any{}, Bodies: []json.RawMessage{artwork}, NoIdentity: true})
	for index, args := range []any{nil, 7, "invalid", []any{}, true} {
		rows = append(rows, row{Name: fmt.Sprintf("bookmark_list_all-root-%d", index), ToolName: "bookmark_list_all", Arguments: args, Bodies: []json.RawMessage{artwork, novel}})
	}
	rows = append(rows, row{Name: "bookmark_list_all-precision-record", ToolName: "bookmark_list_all", Arguments: map[string]any{"limit": 0}, Bodies: []json.RawMessage{json.RawMessage(`{"illusts":[{"id":9007199254740993,"type":"illust","total_view":9007199254740993,"user":{"id":9007199254740993},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`), json.RawMessage(`{"novels":[{"id":9007199254740993,"total_view":9007199254740993,"user":{"id":9007199254740993},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`)}})
	rows = append(rows, row{Name: "bookmark_list_all-empty-artwork-refill", ToolName: "bookmark_list_all", Arguments: map[string]any{}, Bodies: []json.RawMessage{json.RawMessage(`{"illusts":[],"next_url":"https://app-api.pixiv.net/v1/user/bookmarks/illust?user_id=42&max_bookmark_id=30"}`), artwork, novel}})
	rows = append(rows, row{Name: "bookmark_list_all-first-artwork-batch-only", ToolName: "bookmark_list_all", Arguments: map[string]any{}, Bodies: []json.RawMessage{json.RawMessage(`{"illusts":[{"id":11,"type":"illust","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":"https://app-api.pixiv.net/v1/user/bookmarks/illust?user_id=42&max_bookmark_id=30"}`), artwork, novel}})

	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, _, err := pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}`)), Request: req}, nil
			}
			bodyIndex := row.Requests
			row.Requests++
			row.Queries = append(row.Queries, req.URL.RawQuery)
			row.Paths = append(row.Paths, req.URL.Path)
			if bodyIndex >= len(row.Bodies) {
				t.Fatalf("unexpected HTTP request %d", row.Requests)
			}
			endpoint := "bookmarks/illust"
			if row.ToolName == "user_novel_bookmarks" {
				endpoint = "bookmarks/novel"
			}
			if row.ToolName == "bookmark_list_all" {
				endpoint = "bookmarks/illust"
				if bytes.Contains(row.Bodies[bodyIndex], []byte(`"novels"`)) {
					endpoint = "bookmarks/novel"
				}
				if strings.Contains(req.URL.Path, "/novel") && !bytes.Contains(row.Bodies[bodyIndex], []byte(`"illusts"`)) {
					endpoint = "bookmarks/novel"
				}
			}
			if req.URL.Path != "/v1/user/"+endpoint || req.Header.Get("Authorization") != "Bearer fixture-access" {
				t.Fatalf("unexpected request %s", req.URL)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		if row.NoIdentity {
			client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(*http.Request) (*http.Response, error) {
				t.Error("missing identity must fail before content requests")
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
				if item.Name == "user_bookmarks" || item.Name == "user_novel_bookmarks" || item.Name == "bookmark_list_all" {
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
		if row.Name == "user_bookmarks-argument-22" || row.Name == "user_novel_bookmarks-argument-22" {
			t.Logf("%s actual RPC diagnostic: %s", row.Name, row.RPCError)
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		successCounts := map[string]int{
			"bookmark_list_all-argument-0": 3, "bookmark_list_all-argument-1": 3,
			"bookmark_list_all-argument-7": 3, "bookmark_list_all-argument-8": 3, "bookmark_list_all-argument-10": 3,
			"bookmark_list_all-argument-14": 6, "bookmark_list_all-argument-15": 1, "bookmark_list_all-argument-16": 2,
			"bookmark_list_all-argument-17": 3, "bookmark_list_all-argument-18": 4, "bookmark_list_all-argument-19": 6, "bookmark_list_all-argument-20": 6,
			"bookmark_list_all-argument-21": 2, "bookmark_list_all-argument-22": 2, "bookmark_list_all-argument-23": 0,
			"bookmark_list_all-empty-artwork": 3, "bookmark_list_all-empty-all": 0, "bookmark_list_all-two-artwork-pages": 7,
			"bookmark_list_all-precision-record": 2, "bookmark_list_all-empty-artwork-refill": 3, "bookmark_list_all-first-artwork-batch-only": 1,
			"user_bookmarks-argument-0": 2, "user_bookmarks-argument-14": 4, "user_bookmarks-precision-record": 1,
			"user_novel_bookmarks-argument-0": 2, "user_novel_bookmarks-argument-14": 3, "user_novel_bookmarks-precision-record": 1,
		}
		if count, ok := successCounts[row.Name]; ok {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Records []json.RawMessage `json:"records"`
				} `json:"structuredContent"`
			}
			if row.RPCError != "" {
				t.Fatalf("%s expected success, got RPC error %s", row.Name, row.RPCError)
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			if result.IsError || len(result.Structured.Records) != count {
				t.Fatalf("%s expected %d successful records, got %s", row.Name, count, row.Result)
			}
		}

		_ = session.Close()
		cancel()
	}
	if len(tools) != 3 {
		t.Fatal("bookmark lists tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-bookmark-lists.json")
	if *migrationUpdateMCPBookmarkLists {
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
		t.Fatal("MCP bookmark lists differ from fixed Go reference")
	}
}
