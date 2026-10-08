package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPSearch = flag.Bool("migration-update-mcp-search", false, "capture frozen Go MCP search schema and logical pagination")

func TestMigrationMCPSearchMatchesFrozenFiltersLogicalPagesAndBookmarkStrategies(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		Name      string            `json:"name"`
		Arguments any               `json:"arguments"`
		Bodies    []json.RawMessage `json:"bodies"`
		Result    json.RawMessage   `json:"result"`
		Calls     int               `json:"calls"`
		Queries   []url.Values      `json:"queries"`
		RPCError  string            `json:"rpc_error"`
	}
	batch := func(ids []int, next int) json.RawMessage {
		items := []any{}
		for _, id := range ids {
			kind, tag, ai := "illust", "other", 2
			if id == 2 || id == 4 {
				tag = "keep"
			}
			if id == 3 {
				kind = "ugoira"
			}
			if id == 1 {
				ai = 1
			}
			items = append(items, map[string]any{"id": id, "type": kind, "title": fmt.Sprintf("fixture %d", id), "create_date": "2026-01-01T00:00:00Z", "illust_ai_type": ai, "total_bookmarks": id * 10, "total_view": id * 100, "page_count": id%3 + 1, "tags": []any{map[string]any{"name": tag}, map[string]any{"name": "日本語"}}})
		}
		envelope := map[string]any{"illusts": items}
		if next > 0 {
			envelope["next_url"] = fmt.Sprintf("https://app-api.pixiv.net/v1/search/illust?offset=%d", next)
		}
		encoded, err := json.Marshal(envelope)
		if err != nil {
			t.Fatal(err)
		}
		return encoded
	}
	defaultBodies := []json.RawMessage{batch([]int{1, 2, 3}, 30), batch([]int{1, 4}, 60), batch([]int{5}, 0)}
	rows := []row{}
	add := func(name string, arguments any) {
		rows = append(rows, row{Name: name, Arguments: arguments, Bodies: defaultBodies})
	}
	add("default", map[string]any{"word": "fixture"})
	for _, strategy := range []string{"auto", "local", "best_effort"} {
		add("strategy-without-bounds:"+strategy, map[string]any{"word": "fixture", "bookmark_strategy": strategy, "limit": 0})
	}
	add("strategy-without-bounds-filter", map[string]any{"word": "fixture", "bookmark_strategy": "local", "limit": 0, "illust_filter": map[string]any{"id": 99}})
	add("logical-offset-overflow", map[string]any{"word": "fixture", "page": int64(4503599627370496), "limit": 4096})
	rows = append(rows, row{Name: "leading-empty-one-batch", Arguments: map[string]any{"word": "fixture"}, Bodies: []json.RawMessage{batch([]int{}, 30), defaultBodies[1], defaultBodies[2]}})
	rows = append(rows, row{Name: "leading-ai-empty-one-batch", Arguments: map[string]any{"word": "fixture", "ai_mode": "only"}, Bodies: []json.RawMessage{batch([]int{1}, 30), defaultBodies[1], defaultBodies[2]}})
	for _, limit := range []int{-1, 0, 1, 2, 3, 4, 5, 6, 20} {
		add(fmt.Sprintf("limit:%d", limit), map[string]any{"word": "fixture", "limit": limit})
	}
	for _, pair := range [][2]int{{0, 2}, {-1, 2}, {1, 0}, {2, 0}, {1, 2}, {2, 2}, {3, 2}, {4, 2}, {9223372036854775807, 1}, {9223372036854775807, 2}, {2, 9223372036854775807}} {
		add(fmt.Sprintf("page:%d:limit:%d", pair[0], pair[1]), map[string]any{"word": "fixture", "page": pair[0], "limit": pair[1]})
	}
	add("page-without-limit", map[string]any{"word": "fixture", "page": 1})
	for _, entry := range []struct {
		field  string
		values []any
	}{
		{"word", []any{"", " \t", " 日本語 ~*+% ", nil, 7}},
		{"search_target", []any{"partial_match_for_tags", "exact_match_for_tags", "title_and_caption", "keyword", "bad", ""}},
		{"sort", []any{"date_desc", "date_asc", "popular_desc", "bad", ""}},
		{"duration", []any{"within_last_day", "within_last_week", "within_last_month", "bad", ""}},
		{"start_date", []any{"2024-02-29", "2025-02-29", "0000-01-01", "2026-1-01", ""}},
		{"end_date", []any{"2026-12-31", "bad"}},
		{"content_type", []any{"all", "illust-and-ugoira", "illust", "manga", "ugoira", "bad"}},
		{"ai_mode", []any{"all", "exclude", "only", "bad"}},
		{"aspect_ratio", []any{"all", "landscape", "portrait", "square", "bad"}},
		{"resolution", []any{"all", "high", "medium", "low", "bad"}},
		{"tool", []any{"CLIP STUDIO PAINT", "unlisted exact tool"}},
		{"bookmark_strategy", []any{"auto", "local", "best_effort", "server", "bad", ""}},
	} {
		for index, value := range entry.values {
			add(fmt.Sprintf("%s:%d", entry.field, index), map[string]any{"word": "fixture", entry.field: value})
		}
	}
	for _, args := range []map[string]any{
		{}, {"word": "fixture", "unknown": 1}, {"word": "fixture", "limit": nil}, {"word": "fixture", "limit": 1.5},
		{"word": "fixture", "duration": "within_last_day", "start_date": "2026-01-01"},
		{"word": "fixture", "duration": "within_year", "start_date": "2026-01-01"},
		{"word": "fixture", "start_date": "2026-01-02", "end_date": "2026-01-01"},
		{"word": "fixture", "bookmark_min": -1}, {"word": "fixture", "bookmark_max": -1},
		{"word": "fixture", "bookmark_min": 20, "bookmark_max": 10},
		{"word": "fixture", "bookmark_min": 20, "illust_filter": map[string]any{}},
	} {
		add(fmt.Sprintf("validation:%d", len(rows)), args)
	}
	add("null-arguments", nil)
	add("array-arguments", []any{})
	for _, filter := range []map[string]any{
		{}, {"id": 2}, {"id": 99}, {"type": "illust"}, {"type": "ugoira"}, {"tags": []string{"keep", "日本語"}},
		{"tags": []string{"missing"}}, {"min_views": 300}, {"min_pages": 3}, {"id": 0}, {"type": "bad"},
		{"min_views": -1}, {"min_pages": -1}, {"unknown": true}, {"tags": []any{nil}},
	} {
		add(fmt.Sprintf("filter:%d", len(rows)), map[string]any{"word": "fixture", "limit": 0, "illust_filter": filter})
	}
	for _, strategy := range []string{"", "auto", "local", "best_effort", "server"} {
		for _, limit := range []int{0, 1, 2, 3} {
			add(fmt.Sprintf("bookmark:%s:%d", strategy, limit), map[string]any{"word": "fixture", "bookmark_min": 20, "bookmark_max": 40, "bookmark_strategy": strategy, "limit": limit})
		}
	}
	for _, args := range []map[string]any{{"word": "fixture", "bookmark_min": 0}, {"word": "fixture", "bookmark_max": 0}, {"word": "fixture", "bookmark_min": 99, "limit": 0}, {"word": "fixture", "illust_filter": map[string]any{"tags": []string{"keep"}}, "page": 2, "limit": 1}} {
		add(fmt.Sprintf("bounds:%d", len(rows)), args)
	}
	for _, first := range []json.RawMessage{batch([]int{}, 30), json.RawMessage(`{"illusts":[]}`), json.RawMessage(`{}`), json.RawMessage(`{"illusts":[{"id":1,"create_date":"bad"}]}`), json.RawMessage(`{"illusts":[{"id":1,"type":"bad","create_date":"2026-01-01T00:00:00Z"}]}`), json.RawMessage(`{"illusts":[{"id":1,"total_bookmarks":-1,"create_date":"2026-01-01T00:00:00Z"}]}`)} {
		rows = append(rows, row{Name: fmt.Sprintf("response:%d", len(rows)), Arguments: map[string]any{"word": "fixture", "bookmark_min": 0, "limit": 0}, Bodies: []json.RawMessage{first, defaultBodies[1], defaultBodies[2]}})
	}
	rows = append(rows, row{Name: "later-page-error", Arguments: map[string]any{"word": "fixture", "limit": 0}, Bodies: []json.RawMessage{defaultBodies[0], json.RawMessage(`{}`)}})
	var tool json.RawMessage
	for index := range rows {
		row := &rows[index]
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			row.Queries = append(row.Queries, req.URL.Query())
			bodyIndex := 0
			switch req.URL.Query().Get("offset") {
			case "30":
				bodyIndex = 1
			case "60":
				bodyIndex = 2
			}
			if bodyIndex >= len(row.Bodies) {
				bodyIndex = 0
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
				if item.Name == "search_illust" {
					tool, err = json.Marshal(item)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
		}
		_, callErr := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "search_illust", Arguments: row.Arguments})
		if callErr != nil {
			row.RPCError = callErr.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
	}
	if len(tool) == 0 {
		t.Fatal("search_illust is not registered")
	}
	data, err := json.MarshalIndent(struct {
		Tool  json.RawMessage `json:"tool"`
		Cases []row           `json:"cases"`
	}{tool, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-search.json")
	if *migrationUpdateMCPSearch {
		if err = os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP search differs from fixed Go reference")
	}
}
