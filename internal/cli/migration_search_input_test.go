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
	"strings"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateSearchInput = flag.Bool("migration-update-search-input", false, "capture search words and output selection")

type migrationSearchPipe struct{ *bytes.Buffer }

func (migrationSearchPipe) Fd() uintptr { return ^uintptr(0) }

func TestMigrationSearchWordsAndOutputSelection(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	sourceData, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(sourceData, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Args           []string `json:"args"`
		ConfiguredJSON bool     `json:"configured_json"`
		Piped          bool     `json:"piped"`
		Word           string   `json:"word"`
		Stdout         string   `json:"stdout"`
		Stderr         string   `json:"stderr"`
		Exit           int      `json:"exit"`
		StartupStderr  string   `json:"startup_stderr"`
		StartupExit    int      `json:"startup_exit"`
		Database       bool     `json:"database"`
	}
	inputs := [][]string{
		{"cat", "girl"}, {"cat", "--json", "girl"}, {"cat", "girl", "--json=false"},
		{"cat", "girl", "-j"}, {"cat", "girl", "--json=true", "--json=false"},
		{"cat", "girl", "--json=false", "--json"}, {"cat", "girl", "--ndjson"},
		{"cat", "girl", "--ndjson=false"}, {"cat", "girl", "--json=false", "--ndjson"},
		{"cat", "girl", "--json", "--ndjson=false"}, {"cat", "girl", "--ndjson", "--ndjson=false"},
		{"cat", "girl", "--ndjson=false", "--ndjson"}, {"cat", "girl", "-j=false"},
		{"cat", "", "girl"}, {"cat", "--", "--json", "girl"}, {"ねこ", "少女"},
	}
	var rows []row
	for _, args := range inputs {
		for _, configured := range []bool{false, true} {
			for _, piped := range []bool{false, true} {
				current := row{Args: args, ConfiguredJSON: configured, Piped: piped}
				var output, diagnostics bytes.Buffer
				var writer io.Writer = &output
				if piped {
					writer = migrationSearchPipe{&output}
				}
				command := searchcmd.New(searchcmd.Dependencies{
					Input: strings.NewReader(""), Output: writer, ErrorOutput: &diagnostics, UsageError: newUsageError,
					JSONOut: func(override *bool) (bool, error) {
						if override != nil {
							return *override, nil
						}
						return configured, nil
					},
					Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
						client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(request *http.Request) (*http.Response, error) {
							current.Word = request.URL.Query().Get("word")
							return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(sources[0].Bodies[0]))}, nil
						})}})
						if err != nil {
							return err
						}
						_, err = invoke(ctx, client)
						return err
					},
				})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(writer)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"search"}, args...))
				err := root.Execute()
				current.Exit = (app{out: writer, errOut: &diagnostics}).exitWithNDJSONScope(err, commandWritesNDJSON(command) || commandAutoWritesNDJSON(command, writer), commandWritesNDJSON(command) || commandExplicitJSON(command))
				current.Stdout, current.Stderr = output.String(), diagnostics.String()
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				directory := filepath.Join(home, ".pixiv-cli")
				if err := os.MkdirAll(directory, 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(directory, "config.toml"), []byte(fmt.Sprintf("output_json = %t\n", configured)), 0600); err != nil {
					t.Fatal(err)
				}
				diagnostics.Reset()
				output.Reset()
				current.StartupExit = Run(append([]string{"pixiv", "search"}, args...), strings.NewReader(""), writer, &diagnostics)
				current.StartupStderr = diagnostics.String()
				_, statErr := os.Stat(filepath.Join(directory, "pixiv-cli.db"))
				if statErr != nil && !os.IsNotExist(statErr) {
					t.Fatal(statErr)
				}
				current.Database = statErr == nil
				rows = append(rows, current)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "search-input.json")
	if *updateSearchInput {
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
		t.Fatal("search words and output selection differ from Go reference")
	}
}
