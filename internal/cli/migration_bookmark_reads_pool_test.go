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
	"strconv"
	"strings"
	"testing"
	"time"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateBookmarkReadsPool = flag.Bool("migration-update-bookmark-reads-pool", false, "capture bookmark detail and tag pool replay, identity, output ownership, and lease")

func TestMigrationBookmarkReadsPoolPreservesReplayCommitIdentityWriterAndLease(t *testing.T) {
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
		Operation  string    `json:"operation"`
		Mode       string    `json:"mode"`
		Kind       string    `json:"kind"`
		Target     int64     `json:"target"`
		Args       []string  `json:"args"`
		Opens      []int64   `json:"opens"`
		Requests   []request `json:"requests"`
		Closes     int       `json:"closes"`
		UnderLease bool      `json:"under_lease"`
		Committed  []bool    `json:"committed"`
		States     []state   `json:"states"`
		Error      string    `json:"error"`
		Stdout     string    `json:"stdout"`
		Stderr     string    `json:"stderr"`
		Exit       int       `json:"exit"`
	}
	sources := map[string][]json.RawMessage{
		"/v1/user/bookmark-tags/illust": bookmarkReadsBodies("tags", "artwork"),
		"/v1/user/bookmark-tags/novel":  bookmarkReadsBodies("tags", "novel"),
		"/v2/illust/bookmark/detail":    bookmarkReadsBodies("detail", "artwork"),
		"/v2/novel/bookmark/detail":     bookmarkReadsBodies("detail", "novel"),
	}
	recommendedFixture(t, "cli-bookmark-reads-pool-bodies.json", sources, *updateBookmarkReadsPool)
	var rows []row
	for _, operation := range []string{"detail", "tags"} {
		kinds := []string{"artwork", "novel"}
		targets := []int64{42, 99}
		if operation == "tags" {
			kinds, targets = append(kinds, "all"), []int64{0, 99}
		}
		for _, kind := range kinds {
			for _, scenario := range []string{"success", "after_page", "before_output", "not_retryable", "all_rate_limited", "other", "broken", "short", "empty_before_output", "after_artworks"} {
				if scenario == "after_page" && (operation == "detail" || kind == "novel") || scenario == "after_artworks" && (operation == "detail" || kind != "all") {
					continue
				}
				for _, mode := range []string{"human", "ndjson", "json", "auto"} {
					if operation == "detail" && mode == "ndjson" {
						continue
					}
					for _, target := range targets {
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
						current := row{Scenario: scenario, Operation: operation, Mode: mode, Kind: kind, Target: target, Args: []string{}, Opens: []int64{}, Requests: []request{}, States: []state{}, Committed: []bool{}}
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
								if req.Method != "GET" {
									t.Fatalf("unexpected bookmark read method %s", req.Method)
								}
								token := req.Header.Get("Authorization")
								id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
								if err != nil {
									return nil, err
								}
								targetID := req.URL.Query().Get("user_id")
								if operation == "detail" {
									targetID = req.URL.Query().Get("illust_id")
									if kind == "novel" {
										targetID = req.URL.Query().Get("novel_id")
									}
								}
								observed := request{id, req.URL.Query().Get("offset"), req.URL.Path, targetID}
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
								decoder := json.NewDecoder(strings.NewReader(payload))
								decoder.UseNumber()
								if err := decoder.Decode(&body); err != nil {
									t.Fatal(err)
								}
								if operation == "detail" {
									detail := body["bookmark_detail"].(map[string]any)
									detail["tags"] = []any{map[string]any{"name": fmt.Sprintf("item-account-%d", id), "is_registered": true}}
									if scenario == "empty_before_output" && id == 42 {
										body["bookmark_detail"] = nil
									}
								} else {
									if items, ok := body["bookmark_tags"].([]any); ok {
										for _, item := range items {
											item.(map[string]any)["name"] = fmt.Sprintf("item-account-%d", id)
										}
									}
									if scenario == "empty_before_output" && id == 42 {
										body["bookmark_tags"] = []any{}
									}
								}
								encoded, err := json.Marshal(body)
								if err != nil {
									t.Fatal(err)
								}
								payload = string(encoded)
								if scenario == "not_retryable" {
									status = 401
								} else if scenario == "all_rate_limited" || id == 42 && (scenario == "before_output" || (scenario == "after_page" || scenario == "empty_before_output") && observed.Offset == "30" || scenario == "after_artworks" && observed.Path == "/v1/user/bookmark-tags/novel") {
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
						var out io.Writer = writer
						if mode == "auto" {
							out = migrationBookmarkListsPipeWriter{Writer: writer}
						}
						var diagnostics bytes.Buffer
						command, prefix := bookmarkReadsCommand(operation, kind, strings.NewReader(""), out, &diagnostics, func(*bool) (bool, error) { return mode == "json", nil }, func(ctx context.Context, request deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
							if request.UserID != 0 {
								t.Fatal("bookmark read target selected authentication account")
							}
							return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, func(ctx context.Context, client *pixiv.Client) (bool, error) {
								committed, err := invoke(ctx, client)
								current.Committed = append(current.Committed, committed)
								return committed, err
							})
						})
						root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
						root.AddCommand(command)
						root.SetOut(out)
						root.SetErr(&diagnostics)
						if operation == "tags" {
							current.Args = append(current.Args, "--limit=0")
						}
						if target > 0 {
							current.Args = append(current.Args, strconv.FormatInt(target, 10))
						}
						if mode == "json" || mode == "ndjson" {
							current.Args = append(current.Args, "--"+mode)
						}
						args := append(prefix, current.Args...)
						root.SetArgs(args)
						executed, _, findErr := root.Find(args)
						if findErr != nil {
							t.Fatal(findErr)
						}
						err = root.Execute()
						if err != nil {
							current.Error = err.Error()
						}
						current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, commandWritesNDJSON(executed) || commandAutoWritesNDJSON(executed, out), commandWritesNDJSON(executed) || commandExplicitJSON(executed))
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
						if active != 0 || current.Closes != len(current.Opens) {
							t.Fatal("bookmark reads leaked account lease")
						}
						if err := db.Close(); err != nil {
							t.Fatal(err)
						}
						if scenario == "success" {
							if current.Exit != 0 || current.Error != "" || current.Stdout == "" || len(current.Opens) != 1 || len(current.Requests) == 0 || len(current.Committed) != 1 {
								t.Fatalf("bookmark reads success failed: %#v", current)
							}
							if current.UnderLease != (operation == "tags") || current.Committed[0] != (operation == "tags") {
								t.Fatalf("bookmark read output ownership differs: %#v", current)
							}
						}
						rows = append(rows, current)
					}
				}
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-reads-pool.json", rows, *updateBookmarkReadsPool)
}
