package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	seriescmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/series"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateSeriesStdin = flag.Bool("migration-update-series-stdin", false, "capture search stdin resolution and startup")

func TestMigrationNovelSeriesStdinPreservesTextAndExplicitArguments(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Args     []string `json:"args"`
		Input    string   `json:"input"`
		Word     string   `json:"word"`
		Bytes    int      `json:"bytes"`
		Error    string   `json:"error"`
		Stderr   string   `json:"stderr"`
		Exit     int      `json:"exit"`
		Config   bool     `json:"config"`
		Database bool     `json:"database"`
	}
	var rows []row
	for _, input := range []string{"", "\n", "\r\n", "6001\n", "6001\r\n", " 6001 \r\n", "6001\n\n", "6001\r", "6001\n6002\n", "invalid\n", "https://www.pixiv.net/novel/series/6001\n"} {
		for _, words := range [][]string{{}, {"6001"}, {"6001", "6002"}} {
			for _, mode := range []string{"", "--json=false", "--ndjson"} {
				current := row{Args: append([]string{"--type=novel"}, words...), Input: input}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				reader := &migrationSearchReader{input: strings.NewReader(input)}
				var command *cobra.Command
				var output, diagnostics bytes.Buffer
				command = seriescmd.New(deps.Data{
					Input: reader, Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError,
					JSONOut: func(*bool) (bool, error) { return false, nil },
					Pooled: func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
						current.Word = strings.Join(pipeline.ResolvedArgs(command, nil), " ")
						return nil
					},
				})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(&output)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"series"}, current.Args...))
				if err := root.Execute(); err != nil {
					current.Error = err.Error()
				}
				current.Bytes = reader.bytes
				pipeline.Clear(root)
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				diagnostics.Reset()
				output.Reset()
				current.Exit = Run(append([]string{"pixiv", "series"}, current.Args...), strings.NewReader(input), &output, &diagnostics)
				current.Stderr = diagnostics.String()
				directory := filepath.Join(home, ".pixiv-cli")
				exists := func(path string) bool {
					_, err := os.Stat(path)
					if err != nil && !os.IsNotExist(err) {
						t.Fatal(err)
					}
					return err == nil
				}
				current.Config = exists(filepath.Join(directory, "config.toml"))
				current.Database = exists(filepath.Join(directory, "pixiv-cli.db"))
				if output.Len() != 0 {
					t.Fatal("startup error wrote stdout")
				}
				rows = append(rows, current)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "cli-novel-series-stdin.json")
	if *updateSeriesStdin {
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
		t.Fatal("series stdin differs from Go reference")
	}
}
