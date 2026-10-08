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
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateRPC = flag.Bool("migration-update-mcp-rpc", false, "capture fixed Go MCP protocol contracts")

func TestMigrationMCPRPCMatchesFrozenInitializationAndArgumentErrors(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var artwork []struct {
		Body json.RawMessage `json:"body"`
	}
	if err = json.Unmarshal(data, &artwork); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Name     string          `json:"name"`
		Request  json.RawMessage `json:"request"`
		Response json.RawMessage `json:"response"`
		Calls    int             `json:"calls"`
		Requests int             `json:"requests"`
	}
	var rows []row
	for _, version := range []string{"2025-06-18", "2025-03-26", "2024-11-05", "future", ""} {
		request, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": "init", "method": "initialize", "params": map[string]any{"protocolVersion": version, "capabilities": map[string]any{}, "clientInfo": map[string]any{"name": "migration", "version": "0"}}})
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{Name: "initialize-" + version, Request: request})
	}
	for _, in := range []struct{ name, method, params string }{
		{"ping", "ping", `{}`}, {"unknown-method", "missing/method", `{}`},
		{"logging-debug", "logging/setLevel", `{"level":"debug"}`},
		{"logging-invalid", "logging/setLevel", `{"level":"invalid"}`},
		{"logging-missing", "logging/setLevel", `{}`},
		{"initialize-missing-params", "initialize", ``},
		{"missing-tool", "tools/call", `{"name":"missing_tool","arguments":{}}`},
		{"missing-name", "tools/call", `{"arguments":{}}`},
		{"missing-params", "tools/call", ``},
		{"empty", "tools/call", `{"name":"illust_detail","arguments":{}}`},
		{"missing-arguments", "tools/call", `{"name":"illust_detail"}`},
		{"null-arguments", "tools/call", `{"name":"illust_detail","arguments":null}`},
		{"array-arguments", "tools/call", `{"name":"illust_detail","arguments":[]}`},
		{"string-arguments", "tools/call", `{"name":"illust_detail","arguments":"x"}`},
		{"string-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":"42"}}`},
		{"boolean-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":true}}`},
		{"null-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":null}}`},
		{"fraction-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":42.5}}`},
		{"integral-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":42.0}}`},
		{"int64-max-id", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":9223372036854775807}}`},
		{"unknown-field", "tools/call", `{"name":"illust_detail","arguments":{"extra":42}}`},
		{"null-url", "tools/call", `{"name":"illust_detail","arguments":{"url":null}}`},
		{"number-url", "tools/call", `{"name":"illust_detail","arguments":{"url":42}}`},
		{"valid-url", "tools/call", `{"name":"illust_detail","arguments":{"url":"https://www.pixiv.net/artworks/42"}}`},
		{"cancelled", "tools/call", `{"name":"illust_detail","arguments":{"illust_id":42}}`},
	} {
		params := ""
		if in.params != "" {
			params = `,"params":` + in.params
		}
		rows = append(rows, row{Name: in.name, Request: json.RawMessage(`{"jsonrpc":"2.0","id":7,"method":"` + in.method + `"` + params + `}`)})
	}
	for index := range rows {
		row := &rows[index]
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		started := make(chan struct{})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			row.Requests++
			if row.Name == "cancelled" {
				close(started)
				<-req.Context().Done()
				return nil, req.Context().Err()
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(artwork[0].Body)), Request: req}, nil
		})}})
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			row.Calls++
			_, err := invoke(ctx, client)
			return err
		}}, pixivmcp.Account{})
		clientTransport, serverTransport := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, serverTransport) }()
		connection, err := clientTransport.Connect(ctx)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		send := func(raw []byte) {
			message, err := jsonrpc.DecodeMessage(raw)
			if err != nil {
				t.Fatal(err)
			}
			if err = connection.Write(ctx, message); err != nil {
				t.Fatal(err)
			}
		}
		read := func() json.RawMessage {
			message, err := connection.Read(ctx)
			if err != nil {
				t.Fatalf("%s: %v", row.Name, err)
			}
			data, err := jsonrpc.EncodeMessage(message)
			if err != nil {
				t.Fatal(err)
			}
			return data
		}
		if index >= 5 {
			send([]byte(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration","version":"0"}}}`))
			_ = read()
			send([]byte(`{"jsonrpc":"2.0","method":"notifications/initialized"}`))
		}
		send(row.Request)
		if row.Name == "cancelled" {
			select {
			case <-started:
			case <-ctx.Done():
				t.Fatal(ctx.Err())
			}
			send([]byte(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture cancelled"}}`))
		}
		row.Response = read()
		_ = connection.Close()
		cancel()
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-rpc.json")
	if *migrationUpdateRPC {
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
		t.Fatal("MCP protocol differs from fixed Go reference")
	}
}
