package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	rankingcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/ranking"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCLINovelRanking = flag.Bool("migration-update-cli-novel-ranking", false, "capture ranking CLI output and startup")

func TestMigrationNovelRankingPreservesLogicalPagesOutputsAndStartup(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "novel-ranking.json"))
	if err != nil {
		t.Fatal(err)
	}
	type source struct {
		Name   string            `json:"name"`
		Mode   string            `json:"mode"`
		Date   string            `json:"date"`
		Bodies []json.RawMessage `json:"bodies"`
	}
	var sources []source
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type candidate struct {
		Source source
		Extra  []string
	}
	var candidates []candidate
	for _, source := range sources {
		if strings.HasPrefix(source.Name, "cursor:") || strings.HasPrefix(source.Name, "binding:") {
			continue
		}
		candidates = append(candidates, candidate{Source: source})
	}
	for _, extra := range [][]string{{"--limit=0"}, {"--limit=1"}, {"--limit=2", "--page=2"}, {"--limit=-1"}, {"--page=0"}, {"--page=1"}, {"--limit=0", "--page=2"}, {"--limit=3", "--page=4000000000000000000"}, {"--date="}, {"--date=2026-09-01"}, {"--type=invalid"}, {"extra"}} {
		candidates = append(candidates, candidate{Source: sources[0], Extra: extra})
	}
	type row struct {
		Args          []string          `json:"args"`
		Bodies        []json.RawMessage `json:"bodies"`
		Queries       []url.Values      `json:"queries"`
		Error         string            `json:"error"`
		Stdout        string            `json:"stdout"`
		Stderr        string            `json:"stderr"`
		Exit          int               `json:"exit"`
		StartupStderr string            `json:"startup_stderr"`
		StartupExit   int               `json:"startup_exit"`
		Config        bool              `json:"config"`
		Database      bool              `json:"database"`
	}
	rows := []row{}
	for _, candidate := range candidates {
		for _, mode := range []string{"human", "json", "ndjson"} {
			current := row{Args: []string{"--type=novel"}, Bodies: candidate.Source.Bodies, Queries: []url.Values{}}
			if candidate.Source.Mode != "" {
				current.Args = append(current.Args, "--mode="+candidate.Source.Mode)
			}
			if candidate.Source.Date != "" {
				current.Args = append(current.Args, "--date="+candidate.Source.Date)
			}
			current.Args = append(current.Args, candidate.Extra...)
			if mode == "json" {
				current.Args = append(current.Args, "--json")
			}
			if mode == "ndjson" {
				current.Args = append(current.Args, "--ndjson")
			}
			var output, diagnostics bytes.Buffer
			command := rankingcmd.New(deps.Data{Input: migrationSearchFailedRead{}, Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
					if req.Method != "GET" || req.URL.Path != "/v1/novel/ranking" {
						t.Fatal("unexpected ranking CLI route")
					}
					current.Queries = append(current.Queries, req.URL.Query())
					body := current.Bodies[0]
					if len(current.Bodies) > 1 && req.URL.Query().Get("offset") != "" {
						body = current.Bodies[1]
					}
					return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
				})}})
				if err != nil {
					return err
				}
				_, err = invoke(ctx, client)
				return err
			}})
			root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
			root.AddCommand(command)
			root.SetOut(&output)
			root.SetErr(&diagnostics)
			root.SetArgs(append([]string{"ranking"}, current.Args...))
			err := root.Execute()
			if err != nil {
				current.Error = err.Error()
			}
			current.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
			current.Stdout, current.Stderr = output.String(), diagnostics.String()
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("HTTPS_PROXY", "")
			t.Setenv("REQUEST_INTERVAL", "0")
			output.Reset()
			diagnostics.Reset()
			current.StartupExit = Run(append([]string{"pixiv", "ranking"}, current.Args...), migrationSearchFailedRead{}, &output, &diagnostics)
			current.StartupStderr = diagnostics.String()
			exists := func(name string) bool {
				_, err := os.Stat(filepath.Join(home, ".pixiv-cli", name))
				if err != nil && !os.IsNotExist(err) {
					t.Fatal(err)
				}
				return err == nil
			}
			current.Config, current.Database = exists("config.toml"), exists("pixiv-cli.db")
			rows = append(rows, current)
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "cli-novel-ranking.json")
	if *updateCLINovelRanking {
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
		t.Fatal("ranking CLI differs from Go reference")
	}
}
