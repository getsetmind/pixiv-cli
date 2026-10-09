package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCLIUserSearch = flag.Bool("migration-update-cli-user-search", false, "capture user search CLI output and startup")

func TestMigrationUserSearchPreservesLogicalPagesOutputsAndStartup(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "user-search.json"))
	if err != nil {
		t.Fatal(err)
	}
	type source struct {
		Name       string            `json:"name"`
		Word       string            `json:"word"`
		Bodies     []json.RawMessage `json:"bodies"`
		WireBodies []string          `json:"wire_bodies,omitempty"`
	}
	var sources []source
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type candidate struct {
		Source source
		Extra  []string
		Writer string
	}
	var candidates []candidate
	for _, source := range sources {
		if strings.HasPrefix(source.Name, "cursor:") || strings.HasPrefix(source.Name, "binding:") {
			continue
		}
		candidates = append(candidates, candidate{Source: source})
	}
	wireData, err := os.ReadFile(filepath.Join(path, "user-wire.json"))
	if err != nil {
		t.Fatal(err)
	}
	var wireInputs []struct {
		Name      string `json:"name"`
		Operation string `json:"operation"`
		Body      string `json:"body"`
	}
	if err := json.Unmarshal(wireData, &wireInputs); err != nil {
		t.Fatal(err)
	}
	selectedWire := map[string]bool{
		"uppercase_known_user_fields":             true,
		"unicode_long_s_matches_known_fields":     true,
		"ordinary_nested_image_objects_merge":     true,
		"later_image_pointer_null_clears_value":   true,
		"later_scalar_null_preserves_value":       true,
		"invalid_scalar_then_valid_scalar":        true,
		"unknown_complex_fields_ignored":          true,
		"duplicate_user_objects_merge_or_replace": true,
		"long_s_envelope":                         true,
		"duplicate_list_null_invalidates":         true,
	}
	for _, input := range wireInputs {
		if input.Operation == "SearchUsers" && selectedWire[input.Name] {
			candidates = append(candidates, candidate{Source: source{Name: "wire:" + input.Name, Word: "artist", Bodies: []json.RawMessage{}, WireBodies: []string{input.Body}}})
		}
	}
	for _, extra := range [][]string{{"--limit=0"}, {"--limit=1"}, {"--limit=2", "--page=2"}, {"--limit=-1"}, {"--page=0"}, {"--page=1"}, {"--limit=0", "--page=2"}, {"--limit=3", "--page=4000000000000000000"}, {"--type=invalid"}, {"extra"}} {
		candidates = append(candidates, candidate{Source: sources[0], Extra: extra})
	}
	for _, extra := range [][]string{{"--period="}, {"--period=year"}, {"--sort=date_desc"}, {"--search-by=tag-partial"}, {"--search-by=invalid"}, {"--rating=", "--content-type=all"}, {"more", "words"}} {
		candidates = append(candidates, candidate{Source: sources[0], Extra: extra})
	}
	for _, flag := range []string{"content-type=all", "resolution=all", "aspect-ratio=all", "draw-tool=", "ai-mode=all", "bookmark-min=0", "bookmark-max=0", "bookmark-strategy=auto", "start-date=", "end-date=", "rating="} {
		candidates = append(candidates, candidate{Source: sources[0], Extra: []string{"--" + flag}})
	}
	type row struct {
		Writer        string            `json:"writer"`
		Args          []string          `json:"args"`
		Bodies        []json.RawMessage `json:"bodies"`
		WireBodies    []string          `json:"wire_bodies,omitempty"`
		Queries       []url.Values      `json:"queries"`
		Error         string            `json:"error"`
		Stdout        string            `json:"stdout"`
		Stderr        string            `json:"stderr"`
		Exit          int               `json:"exit"`
		StartupStderr string            `json:"startup_stderr"`
		StartupInput  string            `json:"startup_input"`
		StartupArgs   []string          `json:"startup_args"`
		StartupExit   int               `json:"startup_exit"`
		Config        bool              `json:"config"`
		Database      bool              `json:"database"`
	}
	pageBodies := []json.RawMessage{
		json.RawMessage(`{"user_previews":[{"user":{"id":101,"name":"first"}},{"user":{"id":102,"name":"second"}}],"next_url":"https://app-api.pixiv.net/v1/search/user?offset=30"}`),
		json.RawMessage(`{"user_previews":[{"user":{"id":102,"name":"duplicate"}},{"user":{"id":103,"name":"last"}}],"next_url":null}`),
	}
	for _, extra := range [][]string{{"--limit=0"}, {"--limit=1"}, {"--limit=2"}, {"--limit=3"}, {"--limit=2", "--page=2"}, {"--limit=1", "--page=4"}, {"--limit=1", "--page=5"}} {
		candidates = append(candidates, candidate{Source: source{Word: "pages", Bodies: pageBodies}, Extra: extra})
	}
	for _, bodies := range [][]json.RawMessage{
		{json.RawMessage(`{"user_previews":[],"next_url":"https://app-api.pixiv.net/v1/search/user?offset=30"}`), pageBodies[1]},
		{pageBodies[0], json.RawMessage(`{"user_previews":null}`)},
		{pageBodies[0], pageBodies[0]},
	} {
		candidates = append(candidates, candidate{Source: source{Word: "pages", Bodies: bodies}, Extra: []string{"--limit=0"}})
	}
	for _, writer := range []string{"other", "broken", "short"} {
		candidates = append(candidates, candidate{Source: source{Word: "writers", Bodies: pageBodies}, Extra: []string{"--limit=0"}, Writer: writer})
	}
	rows := []row{}
	for _, candidate := range candidates {
		for _, mode := range []string{"human", "json", "ndjson"} {
			current := row{Writer: candidate.Writer, Args: []string{"--type=user"}, Bodies: candidate.Source.Bodies, WireBodies: candidate.Source.WireBodies, Queries: []url.Values{}}
			current.Args = append(current.Args, candidate.Extra...)
			current.Args = append(current.Args, candidate.Source.Word)
			if mode == "json" {
				current.Args = append(current.Args, "--json")
			}
			if mode == "ndjson" {
				current.Args = append(current.Args, "--ndjson")
			}
			var output, diagnostics bytes.Buffer
			writer := io.Writer(&output)
			if candidate.Writer != "" {
				writer = migrationUserSearchWriter{&output, candidate.Writer}
			}
			command := searchcmd.New(searchcmd.Dependencies{Input: migrationSearchFailedRead{}, Output: writer, ErrorOutput: &diagnostics, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
					if req.Method != "GET" || req.URL.Path != "/v1/search/user" || req.Header.Get("Authorization") != "Bearer fixture-access" {
						t.Fatal("unexpected user search CLI route")
					}
					current.Queries = append(current.Queries, req.URL.Query())
					var body []byte
					if len(current.WireBodies) != 0 {
						body = []byte(current.WireBodies[0])
					} else {
						body = current.Bodies[0]
						if len(current.Bodies) > 1 && req.URL.Query().Get("offset") != "" {
							body = current.Bodies[1]
						}
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
			current.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
			current.Stdout, current.Stderr = output.String(), diagnostics.String()
			if len(current.WireBodies) != 0 {
				current.StartupArgs = []string{}
				rows = append(rows, current)
				continue
			}
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("HTTPS_PROXY", "")
			t.Setenv("REQUEST_INTERVAL", "0")
			output.Reset()
			diagnostics.Reset()
			current.StartupArgs = append([]string{}, current.Args...)
			var startupInput io.Reader = migrationSearchFailedRead{}
			for index, arg := range current.StartupArgs {
				if strings.ContainsRune(arg, 0) {
					current.StartupInput = arg
					current.StartupArgs = append(current.StartupArgs[:index], current.StartupArgs[index+1:]...)
					startupInput = strings.NewReader(arg)
					break
				}
			}
			current.StartupExit = Run(append([]string{"pixiv", "search"}, current.StartupArgs...), startupInput, &output, &diagnostics)
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
	target := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli-user-search.json")
	if *updateCLIUserSearch {
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			t.Fatal(err)
		}
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
		t.Fatal("user search CLI differs from Go reference")
	}
}

type migrationUserSearchWriter struct {
	output   *bytes.Buffer
	scenario string
}

func (w migrationUserSearchWriter) Write(bytes []byte) (int, error) {
	switch w.scenario {
	case "broken":
		return 0, syscall.EPIPE
	case "short":
		count := min(len(bytes), max(0, 10-w.output.Len()))
		w.output.Write(bytes[:count])
		return count, io.ErrShortWrite
	default:
		return 0, fmt.Errorf("fixture write failed")
	}
}
