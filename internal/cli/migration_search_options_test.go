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
	"sort"
	"strings"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var migrationUpdateSearchOptions = flag.Bool("migration-update-search-options", false, "capture frozen CLI artwork search selectors and validation order")

func TestMigrationSearchSelectorsMatchFrozenQueriesAndValidationOrder(t *testing.T) {
	type input struct {
		Name  string
		Flags map[string]string
	}
	inputs := []input{{"defaults", map[string]string{}}}
	for _, group := range []struct {
		Flag   string
		Values []string
	}{
		{"search-by", []string{"tag-partial", "tag-exact", "title-caption", "tag-title-caption", "", "bad", "TAG-PARTIAL"}},
		{"content-type", []string{"", "all", "illust-and-ugoira", "illust", "illustration", "manga", "ugoira", " ILLUST ", "ILLUSTRATION", "İLLUST", "\u3000MANGA\u00a0", "bad", "illust_and_ugoira", "-bad"}},
		{"ai-mode", []string{"all", "exclude", "only", "", "bad", "EXCLUDE", " only ", "-bad"}},
		{"aspect-ratio", []string{"all", "landscape", "portrait", "square", "", "bad", "SQUARE", " square ", "-bad"}},
		{"resolution", []string{"all", "high", "medium", "low", "", "bad", "HIGH", " high ", "-bad"}},
		{"sort", []string{"", "date_desc", "date_asc", "popular_desc", "bad", "DATE_DESC", " date_desc ", "-bad"}},
		{"draw-tool", []string{"", "CLIP STUDIO PAINT", "日本語 &? ~*", "unknown tool"}},
	} {
		for _, value := range group.Values {
			inputs = append(inputs, input{group.Flag + ":" + value, map[string]string{group.Flag: value}})
		}
	}
	for index, flags := range []map[string]string{
		{"search-by": "tag-title-caption", "content-type": "illustration", "ai-mode": "only", "aspect-ratio": "portrait", "resolution": "medium", "sort": "popular_desc", "draw-tool": "arbitrary tool", "start-date": "2024-02-29", "end-date": "2024-03-01"},
		{"search-by": "bad", "content-type": "bad", "ai-mode": "bad", "aspect-ratio": "bad", "resolution": "bad", "period": "bad", "sort": "bad"},
		{"content-type": "bad", "ai-mode": "bad", "aspect-ratio": "bad", "resolution": "bad", "period": "bad", "sort": "bad"},
		{"ai-mode": "bad", "aspect-ratio": "bad", "resolution": "bad", "period": "bad", "sort": "bad"},
		{"aspect-ratio": "bad", "resolution": "bad", "period": "bad", "sort": "bad"},
		{"resolution": "bad", "period": "bad", "sort": "bad"},
		{"period": "bad", "sort": "bad"},
		{"start-date": "bad", "sort": "bad"},
		{"period": "day", "start-date": "bad", "sort": "bad"},
		{"content-type": "illust-and-ugoira", "ai-mode": "exclude", "aspect-ratio": "square", "resolution": "low", "period": "week"},
	} {
		inputs = append(inputs, input{string(rune('A' + index)), flags})
	}
	type row struct {
		Name    string            `json:"name"`
		Flags   map[string]string `json:"flags"`
		Mode    string            `json:"mode"`
		Args    []string          `json:"args"`
		Queries []url.Values      `json:"queries"`
		Calls   int               `json:"calls"`
		Error   string            `json:"error"`
		Stdout  string            `json:"stdout"`
		Stderr  string            `json:"stderr"`
		Exit    int               `json:"exit"`
	}
	rows := []row{}
	for _, in := range inputs {
		for _, mode := range []string{"human", "json", "ndjson"} {
			current := row{Name: in.Name, Flags: in.Flags, Mode: mode, Queries: []url.Values{}}
			var out, diagnostics bytes.Buffer
			command := searchcmd.New(searchcmd.Dependencies{Input: strings.NewReader(""), Output: &out, ErrorOutput: &diagnostics, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				current.Calls++
				client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(request *http.Request) (*http.Response, error) {
					if request.Method != "GET" || request.URL.Path != "/v1/search/illust" {
						t.Fatalf("unexpected request: %s %s", request.Method, request.URL.Path)
					}
					current.Queries = append(current.Queries, request.URL.Query())
					return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"illusts":[]}`))}, nil
				})}})
				if err != nil {
					return err
				}
				_, err = invoke(ctx, client)
				return err
			}})
			root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
			root.AddCommand(command)
			root.SetOut(&out)
			root.SetErr(&diagnostics)
			current.Args = []string{"search", "cat"}
			keys := []string{}
			for key := range in.Flags {
				keys = append(keys, key)
			}
			sort.Strings(keys)
			for _, key := range keys {
				current.Args = append(current.Args, "--"+key, in.Flags[key])
			}
			if mode != "human" {
				current.Args = append(current.Args, "--"+mode)
			}
			root.SetArgs(current.Args)
			err := root.Execute()
			if err != nil {
				current.Error = err.Error()
			}
			current.Exit = (app{out: &out, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
			current.Stdout = out.String()
			current.Stderr = diagnostics.String()
			rows = append(rows, current)
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "search-options.json")
	if *migrationUpdateSearchOptions {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("CLI search selectors differ from the fixed Go reference")
	}
}

var migrationUpdateSearchOptionsStartup = flag.Bool("migration-update-search-options-startup", false, "capture isolated real startup for artwork search selector errors")

func TestMigrationSearchSelectorsPreserveRealStartupScope(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "search-options.json"))
	if err != nil {
		t.Fatal(err)
	}
	type source struct {
		Name  string   `json:"name"`
		Mode  string   `json:"mode"`
		Args  []string `json:"args"`
		Error string   `json:"error"`
	}
	var sources []source
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Name     string `json:"name"`
		Mode     string `json:"mode"`
		Stdout   string `json:"stdout"`
		Stderr   string `json:"stderr"`
		Exit     int    `json:"exit"`
		Config   bool   `json:"config"`
		Database bool   `json:"database"`
	}
	rows := []row{}
	for _, in := range sources {
		if in.Error == "" {
			continue
		}
		home := t.TempDir()
		t.Setenv("HOME", home)
		t.Setenv("USERPROFILE", home)
		t.Setenv("PIXIV_ACCESS_TOKEN", "")
		t.Setenv("HTTPS_PROXY", "")
		t.Setenv("https_proxy", "")
		t.Setenv("REQUEST_INTERVAL", "0")
		var output, diagnostics bytes.Buffer
		exit := Run(append([]string{"pixiv"}, in.Args...), strings.NewReader(""), &output, &diagnostics)
		exists := func(name string) bool {
			_, err := os.Stat(filepath.Join(home, ".pixiv-cli", name))
			if err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			return err == nil
		}
		rows = append(rows, row{in.Name, in.Mode, output.String(), diagnostics.String(), exit, exists("config.toml"), exists("pixiv-cli.db")})
	}
	if len(rows) != 99 {
		t.Fatalf("selector error startup cases = %d, want 99", len(rows))
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "search-options-startup.json")
	if *migrationUpdateSearchOptionsStartup {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("search selectors startup differs from fixed Go behavior")
	}
}
