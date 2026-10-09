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
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPMyPixivPool = flag.Bool("migration-update-mcp-mypixiv-pool", false, "capture saved account MCP pool replay contracts")

func TestMigrationMCPMyPixivPoolReplaysCompleteCollection(t *testing.T) {
	type state struct {
		ID       int64  `json:"id"`
		Revision int64  `json:"revision"`
		Token    string `json:"token"`
		Frozen   bool   `json:"frozen"`
		Selected bool   `json:"selected"`
	}
	type contentRequest struct {
		ID     int64      `json:"id"`
		Method string     `json:"method"`
		Path   string     `json:"path"`
		Query  url.Values `json:"query"`
	}
	type row struct {
		ToolName string `json:"tool_name"`
		Mode     string `json:"mode"`

		Results  []json.RawMessage `json:"results"`
		Opens    []int64           `json:"opens"`
		Requests []contentRequest  `json:"requests"`
		Closes   int               `json:"closes"`
		States   []state           `json:"states"`
		Reusable bool              `json:"reusable"`
	}
	var data []byte
	var err error
	rows := []row{}
	bodies := map[string]map[string]json.RawMessage{}
	for _, toolName := range []string{"mypixiv_illusts", "mypixiv_novels", "mypixiv_users"} {
		field, endpoint, continuation, base := "novels", "/v1/novel/mypixiv", "offset", url.Values{}
		if toolName == "mypixiv_illusts" {
			field, endpoint = "illusts", "/v2/illust/mypixiv"
		}
		if toolName == "mypixiv_users" {
			field, endpoint = "user_previews", "/v1/user/mypixiv"
			base = url.Values{"user_id": {"42"}, "filter": {"for_android"}}
		}
		nextQuery := url.Values{}
		for key, value := range base {
			nextQuery[key] = value
		}
		nextQuery.Set(continuation, "30")
		body := json.RawMessage(fmt.Sprintf(`{"%s":[{"id":22,"type":"illust","title":"first metadata","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":"https://app-api.pixiv.net%s?%s"}`, field, endpoint, nextQuery.Encode()))
		finalBody := json.RawMessage(fmt.Sprintf(`{"%s":[{"id":23,"type":"illust","title":"final metadata","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`, field))
		bodies[toolName] = map[string]json.RawMessage{"first": body, "final": finalBody}
		if toolName == "mypixiv_users" {
			body = json.RawMessage(strings.ReplaceAll(strings.ReplaceAll(string(body), `"user":{"id":7}`, `"user":{"id":22,"name":"first metadata"}`), `"id":22,"type":"illust","title":"first metadata",`, ``))
			finalBody = json.RawMessage(strings.ReplaceAll(strings.ReplaceAll(string(finalBody), `"user":{"id":7}`, `"user":{"id":23,"name":"final metadata"}`), `"id":23,"type":"illust","title":"final metadata",`, ``))
			bodies[toolName] = map[string]json.RawMessage{"first": body, "final": finalBody}
		}
		for _, mode := range []string{"replay_success", "replay_malformed"} {
			t.Run(toolName+"-"+mode, func(t *testing.T) {
				ctx := context.Background()
				db, err := database.Open(t.TempDir())
				if err != nil {
					t.Fatal(err)
				}
				defer db.Close()
				for _, id := range []int64{42, 43} {
					if err = db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
						t.Fatal(err)
					}
				}
				if err = db.SetAllPixivSchedulable(ctx, true); err != nil {
					t.Fatal(err)
				}
				r := row{ToolName: toolName, Mode: mode, Results: []json.RawMessage{}, Opens: []int64{}, Requests: []contentRequest{}, States: []state{}}
				var mu sync.Mutex
				transport := migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
					mu.Lock()
					defer mu.Unlock()
					payload, status := string(body), 200
					header := http.Header{"Content-Type": {"application/json"}}
					if req.URL.Host == "oauth.secure.pixiv.net" {
						if err := req.ParseForm(); err != nil {
							return nil, err
						}
						token := req.Form.Get("refresh_token")
						id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
						if err != nil {
							return nil, err
						}
						stored, err := db.GetPixiv(ctx, id)
						if err != nil {
							return nil, err
						}
						if token != string(stored.RefreshTokenCopy()) {
							t.Error("refresh did not use persisted token")
						}
						r.Opens = append(r.Opens, id)
						payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id)
					} else {
						token := req.Header.Get("Authorization")
						id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
						if err != nil {
							return nil, err
						}
						stored, err := db.GetPixiv(ctx, id)
						if err != nil {
							return nil, err
						}
						if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", id) {
							t.Error("content requested before rotation persisted")
						}
						if req.Method != "GET" || req.URL.Path != endpoint {
							t.Errorf("unexpected content request: %s %s", req.Method, req.URL)
						}
						r.Requests = append(r.Requests, contentRequest{id, req.Method, req.URL.Path, req.URL.Query()})
						count := 0
						for _, requested := range r.Requests {
							if requested.ID == id {
								count++
							}
						}
						if id == 42 && count > 1 {
							status = 429
							if count%2 == 0 {
								header.Set("Retry-After", "0")
							} else {
								header.Set("Retry-After", "120")
							}
						} else if req.URL.Query().Get(continuation) != "" {
							if mode == "replay_malformed" && id == 43 {
								payload = `{}`
							} else {
								payload = string(finalBody)
							}
						} else if id == 42 {
							payload = strings.ReplaceAll(payload, "first metadata", "discarded account42 metadata")
						}

					}
					return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(strings.NewReader(payload)), Request: req}, nil
				})
				gate := pool.NewGate()
				facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: gate,
					LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
						return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
					},
					Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
						return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
					},
					CloseClient: func(*pixiv.Client) error { mu.Lock(); defer mu.Unlock(); r.Closes++; return nil },
				})
				server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					return facade.Use(ctx, pixivapp.Request{UserID: 99, Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
				}}, pixivmcp.Account{})
				runCtx, cancel := context.WithCancel(ctx)
				clientTransport, serverTransport := mcp.NewInMemoryTransports()
				done := make(chan error, 1)
				go func() { done <- server.Run(runCtx, serverTransport) }()
				capture := &migrationMCPCapture{}
				session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(runCtx, migrationMCPCaptureTransport{clientTransport, capture}, nil)
				if err != nil {
					cancel()
					t.Fatal(err)
				}
				for range 2 {
					_, _ = session.CallTool(runCtx, &mcp.CallToolParams{Name: toolName, Arguments: func() map[string]any {
						args := map[string]any{"limit": 0}
						return args
					}()})
					capture.mu.Lock()
					r.Results = append(r.Results, append(json.RawMessage(nil), capture.result...))
					capture.mu.Unlock()
				}
				for _, result := range r.Results {
					var out struct {
						IsError    bool `json:"isError"`
						Structured struct {
							Records []struct {
								Title string `json:"title"`
								User  struct {
									Name string `json:"name"`
								} `json:"user"`
							} `json:"records"`
						} `json:"structuredContent"`
					}
					if err := json.Unmarshal(result, &out); err != nil {
						t.Fatal(err)
					}
					if toolName == "mypixiv_users" {
						for i := range out.Structured.Records {
							out.Structured.Records[i].Title = out.Structured.Records[i].User.Name
						}
					}
					if mode == "replay_success" && (out.IsError || len(out.Structured.Records) != 2 || out.Structured.Records[0].Title != "first metadata" || out.Structured.Records[1].Title != "final metadata") {
						t.Fatalf("pool did not replay complete successful collection: %s", result)
					}
					if mode == "replay_malformed" && (!out.IsError || len(out.Structured.Records) != 0 || bytes.Contains(result, []byte("first metadata")) || bytes.Contains(result, []byte("discarded account42 metadata"))) {
						t.Fatalf("pool malformed replay was successful: %s", result)
					}
				}
				wantRequests := []contentRequest{{42, "GET", endpoint, base}, {42, "GET", endpoint, nextQuery}, {42, "GET", endpoint, nextQuery}, {43, "GET", endpoint, base}, {43, "GET", endpoint, nextQuery}, {43, "GET", endpoint, base}, {43, "GET", endpoint, nextQuery}}
				if toolName == "mypixiv_users" {
					for i := range wantRequests {
						if wantRequests[i].ID == 43 {
							q := url.Values{}
							for k, v := range wantRequests[i].Query {
								q[k] = v
							}
							q.Set("user_id", "43")
							wantRequests[i].Query = q
						}
					}
				}
				if !reflect.DeepEqual(r.Requests, wantRequests) {
					t.Fatalf("pool requests differ: got=%v want=%v", r.Requests, wantRequests)
				}
				if !reflect.DeepEqual(r.Opens, []int64{42, 43, 43}) || r.Closes != 3 {
					t.Fatalf("pool opens/closes differ: %v/%d", r.Opens, r.Closes)
				}
				_ = session.Close()
				cancel()
				select {
				case <-done:
				case <-time.After(5 * time.Second):
					t.Fatal("MCP pool session did not stop")
				}
				probe, stop := context.WithTimeout(ctx, time.Second)
				if err := gate.Acquire(probe); err == nil {
					r.Reusable = true
					gate.Release()
				}
				stop()
				for _, id := range []int64{42, 43} {
					a, err := db.GetPixiv(ctx, id)
					if err != nil {
						t.Fatal(err)
					}
					r.States = append(r.States, state{id, a.CredentialRevision, string(a.RefreshTokenCopy()), a.PoolFrozenUntil != nil && *a.PoolFrozenUntil > time.Now().Unix(), a.PoolLastSelected})
				}
				if !r.Reusable || !reflect.DeepEqual(r.States, []state{{42, 2, "fixture-rotated-42", true, false}, {43, 3, "fixture-rotated-43", false, true}}) {
					t.Fatalf("pool state/reuse differs: %+v reusable=%v", r.States, r.Reusable)
				}
				rows = append(rows, r)
			})
		}
	}
	if t.Failed() {
		return
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-mypixiv-pool.json")
	if *updateMCPMyPixivPool {
		bodyData, err := json.MarshalIndent(bodies, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(strings.Replace(path, ".json", "-bodies.json", 1), append(bodyData, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	bodyData, err := json.MarshalIndent(bodies, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	wantBodies, err := os.ReadFile(strings.Replace(path, ".json", "-bodies.json", 1))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(append(bodyData, '\n'), wantBodies) {
		t.Fatal("timeline pool bodies differ from fixed Go reference")
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP pool replay differs from fixed Go reference")
	}
}
