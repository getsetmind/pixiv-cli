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

var updateMCPBookmarkReadsPool = flag.Bool("migration-update-mcp-bookmark-reads-pool", false, "capture saved account MCP pool replay contracts")

func TestMigrationMCPBookmarkReadsPoolPreservesResolvedTargetAndReplay(t *testing.T) {
	type state struct {
		ID       int64  `json:"id"`
		Revision int64  `json:"revision"`
		Token    string `json:"token"`
		Frozen   bool   `json:"frozen"`
		Selected bool   `json:"selected"`
	}
	type row struct {
		ToolName string            `json:"tool_name"`
		Explicit bool              `json:"explicit"`
		Targets  []int64           `json:"targets"`
		Mode     string            `json:"mode"`
		Body     json.RawMessage   `json:"body"`
		Results  []json.RawMessage `json:"results"`
		Opens    []int64           `json:"opens"`
		Paths    []string          `json:"paths"`
		Queries  []string          `json:"queries"`
		Requests []int64           `json:"requests"`
		Closes   int               `json:"closes"`
		States   []state           `json:"states"`
		Reusable bool              `json:"reusable"`
	}
	var data []byte
	var err error
	rows := []row{}
	for _, toolName := range []string{"bookmark_detail", "novel_bookmark_detail", "bookmark_tags", "novel_bookmark_tags", "bookmark_tags_all"} {
		detail := strings.HasSuffix(toolName, "detail")
		explicitModes := []bool{false, true}
		if detail {
			explicitModes = []bool{true}
		}
		for _, explicit := range explicitModes {
			endpoint := "/v1/user/bookmark-tags/illust"
			if toolName == "novel_bookmark_tags" {
				endpoint = "/v1/user/bookmark-tags/novel"
			}
			if detail {
				endpoint = "/v2/illust/bookmark/detail"
				if toolName == "novel_bookmark_detail" {
					endpoint = "/v2/novel/bookmark/detail"
				}
			}
			body := json.RawMessage(`{"bookmark_tags":[{"name":"same","count":2},{"name":"artwork","count":3}],"next_url":"https://app-api.pixiv.net/v1/user/bookmark-tags/illust?user_id=42&offset=30"}`)
			if toolName == "novel_bookmark_tags" {
				body = json.RawMessage(`{"bookmark_tags":[{"name":"same","count":2},{"name":"novel","count":5}],"next_url":null}`)
			}
			if detail {
				body = json.RawMessage(`{"bookmark_detail":{"is_bookmarked":true,"restrict":"private","tags":[{"name":"chosen","is_registered":true}]}}`)
			}

			for _, mode := range []string{"replay_success", "replay_malformed"} {
				t.Run(mode, func(t *testing.T) {
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
					r := row{ToolName: toolName, Explicit: explicit, Paths: []string{}, Queries: []string{}, Targets: []int64{}, Mode: mode, Body: body, Results: []json.RawMessage{}, Opens: []int64{}, Requests: []int64{}, States: []state{}}
					throttled := int64(42)
					replayed := int64(43)
					if !explicit {
						throttled, replayed = 43, 42
					}
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
							r.Requests = append(r.Requests, id)
							r.Paths = append(r.Paths, req.URL.Path)
							r.Queries = append(r.Queries, req.URL.RawQuery)
							targetKey := "user_id"
							if toolName == "bookmark_detail" {
								targetKey = "illust_id"
							}
							if toolName == "novel_bookmark_detail" {
								targetKey = "novel_id"
							}
							target, err := strconv.ParseInt(req.URL.Query().Get(targetKey), 10, 64)
							if err != nil {
								t.Fatal(err)
							}
							r.Targets = append(r.Targets, target)
							if req.URL.Path != endpoint && !(toolName == "bookmark_tags_all" && req.URL.Path == "/v1/user/bookmark-tags/novel") {
								t.Fatalf("unexpected endpoint %s", req.URL)
							}
							count := 0
							for _, requested := range r.Requests {
								if requested == id {
									count++
								}
							}
							throttle := detail || toolName == "novel_bookmark_tags" || (toolName == "bookmark_tags_all" && strings.HasSuffix(req.URL.Path, "/novel")) || (toolName == "bookmark_tags" && req.URL.Query().Get("offset") != "")
							if id == throttled && throttle {
								status = 429
								firstThrottle := 2
								if detail || toolName == "novel_bookmark_tags" {
									firstThrottle = 1
								}
								if toolName == "bookmark_tags_all" {
									firstThrottle = 3
								}
								if count == firstThrottle {
									header.Set("Retry-After", "0")
								} else {
									header.Set("Retry-After", "120")
								}
							} else if id == replayed && mode == "replay_malformed" && throttle {
								payload = `{}`
								if detail {
									payload = `{"bookmark_detail":{"is_bookmarked":"wrong"}}`
								}
							} else if toolName == "bookmark_tags_all" && strings.HasSuffix(req.URL.Path, "/novel") {
								payload = `{"bookmark_tags":[{"name":"same","count":2},{"name":"novel","count":5}],"next_url":null}`
							} else if req.URL.Query().Get("offset") != "" {
								payload = `{"bookmark_tags":[{"name":"later","count":4}],"next_url":null}`
							} else if id == throttled && !detail {
								payload = strings.ReplaceAll(payload, `"name":"same"`, `"name":"discarded account metadata"`)
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
					for callIndex := range 2 {
						args := map[string]any{"limit": 0}
						if detail {
							args = map[string]any{"illust_id": 99}
							if toolName == "novel_bookmark_detail" {
								args = map[string]any{"novel_id": 99}
							}
						} else if explicit {
							args["user_id"] = 7
						}
						_, _ = session.CallTool(runCtx, &mcp.CallToolParams{Name: toolName, Arguments: args})
						capture.mu.Lock()
						r.Results = append(r.Results, append(json.RawMessage(nil), capture.result...))
						capture.mu.Unlock()
						var result struct {
							IsError    bool `json:"isError"`
							Structured struct {
								Tags       []json.RawMessage `json:"bookmark_tags"`
								DetailTags []json.RawMessage `json:"tags"`
								Bookmarked bool              `json:"bookmarked"`
							} `json:"structuredContent"`
						}
						if err := json.Unmarshal(r.Results[len(r.Results)-1], &result); err != nil {
							t.Fatal(err)
						}
						if result.IsError != (mode == "replay_malformed") {
							t.Fatalf("%s %s explicit=%v call=%d unexpected error status: %s", toolName, mode, explicit, callIndex, r.Results[len(r.Results)-1])
						}
						expectedTags := 3
						if toolName == "novel_bookmark_tags" {
							expectedTags = 2
						}
						if toolName == "bookmark_tags_all" {
							expectedTags = 5
						}
						if detail {
							expectedTags = 1
						}
						if mode == "replay_malformed" {
							expectedTags = 0
						}
						actualTags := len(result.Structured.Tags)
						if detail {
							actualTags = len(result.Structured.DetailTags)
							if result.Structured.Bookmarked != (mode == "replay_success") {
								t.Fatalf("unexpected bookmark state: %s", r.Results[len(r.Results)-1])
							}
						}
						if actualTags != expectedTags {
							t.Fatalf("%s %s returned %d tags, want %d: %s", toolName, mode, actualTags, expectedTags, r.Results[len(r.Results)-1])
						}
						if callIndex == 0 {
							if len(r.Requests) < 3 || r.Requests[0] != throttled || r.Requests[len(r.Requests)-1] != replayed {
								t.Fatalf("%s did not reach planned replay: %v", toolName, r.Requests)
							}
							if toolName == "bookmark_tags" || toolName == "bookmark_tags_all" {
								continued := false
								for index, query := range r.Queries {
									if strings.Contains(query, "offset=30") && r.Requests[index] == replayed {
										continued = true
									}
								}
								if !continued {
									t.Fatal("missing replayed artwork-tag continuation")
								}
							}
							if toolName == "bookmark_tags_all" && r.Paths[len(r.Paths)-1] != "/v1/user/bookmark-tags/novel" {
								t.Fatal("aggregate did not reach novel tags")
							}
						}

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
					rows = append(rows, r)
				})
			}
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
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-bookmark-reads-pool.json")
	if *updateMCPBookmarkReadsPool {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP pool replay differs from fixed Go reference")
	}
}
