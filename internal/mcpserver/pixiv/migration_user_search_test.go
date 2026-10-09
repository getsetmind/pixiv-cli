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
	"strconv"
	"strings"
	"testing"
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPUserSearch = flag.Bool("migration-update-mcp-user-search", false, "capture fixed Go MCP user search contracts")

func TestMigrationMCPUserSearchPreservesSchemaFiltersLogicalPagesAndValidation(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "user-search.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Name   string            `json:"name"`
		Word   string            `json:"word"`
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Name      string            `json:"name"`
		Arguments any               `json:"arguments"`
		Bodies    []json.RawMessage `json:"bodies"`
		Result    json.RawMessage   `json:"result"`
		Queries   []url.Values      `json:"queries"`
		Calls     int               `json:"calls"`
		RPCError  string            `json:"rpc_error"`
	}
	var rows []row
	for _, source := range sources {
		if strings.HasPrefix(source.Name, "cursor:") || strings.HasPrefix(source.Name, "binding:") {
			continue
		}
		rows = append(rows, row{Name: source.Name, Arguments: map[string]any{"word": source.Word}, Bodies: source.Bodies})
	}
	batch := func(ids []int64, offset int) json.RawMessage {
		var body map[string]any
		if err := json.Unmarshal(sources[0].Bodies[0], &body); err != nil {
			t.Fatal(err)
		}
		preview := body["user_previews"].([]any)[0].(map[string]any)
		var previews []any
		for _, id := range ids {
			copyData, err := json.Marshal(preview)
			if err != nil {
				t.Fatal(err)
			}
			var copyPreview map[string]any
			if err := json.Unmarshal(copyData, &copyPreview); err != nil {
				t.Fatal(err)
			}
			copyPreview["user"].(map[string]any)["id"] = id
			previews = append(previews, copyPreview)
		}
		if previews == nil {
			previews = []any{}
		}
		body["user_previews"] = previews
		body["next_url"] = nil
		if offset > 0 {
			body["next_url"] = "https://app-api.pixiv.net/v1/search/user?offset=" + strconv.Itoa(offset)
		}
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	bodies := []json.RawMessage{batch([]int64{31, 31, 32}, 30), batch([]int64{32, 33}, 60), batch([]int64{34}, 0)}
	for _, filter := range []any{nil, map[string]any{}, map[string]any{"id": 31}, map[string]any{"id": 33}, map[string]any{"id": 999}} {
		for _, limit := range []any{nil, 0, 1, 2, 4, 5} {
			for _, page := range []any{nil, 1, 2, 3} {
				if page != nil && (limit == nil || limit == 0) {
					continue
				}
				arguments := map[string]any{"word": "miku"}
				if filter != nil {
					arguments["user_filter"] = filter
				}
				if limit != nil {
					arguments["limit"] = limit
				}
				if page != nil {
					arguments["page"] = page
				}
				rows = append(rows, row{Name: fmt.Sprintf("logical:filter=%v:limit=%v:page=%v", filter, limit, page), Arguments: arguments, Bodies: bodies})
			}
		}
	}
	for _, custom := range []struct {
		Name   string
		Bodies []json.RawMessage
	}{
		{"empty-first-batch", []json.RawMessage{batch(nil, 30), batch([]int64{33}, 0)}},
		{"duplicates-only-second-batch", []json.RawMessage{batch([]int64{31}, 30), batch([]int64{31}, 60), batch([]int64{34}, 0)}},
		{"failure-after-partial-results", []json.RawMessage{batch([]int64{31}, 30), json.RawMessage(`{}`)}},
		{"numeric-precision", []json.RawMessage{batch([]int64{9007199254740993}, 0)}},
	} {
		rows = append(rows, row{Name: custom.Name, Arguments: map[string]any{"word": "miku", "limit": 0}, Bodies: custom.Bodies})
	}
	for index, arguments := range []any{
		map[string]any{}, nil, 7, "miku", []any{},
		map[string]any{"word": ""}, map[string]any{"word": nil}, map[string]any{"word": 7}, map[string]any{"word": "miku", "extra": true},
		map[string]any{"word": " \t", "page": 1}, map[string]any{"word": "\u0085\u00a0\u3000"},
		map[string]any{"word": "miku", "page": 0}, map[string]any{"word": "miku", "page": -1}, map[string]any{"word": "miku", "page": 1.5},
		map[string]any{"word": "miku", "page": nil}, map[string]any{"word": "miku", "page": "2"},
		map[string]any{"word": "miku", "limit": -1}, map[string]any{"word": "miku", "limit": 1.5}, map[string]any{"word": "miku", "limit": nil},
		map[string]any{"word": "miku", "limit": "2"}, map[string]any{"word": "miku", "page": 1}, map[string]any{"word": "miku", "page": 2, "limit": 0},
		map[string]any{"word": "miku", "page": int64(4000000000000000000), "limit": 3},
		map[string]any{"word": "miku", "page": int64(9223372036854775807), "limit": 1}, map[string]any{"word": "miku", "limit": int64(9223372036854775807)},
		map[string]any{"word": "miku", "user_filter": nil}, map[string]any{"word": "miku", "user_filter": []any{}}, map[string]any{"word": "miku", "user_filter": 7},
		map[string]any{"word": "miku", "user_filter": map[string]any{"id": 0}}, map[string]any{"word": "miku", "user_filter": map[string]any{"id": -1}},
		map[string]any{"word": "miku", "user_filter": map[string]any{"id": 1.5}}, map[string]any{"word": "miku", "user_filter": map[string]any{"id": nil}},
		map[string]any{"word": "miku", "user_filter": map[string]any{"id": "31"}}, map[string]any{"word": "miku", "user_filter": map[string]any{"id": int64(9223372036854775807)}},
		map[string]any{"word": "miku", "user_filter": map[string]any{"id": int64(9007199254740993)}},
		map[string]any{"word": "miku", "user_filter": map[string]any{"tags": []string{"unexpected"}}},
	} {
		rows = append(rows, row{Name: fmt.Sprintf("argument:%02d", index), Arguments: arguments, Bodies: bodies})
	}
	var schema json.RawMessage
	for index := range rows {
		current := &rows[index]
		current.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(request *http.Request) (*http.Response, error) {
			if request.Method != "GET" || request.URL.String() != "https://app-api.pixiv.net/v1/search/user?"+request.URL.RawQuery {
				t.Fatal("unexpected user search MCP request")
			}
			current.Queries = append(current.Queries, request.URL.Query())
			page := 0
			if offset := request.URL.Query().Get("offset"); offset != "" {
				parsed, err := strconv.Atoi(offset)
				if err != nil || parsed <= 0 {
					t.Fatalf("unexpected offset %q", offset)
				}
				page = parsed / 30
			}
			if page >= len(current.Bodies) {
				t.Fatalf("unexpected page %d in %s", page, current.Name)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(current.Bodies[page])), Request: request}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			current.Calls++
			_, err := invoke(ctx, client)
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		peer, local := mcp.NewInMemoryTransports()
		done := make(chan error, 1)
		go func() { done <- server.Run(ctx, local) }()
		capture := &migrationMCPCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationMCPCaptureTransport{peer, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if schema == nil {
			tools, err := session.ListTools(ctx, nil)
			if err != nil {
				t.Fatal(err)
			}
			for _, tool := range tools.Tools {
				if tool.Name == "search_user" {
					schema, err = json.Marshal(tool)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
			if schema == nil {
				t.Fatal("missing user search MCP schema")
			}
		}
		if _, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "search_user", Arguments: current.Arguments}); err != nil {
			current.RPCError = err.Error()
		}
		capture.mu.Lock()
		current.Result = append(json.RawMessage(nil), capture.result...)
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("user search MCP session did not stop")
		}
	}
	data, err = json.MarshalIndent(struct {
		Schema json.RawMessage `json:"schema"`
		Cases  []row           `json:"cases"`
	}{schema, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-user-search.json")
	if *migrationUpdateMCPUserSearch {
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
		t.Fatal("user search MCP differs from fixed Go reference")
	}
}
