package cli

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
	"testing"
	"time"

	usercmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/user"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateUserWorksPool = flag.Bool("migration-update-user-works-pool", false, "capture recommendation pool replay and output ownership")

func TestMigrationUserWorksPoolPreservesCommitReplayWriterAndLease(t *testing.T) {
	type request struct {
		ID     int64  `json:"id"`
		Offset string `json:"offset"`
		Path   string `json:"path"`
		Target string `json:"target"`
	}
	type state struct {
		ID       int64 `json:"id"`
		Revision int64 `json:"revision"`
		Frozen   bool  `json:"frozen"`
		Selected bool  `json:"selected"`
	}
	type row struct {
		Scenario   string    `json:"scenario"`
		Mode       string    `json:"mode"`
		Kind       string    `json:"kind"`
		Target     int64     `json:"target"`
		Opens      []int64   `json:"opens"`
		Requests   []request `json:"requests"`
		Closes     int       `json:"closes"`
		UnderLease bool      `json:"under_lease"`
		States     []state   `json:"states"`
		Stdout     string    `json:"stdout"`
		Stderr     string    `json:"stderr"`
		Exit       int       `json:"exit"`
	}
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "cli-recommended-pool-bodies.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources map[string][]json.RawMessage
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	sources["/v1/user/illusts"] = sources["/v1/illust/recommended"]
	sources["/v1/user/novels"] = sources["/v1/novel/recommended"]
	for _, endpoint := range []string{"/v1/user/illusts", "/v1/user/novels"} {
		for index, body := range sources[endpoint] {
			sources[endpoint][index] = json.RawMessage(strings.ReplaceAll(strings.ReplaceAll(string(body), "/v1/illust/recommended", "/v1/user/illusts"), "/v1/novel/recommended", "/v1/user/novels"))
		}
	}
	var rows []row
	for _, scenario := range []string{"success", "after_page", "before_output", "not_retryable", "all_rate_limited", "other", "broken", "short", "empty_before_output"} {
		for _, mode := range []string{"human", "ndjson", "json"} {
			for _, kind := range []string{"artworks", "novels"} {
				for _, target := range []int64{0, 99} {

					ctx := context.Background()
					db, err := database.Open(t.TempDir())
					if err != nil {
						t.Fatal(err)
					}
					for _, id := range []int64{42, 43} {
						if err := db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
							t.Fatal(err)
						}
					}
					if err := db.SetAllPixivSchedulable(ctx, true); err != nil {
						t.Fatal(err)
					}
					current := row{Scenario: scenario, Mode: mode, Kind: kind, Target: target, Opens: []int64{}, Requests: []request{}, States: []state{}}
					active := 0
					output := &migrationSearchOutputWriter{failure: scenario}
					spoolDirectory := t.TempDir()
					t.Setenv("TEMP", spoolDirectory)
					t.Setenv("TMP", spoolDirectory)
					t.Setenv("TMPDIR", spoolDirectory)
					counts := map[request]int{}
					transport := migrationDateTransport(func(req *http.Request) (*http.Response, error) {
						header := http.Header{"Content-Type": {"application/json"}}
						status := 200
						payload := ""
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
								t.Fatal("refresh used stale credentials")
							}
							current.Opens = append(current.Opens, id)
							active++
							payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id)
						} else {
							token := req.Header.Get("Authorization")
							id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
							if err != nil {
								return nil, err
							}

							observed := request{id, req.URL.Query().Get("offset"), req.URL.Path, req.URL.Query().Get("user_id")}
							current.Requests = append(current.Requests, observed)
							counts[observed]++
							stored, err := db.GetPixiv(ctx, id)
							if err != nil {
								return nil, err
							}
							if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", id) {
								t.Fatal("fetch preceded refresh persistence")
							}
							index := 0
							if observed.Offset == "30" {
								index = 1
							}
							if _, ok := sources[observed.Path]; !ok {
								t.Fatalf("unexpected request %s", req.URL)
							}
							payload = string(sources[observed.Path][index])
							var body map[string]any
							if err := json.Unmarshal([]byte(payload), &body); err != nil {
								t.Fatal(err)
							}
							for _, key := range []string{"illusts", "novels"} {
								if items, ok := body[key].([]any); ok {
									for _, item := range items {
										item.(map[string]any)["title"] = fmt.Sprintf("item-account-%d", id)
									}
								}
							}
							if scenario == "empty_before_output" && id == 42 {
								for _, key := range []string{"illusts", "novels"} {
									if _, ok := body[key]; ok {
										body[key] = []any{}
									}
								}
							}
							encoded, err := json.Marshal(body)
							if err != nil {
								t.Fatal(err)
							}
							payload = string(encoded)

							if scenario == "not_retryable" {
								status = 401
							} else if scenario == "all_rate_limited" || id == 42 && (scenario == "before_output" || (scenario == "after_page" || scenario == "empty_before_output") && observed.Offset == "30") {
								status = 429
								if counts[observed]%2 == 1 {
									header.Set("Retry-After", "0")
								} else {
									header.Set("Retry-After", "120")
								}
							}
						}
						return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(strings.NewReader(payload)), Request: req}, nil
					})
					facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: pool.NewGate(), LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
						return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
					}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
						return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
					}, CloseClient: func(*pixiv.Client) error { current.Closes++; active--; return nil }})
					writer := &migrationSearchLeaseWriter{output: output, active: &active}
					var diagnostics bytes.Buffer
					command := usercmd.New(usercmd.Dependencies{Input: strings.NewReader(""), Output: writer, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ usercmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
						return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
					}})
					root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
					root.AddCommand(command)
					root.SetOut(writer)
					root.SetErr(&diagnostics)
					args := []string{"user", kind, "--limit=0"}
					if target > 0 {
						args = append(args, strconv.FormatInt(target, 10))
					}
					if mode != "human" {
						args = append(args, "--"+mode)
					}
					root.SetArgs(args)
					err = root.Execute()
					current.Exit = (app{out: writer, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
					entries, readErr := os.ReadDir(spoolDirectory)
					if readErr != nil || len(entries) != 0 {
						t.Fatalf("private spools leaked: %v %v", entries, readErr)
					}
					current.Stdout, current.Stderr, current.UnderLease = output.out.String(), diagnostics.String(), writer.underLease
					for _, id := range []int64{42, 43} {
						stored, err := db.GetPixiv(ctx, id)
						if err != nil {
							t.Fatal(err)
						}
						current.States = append(current.States, state{id, stored.CredentialRevision, stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), stored.PoolLastSelected})
					}
					if active != 0 {
						t.Fatal("user-works leaked account lease")
					}
					if err := db.Close(); err != nil {
						t.Fatal(err)
					}

					if scenario == "success" && current.Exit != 0 {
						t.Fatal("user-works success failed")
					}

					rows = append(rows, current)
				}
			}
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "cli-user-works-pool.json")
	if *updateUserWorksPool {
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
		t.Fatal("user-works pool differs from Go reference")
	}
}
