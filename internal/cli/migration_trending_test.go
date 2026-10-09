package cli

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

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateTrending = flag.Bool("migration-update-trending", false, "capture trending tag data and CLI constraints")

func TestMigrationTrendingTagsPreservesSamplesErrorsAndFlagRestrictions(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	source, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Bodies []struct {
			Illusts []json.RawMessage `json:"illusts"`
		} `json:"bodies"`
	}
	if err := json.Unmarshal(source, &sources); err != nil {
		t.Fatal(err)
	}
	valid, _ := json.Marshal(map[string]any{"trend_tags": []any{map[string]any{"tag": "cat\n<tag>", "translated_name": "猫\t訳", "illust": sources[0].Bodies[0].Illusts[0]}, map[string]any{"tag": "cat", "translated_name": "", "illust": sources[0].Bodies[0].Illusts[1]}}})
	bodies := []json.RawMessage{valid, []byte(`{"trend_tags":[]}`), []byte(`{}`), []byte(`{"trend_tags":null}`), []byte(`{"trend_tags":{}}`), []byte(`{"trend_tags":[null]}`), []byte(`{"trend_tags":[{"tag":"cat"}]}`), []byte(`{"trend_tags":[{"tag":"cat","illust":null}]}`), []byte(`{"trend_tags":[{"tag":"cat","illust":{}}]}`)}
	for _, mutation := range []string{"empty_tag", "numeric_tag", "null_translation", "numeric_translation", "bad_title", "bad_date", "ignored_user_fields"} {
		var body map[string]any
		if err := json.Unmarshal(valid, &body); err != nil {
			t.Fatal(err)
		}
		tag := body["trend_tags"].([]any)[0].(map[string]any)
		art := tag["illust"].(map[string]any)
		switch mutation {
		case "empty_tag":
			tag["tag"] = ""
		case "numeric_tag":
			tag["tag"] = 1
		case "null_translation":
			tag["translated_name"] = nil
		case "numeric_translation":
			tag["translated_name"] = 1
		case "bad_title":
			art["title"] = 1
		case "bad_date":
			art["create_date"] = "invalid"
		case "ignored_user_fields":
			user := art["user"].(map[string]any)
			user["comment"] = 1
			user["is_followed"] = "invalid"
		}
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		bodies = append(bodies, data)
	}
	type row struct {
		Body          json.RawMessage `json:"body"`
		Args          []string        `json:"args"`
		Requests      int             `json:"requests"`
		Error         string          `json:"error"`
		Stdout        string          `json:"stdout"`
		Stderr        string          `json:"stderr"`
		Exit          int             `json:"exit"`
		StartupStderr string          `json:"startup_stderr"`
		StartupExit   int             `json:"startup_exit"`
		Config        bool            `json:"config"`
		Database      bool            `json:"database"`
	}
	var rows []row
	inputs := [][]string{{}}
	for _, flag := range []string{"--type=artwork", "--content-type=all", "--search-by=tag-partial", "--sort=date_desc", "--period=day", "--start-date=2024-01-01", "--end-date=2024-01-01", "--rating=all", "--resolution=all", "--aspect-ratio=all", "--draw-tool=pen", "--ai-mode=all", "--bookmark-min=0", "--bookmark-max=0", "--bookmark-strategy=auto", "--limit=0", "--page=1", "--ndjson=false"} {
		inputs = append(inputs, []string{flag})
	}
	inputs = append(inputs, []string{"cat"})
	for index, body := range bodies {
		selected := inputs
		if index != 0 {
			selected = inputs[:1]
		}
		for _, extra := range selected {
			for _, machine := range []bool{false, true} {
				current := row{Body: body, Args: append([]string{"--trending-tags"}, extra...)}
				if machine {
					current.Args = append(current.Args, "--json")
				}
				var output, diagnostics bytes.Buffer
				command := searchcmd.New(searchcmd.Dependencies{Input: migrationSearchFailedRead{}, Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return machine, nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
						current.Requests++
						if req.Method != "GET" || req.URL.Path != "/v1/trending-tags/illust" || len(req.URL.Query()) != 0 {
							t.Fatal("trending request did not use the complete empty-query endpoint")
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
				root.SetArgs(append([]string{"search"}, current.Args...))
				err := root.Execute()
				if err != nil {
					current.Error = err.Error()
				}
				current.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, false, machine)
				current.Stdout, current.Stderr = output.String(), diagnostics.String()
				pipeline.Clear(root)
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				output.Reset()
				diagnostics.Reset()
				current.StartupExit = Run(append([]string{"pixiv", "search"}, current.Args...), migrationSearchFailedRead{}, &output, &diagnostics)
				current.StartupStderr = diagnostics.String()
				exists := func(path string) bool {
					_, err := os.Stat(path)
					if err != nil && !os.IsNotExist(err) {
						t.Fatal(err)
					}
					return err == nil
				}
				directory := filepath.Join(home, ".pixiv-cli")
				current.Config = exists(filepath.Join(directory, "config.toml"))
				current.Database = exists(filepath.Join(directory, "pixiv-cli.db"))
				rows = append(rows, current)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "trending-tags.json")
	if *updateTrending {
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
		t.Fatal("trending tags differ from Go reference")
	}
}
