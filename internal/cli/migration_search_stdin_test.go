package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateSearchStdin = flag.Bool("migration-update-search-stdin", false, "capture search stdin resolution and startup")

type migrationSearchReader struct {
	input *strings.Reader
	bytes int
}

func (r *migrationSearchReader) Read(output []byte) (int, error) {
	n, err := r.input.Read(output)
	r.bytes += n
	return n, err
}

func TestMigrationSearchStdinPreservesTextAndExplicitArguments(t *testing.T) {
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
	for _, input := range []string{"", "\n", "\r\n", "cat\n", "cat\r\n", "  cat girl  \r\n", "cat\n\n", "cat\r", "猫\n少女\n", "{\"type\":\"illust\",\"id\":\"42\"}\n"} {
		for _, words := range [][]string{{}, {"override"}, {"-"}, {"cat", "girl"}} {
			for _, mode := range []string{"", "--json=false", "--ndjson"} {
				current := row{Args: append([]string{}, words...), Input: input}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				reader := &migrationSearchReader{input: strings.NewReader(input)}
				var command *cobra.Command
				var output, diagnostics bytes.Buffer
				command = searchcmd.New(searchcmd.Dependencies{
					Input: reader, Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError,
					JSONOut: func(*bool) (bool, error) { return false, nil },
					Pooled: func(context.Context, searchcmd.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
						current.Word = strings.Join(pipeline.ResolvedArgs(command, nil), " ")
						return nil
					},
				})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(&output)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"search"}, current.Args...))
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
				current.Exit = Run(append([]string{"pixiv", "search"}, current.Args...), strings.NewReader(input), &output, &diagnostics)
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "search-stdin.json")
	if *updateSearchStdin {
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
		t.Fatal("search stdin differs from Go reference")
	}
}

var _ io.Reader = (*migrationSearchReader)(nil)

type migrationSearchFailedRead struct{}

func (migrationSearchFailedRead) Read([]byte) (int, error) {
	return 0, errors.New("fixture read failed")
}

func TestMigrationSearchStdinReadFailureIsUsageBeforeConfiguration(t *testing.T) {
	for _, mode := range []string{"", "--json=false", "--ndjson"} {
		home := t.TempDir()
		t.Setenv("HOME", home)
		t.Setenv("USERPROFILE", home)
		var output, diagnostics bytes.Buffer
		args := []string{"pixiv", "search"}
		if mode != "" {
			args = append(args, mode)
		}
		exit := Run(args, migrationSearchFailedRead{}, &output, &diagnostics)
		if exit != 2 || output.Len() != 0 || diagnostics.String() != "error: read stdin value: fixture read failed\n" {
			t.Fatalf("%v: exit=%d stderr=%q", args, exit, diagnostics.String())
		}
		if _, err := os.Stat(filepath.Join(home, ".pixiv-cli")); !os.IsNotExist(err) {
			t.Fatalf("stdin failure created settings: %v", err)
		}
	}
}
