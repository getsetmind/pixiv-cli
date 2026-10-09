package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	detailcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/detail"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var updateCLINovelContent = flag.Bool("migration-update-cli-novel-content", false, "capture novel content CLI pre-fetch ordering")

func TestMigrationNovelContentCLIRejectsBeforeRequestAccountAndFetch(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Args          []string `json:"args"`
		Builds        int      `json:"builds"`
		Accounts      int      `json:"accounts"`
		Fetches       int      `json:"fetches"`
		Stdout        string   `json:"stdout"`
		Stderr        string   `json:"stderr"`
		Exit          int      `json:"exit"`
		StartupStderr string   `json:"startup_stderr"`
		StartupExit   int      `json:"startup_exit"`
		Config        bool     `json:"config"`
		Database      bool     `json:"database"`
	}
	rows := []row{}
	for _, input := range []string{"42", "0", "invalid", "https://www.pixiv.net/artworks/42"} {
		for _, entity := range []string{"novel", "artwork", "invalid"} {
			for _, mode := range []string{"human", "json", "ndjson"} {
				current := row{Args: []string{"--type=" + entity, "--content"}}
				if mode != "human" {
					current.Args = append(current.Args, "--"+mode)
				}
				current.Args = append(current.Args, "--", input)
				var output, diagnostics bytes.Buffer
				command := detailcmd.New(detailcmd.Dependencies{Input: strings.NewReader(""), Output: &output, ErrorOutput: &diagnostics,
					BuildRequest: func(*cobra.Command, detailcmd.Options) (detailcmd.Request, error) {
						current.Builds++
						return detailcmd.Request{}, nil
					},
					JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil },
					Pooled: func(ctx context.Context, _ detailcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
						current.Accounts++
						_, err := invoke(ctx, nil)
						return err
					},
					FetchNovelContent: func(context.Context, *pixiv.Client, int64) (pixiv.NovelContent, error) {
						current.Fetches++
						t.Fatal("content fetch must not run")
						return pixiv.NovelContent{}, nil
					},
				})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(&output)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"detail"}, current.Args...))
				err := root.Execute()
				current.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
				current.Stdout, current.Stderr = output.String(), diagnostics.String()
				if current.Builds != 0 || current.Accounts != 0 || current.Fetches != 0 {
					t.Fatal("content rejection touched execution")
				}
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				output.Reset()
				diagnostics.Reset()
				current.StartupExit = Run(append([]string{"pixiv", "detail"}, current.Args...), strings.NewReader(""), &output, &diagnostics)
				current.StartupStderr = diagnostics.String()
				exists := func(name string) bool { _, err := os.Stat(filepath.Join(home, ".pixiv-cli", name)); return err == nil }
				current.Config, current.Database = exists("config.toml"), exists("pixiv-cli.db")
				rows = append(rows, current)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "cli-novel-content.json")
	if *updateCLINovelContent {
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
		t.Fatal("novel content CLI differs from Go reference")
	}
}
