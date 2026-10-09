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
	"strings"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPCommentMutations = flag.Bool("migration-update-mcp-comment-mutations", false, "capture fixed Go MCP comment mutation contracts")

func TestMigrationMCPCommentMutationsMatchesFrozenContracts(t *testing.T) {
	type row struct {
		Name      string          `json:"name"`
		Tool      string          `json:"tool"`
		Arguments any             `json:"arguments"`
		Status    int             `json:"status"`
		Body      string          `json:"body"`
		Result    json.RawMessage `json:"result"`
		Calls     int             `json:"calls"`
		Requests  []string        `json:"requests"`
		RPCError  string          `json:"rpc_error"`
	}
	var rows []row
	tools := map[string]json.RawMessage{}
	names := []string{"create_artwork_comment", "reply_artwork_comment", "delete_artwork_comment", "stamp_artwork_comment", "create_novel_comment", "reply_novel_comment", "delete_novel_comment", "stamp_novel_comment"}
	for _, name := range names {
		field := "illust_id"
		if strings.Contains(name, "novel") {
			field = "novel_id"
		}
		base := map[string]any{field: 42, "comment": " 日本語 &+\n "}
		if strings.HasPrefix(name, "reply_") {
			base["parent_comment_id"] = 9
		}
		if strings.HasPrefix(name, "stamp_") {
			base["stamp_id"] = 7
			delete(base, "comment")
		}
		if strings.HasPrefix(name, "delete_") {
			field = "comment_id"
			base = map[string]any{field: 22}
		}
		add := func(label string, args any, status int, body string) {
			rows = append(rows, row{Name: name + ":" + label, Tool: name, Arguments: args, Status: status, Body: body})
		}
		add("success", base, 200, `{"comment":{"id":63}}`)
		if name == "create_artwork_comment" {
			add("large-returned-id", base, 200, `{"comment":{"id":9007199254740993}}`)
		}
		clone := func() map[string]any {
			out := map[string]any{}
			for k, v := range base {
				out[k] = v
			}
			return out
		}
		for _, value := range []any{0, "42", nil, 1.5} {
			a := clone()
			a[field] = value
			add("invalid-id", a, 200, `{"comment":{"id":63}}`)
		}
		a := clone()
		delete(a, field)
		add("missing-id", a, 200, `{}`)
		a = clone()
		a["extra"] = true
		add("extra", a, 200, `{}`)
		if strings.HasPrefix(name, "create_") || strings.HasPrefix(name, "reply_") {
			a = clone()
			a["comment"] = ""
			add("empty-comment", a, 200, `{}`)
			a = clone()
			a["comment"] = " "
			add("blank-comment", a, 200, `{}`)
		}
		if strings.HasPrefix(name, "reply_") {
			a = clone()
			a["parent_comment_id"] = 0
			add("invalid-parent", a, 200, `{}`)
		}
		if strings.HasPrefix(name, "stamp_") {
			a = clone()
			a["stamp_id"] = 0
			add("invalid-stamp", a, 200, `{}`)
			a = clone()
			a["comment"] = " optional "
			add("stamp-text", a, 200, `{"comment":{"id":64}}`)
		}
		if !strings.HasPrefix(name, "delete_") {
			add("missing-returned-id", base, 200, `{"comment":{}}`)
			add("malformed-response", base, 200, `not JSON`)
		}
		add("upstream-error", base, 403, `private fixture body`)
		add("rate-limited", base, 429, `{}`)
	}
	for i := range rows {
		row := &rows[i]
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "POST" || req.URL.Host != "app-api.pixiv.net" || req.URL.RawQuery != "" || req.Header.Get("Authorization") != "Bearer fixture-access" || req.Header.Get("Content-Type") != "application/x-www-form-urlencoded" {
				t.Fatal("unexpected mutation request")
			}
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			row.Requests = append(row.Requests, "POST "+req.URL.Path+" "+req.PostForm.Encode())
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"text/plain"}, "Retry-After": {"0"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			row.Calls++
			committed, err := invoke(ctx, client)
			if !committed {
				t.Fatal("mutation must suppress replay")
			}
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		ct, st := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, st) }()
		capture := &migrationMCPCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationMCPCaptureTransport{ct, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if i == 0 {
			list, err := session.ListTools(ctx, nil)
			if err != nil {
				t.Fatal(err)
			}
			for _, tool := range list.Tools {
				for _, name := range names {
					if tool.Name == name {
						tools[name], err = json.Marshal(tool)
						if err != nil {
							t.Fatal(err)
						}
					}
				}
			}
		}
		_, err = session.CallTool(ctx, &mcp.CallToolParams{Name: row.Tool, Arguments: row.Arguments})
		if err != nil {
			row.RPCError = err.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
	}
	if len(tools) != 8 {
		t.Fatal("missing mutation tools")
	}
	data, err := json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-comment-mutations.json")
	if *migrationUpdateMCPCommentMutations {
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
		t.Fatal("MCP mutations differ from fixed Go reference")
	}
}
