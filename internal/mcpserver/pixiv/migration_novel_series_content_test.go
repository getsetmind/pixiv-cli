package pixiv_test

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

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPNovelSeriesContent = flag.Bool("migration-update-mcp-novel-series-content", false, "capture fixed Go MCP detail contracts")

func TestMigrationMCPNovelSeriesContentMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		Name      string            `json:"name"`
		ToolName  string            `json:"tool_name"`
		Arguments map[string]any    `json:"arguments"`
		Bodies    []json.RawMessage `json:"bodies"`
		Result    json.RawMessage   `json:"result"`
		Calls     int               `json:"calls"`
		Requests  int               `json:"requests"`
		RPCError  string            `json:"rpc_error"`
	}
	var rows []row
	first := json.RawMessage(`{"novel_series_detail":{"id":21,"title":"first metadata","caption":"caption","is_concluded":true,"user":{"id":7,"name":"writer"}},"novels":[{"id":22,"title":"one","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":23,"title":"two","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":"https://app-api.pixiv.net/v2/novel/series?series_id=21&last_order=9"}`)
	second := json.RawMessage(`{"novel_series_detail":{"id":21,"title":"second metadata","user":{"id":7}},"novels":[{"id":23,"title":"duplicate","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":24,"title":"three","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`)
	for _, toolName := range []string{"novel_series", "novel_content"} {
		idName := "series_id"
		if toolName == "novel_content" {
			idName = "novel_id"
		}
		for index, args := range []map[string]any{{idName: 21}, {}, {idName: 0}, {idName: -1}, {idName: 21, "extra": true}, {idName: int64(9223372036854775807)}, {idName: "21"}, {idName: nil}, {idName: 1.5}, {idName: 21, "page": 2}, {idName: 21, "limit": 1}, {idName: 21, "limit": -1}, {idName: 21, "page": 0}, {idName: 21, "page": -1}, {idName: 21, "limit": 0}, {idName: 21, "page": 2, "limit": 1}, {idName: 21, "page": 1.5}, {idName: 21, "limit": "2"}} {
			rows = append(rows, row{Name: toolName + "-argument-" + string(rune('a'+index)), ToolName: toolName, Arguments: args, Bodies: []json.RawMessage{first, second}})
		}
	}
	for index, body := range []string{`{"novels":[]}`, `{"novel_series_detail":null,"novels":[]}`, `{"novel_series_detail":{"id":21,"user":{"id":7}},"novels":null}`, `{"novel_series_detail":{"id":21,"user":{"id":7}},"novels":[{"id":0,"user":{"id":7}}]}`, `{"novel_series_detail":{"id":21,"user":{"id":7}},"novels":[],"next_url":null}`} {
		rows = append(rows, row{Name: "series-body-" + string(rune('a'+index)), ToolName: "novel_series", Arguments: map[string]any{"series_id": 21}, Bodies: []json.RawMessage{json.RawMessage(body)}})
	}
	terminal := json.RawMessage(`{"novel_series_detail":{"id":21,"title":"terminal","user":{"id":7}},"novels":[{"id":22,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"},{"id":23,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`)
	empty := json.RawMessage(`{"novel_series_detail":{"id":21,"title":"empty first metadata","user":{"id":7}},"novels":[],"next_url":"https://app-api.pixiv.net/v2/novel/series?series_id=21&last_order=9"}`)
	for _, extra := range []row{
		{Name: "empty-first-page", ToolName: "novel_series", Arguments: map[string]any{"series_id": 21}, Bodies: []json.RawMessage{empty, terminal}},
		{Name: "terminal-truncation", ToolName: "novel_series", Arguments: map[string]any{"series_id": 21, "limit": 1}, Bodies: []json.RawMessage{terminal}},
		{Name: "later-page-malformed", ToolName: "novel_series", Arguments: map[string]any{"series_id": 21, "limit": 0}, Bodies: []json.RawMessage{first, json.RawMessage(`{"novels":[]}`)}},
		{Name: "offset-past-end", ToolName: "novel_series", Arguments: map[string]any{"series_id": 21, "page": 9, "limit": 1}, Bodies: []json.RawMessage{first, second}},
	} {
		rows = append(rows, extra)
	}

	for index, args := range []map[string]any{{"series_id": 21, "page": nil}, {"series_id": 21, "limit": nil}, {"series_id": 21, "page": nil, "limit": nil}, {"series_id": 21, "limit": int64(9223372036854775807)}, {"series_id": 21, "page": int64(9223372036854775807), "limit": 1}} {
		rows = append(rows, row{Name: "nullable-or-overflow-" + string(rune('a'+index)), ToolName: "novel_series", Arguments: args, Bodies: []json.RawMessage{first, second}})
	}

	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			bodyIndex := row.Requests
			row.Requests++
			if bodyIndex >= len(row.Bodies) {
				t.Fatalf("unexpected HTTP request %d", row.Requests)
			}
			if req.URL.Path != "/v2/novel/series" {
				t.Fatalf("unexpected request %s", req.URL)
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
				if item.Name == "novel_series" || item.Name == "novel_content" {
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
		_ = session.Close()
		cancel()
	}
	if len(tools) != 2 {
		t.Fatal("novel_detail was not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-novel-series-content.json")
	if *migrationUpdateMCPNovelSeriesContent {
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
