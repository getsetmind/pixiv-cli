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

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	commentcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/comment"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCommentMutationsPool = flag.Bool("migration-update-comment-mutations-pool", false, "capture relationship pool replay and output ownership")

func TestMigrationCommentMutationsPoolPreservesCommitReplayWriterAndLease(t *testing.T) {
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
		Operation  string    `json:"operation"`
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
	var rows []row

	type config struct{ operation, kind, mode, scenario string }
	var configs []config
	for _, operation := range []string{"create", "delete", "reply", "stamp"} {
		for _, kind := range []string{"artwork", "novel"} {
			for _, scenario := range []string{"success", "before_output"} {
				configs = append(configs, config{operation, kind, "json", scenario})
			}
		}
	}
	for _, mode := range []string{"human", "json"} {
		for _, scenario := range []string{"other", "broken", "short"} {
			configs = append(configs, config{"create", "artwork", mode, scenario})
		}
	}
	for _, c := range configs {
		operation, kind, mode, scenario, target := c.operation, c.kind, c.mode, c.scenario, int64(99)

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
		current := row{Operation: operation, Scenario: scenario, Mode: mode, Kind: kind, Target: target, Opens: []int64{}, Requests: []request{}, States: []state{}}
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
				if req.Method != "POST" {
					t.Fatal("mutation method is not POST")
				}
				if err := req.ParseForm(); err != nil {
					t.Fatal(err)
				}
				token := req.Header.Get("Authorization")
				id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
				if err != nil {
					return nil, err
				}

				observed := request{id, "", req.URL.Path, func() string {
					if operation == "delete" {
						return req.PostForm.Get("comment_id")
					}
					if kind == "novel" {
						return req.PostForm.Get("novel_id")
					}
					return req.PostForm.Get("illust_id")
				}()}
				current.Requests = append(current.Requests, observed)
				counts[observed]++
				stored, err := db.GetPixiv(ctx, id)
				if err != nil {
					return nil, err
				}
				if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", id) {
					t.Fatal("fetch preceded refresh persistence")
				}
				payload = `{"comment_id":123}`

				if scenario == "not_retryable" {
					status = 401
				} else if scenario == "all_rate_limited" || id == 42 && (scenario == "before_output" || (scenario == "after_page" || scenario == "empty_before_output") && (observed.Offset == "30" || kind == "stamps")) {
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
		command := commentcmd.New(deps.Data{Input: strings.NewReader(""), Output: writer, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
		}})
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(command)
		root.SetOut(writer)
		root.SetErr(&diagnostics)
		args := []string{"comment", operation, "--type=" + kind, strconv.FormatInt(target, 10)}
		if operation == "create" || operation == "reply" {
			args = append(args, "--comment=body")
		}
		if operation == "reply" {
			args = append(args, "--parent-comment-id=9")
		}
		if operation == "stamp" {
			args = append(args, "--stamp-id=7")
		}

		if mode != "human" {
			args = append(args, "--"+mode)
		}
		root.SetArgs(args)
		err = root.Execute()
		current.Exit = (app{out: writer, errOut: &diagnostics}).exitWithNDJSONScope(err, false, mode != "human")
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
			t.Fatal("comment-mutations leaked account lease")
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}

		if scenario == "success" && current.Exit != 0 {
			t.Fatal("comment-mutations success failed")
		}

		rows = append(rows, current)
	}

	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "cli-comment-mutations-pool.json")
	if *updateCommentMutationsPool {
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
		t.Fatal("comment-mutations pool differs from Go reference")
	}
}
