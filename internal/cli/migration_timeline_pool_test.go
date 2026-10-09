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
	timelinecmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/timeline"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateTimelinePool = flag.Bool("migration-update-timeline-pool", false, "capture timeline pool replay and output ownership")

func TestMigrationTimelinePoolPreservesCommitReplayWriterAndLease(t *testing.T) {
	type request struct {
		ID     int64               `json:"id"`
		Offset string              `json:"offset"`
		Path   string              `json:"path"`
		Query  map[string][]string `json:"query"`
	}
	type state struct {
		ID       int64 `json:"id"`
		Revision int64 `json:"revision"`
		Frozen   bool  `json:"frozen"`
		Selected bool  `json:"selected"`
	}
	type row struct {
		Scenario    string    `json:"scenario"`
		Mode        string    `json:"mode"`
		Kind        string    `json:"kind"`
		Name        string    `json:"name"`
		Operation   string    `json:"operation"`
		ContentType string    `json:"content_type"`
		Source      string    `json:"source"`
		Args        []string  `json:"args"`
		Opens       []int64   `json:"opens"`
		Requests    []request `json:"requests"`
		Closes      int       `json:"closes"`
		UnderLease  bool      `json:"under_lease"`
		States      []state   `json:"states"`
		Stdout      string    `json:"stdout"`
		Stderr      string    `json:"stderr"`
		Exit        int       `json:"exit"`
	}
	sources := migrationTimelineReadBodies(t)
	var rows []row
	cases := []struct{ name, scenario, mode, operation, kind, contentType, source string }{
		{"following-human", "success", "human", "following", "artwork", "", "following-artwork"},
		{"following-json", "success", "json", "following", "artwork", "", "following-artwork"},
		{"following-ndjson", "success", "ndjson", "following", "artwork", "", "following-artwork"},
		{"latest-novel-json", "success", "json", "latest", "novel", "", "latest-novel"},
		{"latest-manga-ndjson", "success", "ndjson", "latest", "artwork", "manga", "latest-artwork"},
		{"following-human-after-page", "after_page", "human", "following", "artwork", "", "following-artwork"},
		{"following-json-after-page", "after_page", "json", "following", "artwork", "", "following-artwork"},
		{"following-ndjson-after-page", "after_page", "ndjson", "following", "artwork", "", "following-artwork"},
		{"following-json-before-output", "before_output", "json", "following", "artwork", "", "following-artwork"},
		{"following-novel-ndjson-before-output", "before_output", "ndjson", "following", "novel", "", "following-novel"},
		{"filtered-empty-before-output", "empty_before_output", "ndjson", "following", "artwork", "manga", "following-artwork-manga-filter-empty"},
		{"all-rate-limited", "all_rate_limited", "json", "following", "artwork", "", "following-artwork"},
		{"not-retryable", "not_retryable", "human", "latest", "novel", "", "latest-novel"},
		{"broken-human", "broken", "human", "following", "artwork", "", "following-artwork"},
		{"broken-json", "broken", "json", "following", "artwork", "", "following-artwork"},
		{"short-novel-json", "short", "json", "latest", "novel", "", "latest-novel"},
		{"other-ndjson", "other", "ndjson", "following", "artwork", "", "following-artwork"},
		{"latest-novel-json-after-page", "after_page", "json", "latest", "novel", "", "latest-novel"},
	}
	for _, tc := range cases {
		scenario, mode, kind := tc.scenario, tc.mode, tc.kind

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
		current := row{Scenario: scenario, Mode: mode, Kind: kind, Name: tc.name, Operation: tc.operation, ContentType: tc.contentType, Source: tc.source, Args: []string{}, Opens: []int64{}, Requests: []request{}, States: []state{}}
		active := 0
		output := &migrationSearchOutputWriter{failure: scenario}
		spoolDirectory := t.TempDir()
		t.Setenv("TEMP", spoolDirectory)
		t.Setenv("TMP", spoolDirectory)
		t.Setenv("TMPDIR", spoolDirectory)
		counts := map[string]int{}
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

				offset := req.URL.Query().Get("offset")
				if offset == "" {
					offset = req.URL.Query().Get("max_illust_id")
				}
				if offset == "" {
					offset = req.URL.Query().Get("max_novel_id")
				}
				observed := request{id, offset, req.URL.Path, req.URL.Query()}
				current.Requests = append(current.Requests, observed)
				countKey := fmt.Sprintf("%d:%s:%s", id, observed.Path, observed.Offset)
				counts[countKey]++
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
				if _, ok := sources[tc.source]; !ok {
					t.Fatalf("unexpected request %s", req.URL)
				}
				payload = string(sources[tc.source][index])
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
				encoded, err := json.Marshal(body)
				if err != nil {
					t.Fatal(err)
				}
				payload = string(encoded)

				if scenario == "not_retryable" {
					status = 401
				} else if scenario == "all_rate_limited" || id == 42 && (scenario == "before_output" || (scenario == "after_page" || scenario == "empty_before_output") && observed.Offset == "30") {
					status = 429
					if counts[countKey]%2 == 1 {
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
		command := timelinecmd.New(deps.Data{Input: strings.NewReader(""), Output: writer, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
		}})
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(command)
		root.SetOut(writer)
		root.SetErr(&diagnostics)
		args := []string{"timeline", tc.operation, "--type=" + kind, "--limit=0"}
		if tc.contentType != "" {
			args = append(args, "--content-type="+tc.contentType)
		}

		if mode != "human" {
			args = append(args, "--"+mode)
		}
		current.Args = append([]string{}, args[2:]...)
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
			t.Fatal("timeline leaked account lease")
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}

		if scenario == "success" && current.Exit != 0 {
			t.Fatal("timeline success failed")
		}

		rows = append(rows, current)
	}
	recommendedFixture(t, "cli-timeline-pool.json", rows, *updateTimelinePool)
}
