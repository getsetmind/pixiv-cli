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
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPUserWorks = flag.Bool("migration-update-mcp-user-works", false, "capture fixed Go MCP user works contracts")

var migrationSkipUserWorksViolationSelection = flag.Bool("migration-skip-nondeterministic-user-works-violation-selection", false, "leave selection between simultaneous page and limit schema violations unverified")

const migrationUserWorksLimitViolation = `calling "tools/call": invalid params: validating "arguments": validating root: validating /properties/limit: minimum: -1/1 is less than 0.000000`
const migrationUserWorksPageViolation = `calling "tools/call": invalid params: validating "arguments": validating root: validating /properties/page: minimum: 0/1 is less than 1.000000`

func TestMigrationMCPUserWorksMatchesFrozenContracts(t *testing.T) {
	if *migrationUpdateMCPUserWorks && *migrationSkipUserWorksViolationSelection {
		t.Fatal("fixture capture cannot skip violation selection")
	}
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
		Queries    []string          `json:"queries"`
		RPCError   string            `json:"rpc_error"`
	}
	var rows []row
	art := func(id int, kind string, tags string, views, pages int) string {
		return fmt.Sprintf(`{"id":%d,"type":%q,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"page_count":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, kind, tags, views, pages)
	}
	for _, toolName := range []string{"user_artworks", "user_novels"} {
		field, endpoint := "illusts", "illusts"
		if toolName == "user_novels" {
			field, endpoint = "novels", "novels"
		}
		item := func(id int, kind string, tags string, views, pages int) string {
			if endpoint == "illusts" {
				return art(id, kind, tags, views, pages)
			}
			return fmt.Sprintf(`{"id":%d,"title":"fixture","user":{"id":7},"tags":%s,"total_view":%d,"create_date":"2024-01-02T03:04:05+00:00"}`, id, tags, views)
		}
		first := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s,%s],"next_url":"https://app-api.pixiv.net/v1/user/%s?user_id=42&offset=30"}`, field, item(22, "illust", `[{"name":"cat"},{"name":"blue"}]`, 100, 2), item(22, "illust", `[{"name":"cat"}]`, 50, 1), item(23, "manga", `[{"name":"cat"}]`, 50, 1), endpoint))
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
		for index, args := range []map[string]any{{}, {"user_id": 7}, {"extra": true}, {"user_id": 0}, {"user_id": nil}, {"user_id": -1}, {"user_id": 1.5}, {"user_id": "42"}, {"user_id": int64(9223372036854775807)}, {"page": 2}, {"limit": 1}, {"limit": -1}, {"page": 0}, {"page": -1}, {"limit": 0}, {"page": 2, "limit": 1}, {"page": 1.5}, {"limit": "2"}, {"page": nil}, {"limit": nil}, {"limit": int64(9223372036854775807)}, {"page": int64(9223372036854775807), "limit": 2}, {"page": 0, "limit": -1}} {
			add(fmt.Sprintf("argument-%d", index), args)
		}
		filterName := "illust_filter"
		filters := []any{nil, map[string]any{}, map[string]any{"id": 22}, map[string]any{"tags": []string{"cat", "blue"}}, map[string]any{"min_views": 100}, map[string]any{"id": 0}, map[string]any{"min_views": -1}, map[string]any{"extra": 1}, map[string]any{"tags": []any{1}}, map[string]any{"id": nil}, map[string]any{"tags": nil}, map[string]any{"min_views": nil}, map[string]any{"id": 1.5}, map[string]any{"id": int64(9223372036854775807)}, map[string]any{"min_views": int64(9223372036854775807)}}
		if endpoint == "novels" {
			filterName = "novel_filter"
		} else {
			filters = append(filters, map[string]any{"type": "manga"}, map[string]any{"type": "wrong"}, map[string]any{"min_pages": 2}, map[string]any{"min_pages": -1}, map[string]any{"type": nil}, map[string]any{"min_pages": nil}, map[string]any{"min_pages": int64(9223372036854775807)})
			for index, kind := range []any{"illust", "manga", "ugoira", "illustration", "", nil, 7} {
				add(fmt.Sprintf("type-%d", index), map[string]any{"type": kind, "limit": 0})
			}
			add("type-filter-conflict", map[string]any{"type": "illust", filterName: map[string]any{"type": "manga"}, "limit": 0})
		}
		for index, filter := range filters {
			add(fmt.Sprintf("filter-%d", index), map[string]any{filterName: filter, "limit": 0})
		}
		for _, limit := range []int{0, 1, 2} {
			for _, page := range []int{1, 2, 3} {
				if limit == 0 && page > 1 {
					continue
				}
				args := map[string]any{"limit": limit, filterName: map[string]any{"tags": []string{"cat", "blue"}}}
				if limit > 0 {
					args["page"] = page
				}
				add(fmt.Sprintf("window-%d-%d", limit, page), args)
			}
		}
		empty := json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net/v1/user/%s?user_id=42&offset=30"}`, field, endpoint))
		add("empty-stop", nil, empty, second)
		add("empty-refill", map[string]any{"limit": 1}, empty, second)
		add("later-malformed", map[string]any{"limit": 0}, first, json.RawMessage(`{}`))
		add("missing-list", nil, json.RawMessage(`{}`))
		add("null-list", nil, json.RawMessage(fmt.Sprintf(`{"%s":null}`, field)))
		add("later-invalid-record", map[string]any{"limit": 0}, first, json.RawMessage(fmt.Sprintf(`{"%s":[{"id":0,"user":{"id":7}}]}`, field)))
		rows = append(rows, row{Name: toolName + "-missing-current-identity", ToolName: toolName, Arguments: map[string]any{}, Bodies: []json.RawMessage{first}, NoIdentity: true})
		for index, args := range []any{nil, 7, "invalid", []any{}, true} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-root-%d", toolName, index), ToolName: toolName, Arguments: args, Bodies: []json.RawMessage{first, second}})
		}
	}
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
			if bodyIndex >= len(row.Bodies) {
				t.Fatalf("unexpected HTTP request %d", row.Requests)
			}
			endpoint := "illusts"
			if row.ToolName == "user_novels" {
				endpoint = "novels"
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
				if item.Name == "user_artworks" || item.Name == "user_novels" {
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
		if row.Name == "user_artworks-argument-22" || row.Name == "user_novels-argument-22" {
			t.Logf("%s actual RPC diagnostic: %s", row.Name, row.RPCError)
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
	}
	if len(tools) != 2 {
		t.Fatal("user works tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-user-works.json")
	if *migrationUpdateMCPUserWorks {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if *migrationSkipUserWorksViolationSelection {
		var reference struct {
			Tools map[string]json.RawMessage `json:"tools"`
			Cases []row                      `json:"cases"`
		}
		if err := json.Unmarshal(want, &reference); err != nil {
			t.Fatal(err)
		}
		if len(reference.Cases) != len(rows) {
			t.Fatal("user works case count differs")
		}
		for index := range rows {
			current := &rows[index]
			if current.Name != "user_artworks-argument-22" && current.Name != "user_novels-argument-22" {
				continue
			}
			expected := reference.Cases[index]
			if expected.Name != current.Name || expected.RPCError != migrationUserWorksLimitViolation {
				t.Fatalf("%s frozen diagnostic differs", current.Name)
			}
			if current.RPCError != migrationUserWorksLimitViolation && current.RPCError != migrationUserWorksPageViolation {
				t.Fatalf("%s diagnostic is not either independently observed Go violation: %s", current.Name, current.RPCError)
			}
			current.RPCError = expected.RPCError
			t.Logf("%s exact violation selection remains unverified; complete diagnostic content and all other fields are compared", current.Name)
		}
		data, err = json.MarshalIndent(struct {
			Tools map[string]json.RawMessage `json:"tools"`
			Cases []row                      `json:"cases"`
		}{tools, rows}, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		data = append(data, '\n')
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP user works differ from fixed Go reference")
	}
}

func TestMigrationMCPUserWorksOverlappingSchemaViolationEvidence(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{}, pixivmcp.Account{})
	peer, local := mcp.NewInMemoryTransports()
	done := make(chan error, 1)
	go func() { done <- server.Run(ctx, local) }()
	session, err := mcp.NewClient(&mcp.Implementation{Name: "migration-diagnostic-evidence", Version: "0"}, nil).Connect(ctx, peer, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		_ = session.Close()
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Error("diagnostic evidence session did not stop")
		}
	}()
	for _, tool := range []string{"user_artworks", "user_novels"} {
		observed := map[string]bool{}
		for range 128 {
			_, err := session.CallTool(ctx, &mcp.CallToolParams{Name: tool, Arguments: map[string]any{"page": 0, "limit": -1}})
			if err == nil {
				t.Fatal("overlapping invalid schema input unexpectedly succeeded")
			}
			diagnostic := err.Error()
			if diagnostic != migrationUserWorksLimitViolation && diagnostic != migrationUserWorksPageViolation {
				t.Fatalf("%s unexpected complete diagnostic: %s", tool, diagnostic)
			}
			if !observed[diagnostic] {
				t.Logf("%s-argument-22 independently observed full error: %s", tool, diagnostic)
			}
			observed[diagnostic] = true
		}
		if len(observed) != 2 {
			t.Fatalf("%s bounded 128-call evidence did not reproduce both complete violations", tool)
		}
	}
}
