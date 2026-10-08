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

var migrationUpdateSearchBookmark = flag.Bool("migration-update-search-bookmark", false, "capture CLI bookmark bounds, strategies and output")

func TestMigrationSearchBookmarkPreservesStrategiesCompletenessAndAtomicOutput(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Flags    map[string]string `json:"flags"`
		Source   int               `json:"source"`
		Negative bool              `json:"negative"`
		Mode     string            `json:"mode"`
		Args     []string          `json:"args"`
		Queries  []url.Values      `json:"queries"`
		Calls    int               `json:"calls"`
		Stdout   string            `json:"stdout"`
		Stderr   string            `json:"stderr"`
		Error    string            `json:"error"`
		Exit     int               `json:"exit"`
	}
	rows := []row{}
	for _, bounds := range []map[string]string{{"bookmark-min": "0"}, {"bookmark-min": "15", "bookmark-max": "30"}, {"bookmark-max": "0"}} {
		for _, strategy := range []string{"", "auto", "local", "best_effort", "server"} {
			for _, plan := range []map[string]string{{}, {"limit": "0"}, {"limit": "2"}, {"limit": "2", "page": "2"}} {
				for _, rating := range []string{"", "r18"} {
					for _, mode := range []string{"human", "ndjson", "json"} {
						flags := map[string]string{"rating": rating}
						for k, v := range bounds {
							flags[k] = v
						}
						for k, v := range plan {
							flags[k] = v
						}
						if strategy != "" {
							flags["bookmark-strategy"] = strategy
						}
						rows = append(rows, row{Flags: flags, Mode: mode})
					}
				}
			}
		}
	}
	for _, flags := range []map[string]string{{"bookmark-min": "-1"}, {"bookmark-max": "-1"}, {"bookmark-min": "2", "bookmark-max": "1"}, {"bookmark-strategy": "local"}, {"bookmark-strategy": "bad"}, {"bookmark-min": "0", "bookmark-strategy": "bad"}, {"bookmark-min": "-1", "limit": "-1"}, {"bookmark-min": "-1", "period": "bad"}, {"bookmark-min": "0", "bookmark-strategy": "bad", "page": "0"}} {
		for _, mode := range []string{"human", "ndjson", "json"} {
			rows = append(rows, row{Flags: flags, Mode: mode})
		}
	}
	for _, mode := range []string{"human", "ndjson", "json"} {
		rows = append(rows, row{Flags: map[string]string{"bookmark-min": "0", "limit": "0"}, Mode: mode, Negative: true}, row{Flags: map[string]string{"bookmark-min": "0", "limit": "0"}, Mode: mode, Source: 73})
	}
	for index := range rows {
		current := &rows[index]
		current.Queries = []url.Values{}
		var out, diagnostics bytes.Buffer
		bodies := []json.RawMessage{}
		for pageIndex, raw := range sources[current.Source].Bodies {
			var body map[string]any
			if err := json.Unmarshal(raw, &body); err != nil {
				t.Fatal(err)
			}
			for itemIndex, value := range body["illusts"].([]any) {
				if item, ok := value.(map[string]any); ok {
					item["total_bookmarks"] = int(item["id"].(float64)) * 10
					if current.Negative && pageIndex == 0 && itemIndex == 0 {
						item["total_bookmarks"] = -1
					}
				}
			}
			encoded, err := json.Marshal(body)
			if err != nil {
				t.Fatal(err)
			}
			bodies = append(bodies, encoded)
		}
		command := searchcmd.New(searchcmd.Dependencies{Input: strings.NewReader(""), Output: &out, ErrorOutput: &diagnostics, JSONOut: func(_ *bool) (bool, error) { return current.Mode == "json", nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			current.Calls++
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(request *http.Request) (*http.Response, error) {
				if request.Method != "GET" || request.URL.Path != "/v1/search/illust" {
					t.Fatal("unexpected request")
				}
				current.Queries = append(current.Queries, request.URL.Query())
				position := 0
				switch request.URL.Query().Get("offset") {
				case "30":
					position = 1
				case "60":
					position = 2
				}
				return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(bodies[position]))}, nil
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
		if current.Mode == "json" {
			current.Args = append(current.Args, "--json")
		}
		if current.Mode == "ndjson" {
			current.Args = append(current.Args, "--ndjson")
		}
		keys := []string{}
		for key := range current.Flags {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		for _, key := range keys {
			current.Args = append(current.Args, "--"+key, current.Flags[key])
		}
		root.SetArgs(current.Args)
		err := root.Execute()
		if err != nil {
			current.Error = err.Error()
		}
		current.Exit = (app{out: &out, errOut: &diagnostics}).exitWithNDJSONScope(err, current.Mode == "ndjson", current.Mode != "human")
		current.Stdout, current.Stderr = out.String(), diagnostics.String()
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "search-bookmark.json")
	if *migrationUpdateSearchBookmark {
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
		t.Fatal("bookmark CLI differs from frozen Go reference")
	}
}
