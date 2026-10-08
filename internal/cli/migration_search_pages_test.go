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
	"strconv"
	"strings"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var migrationUpdateSearchPages = flag.Bool("migration-update-search-pages", false, "capture CLI logical search pages and local rating behavior")

func TestMigrationSearchRatingAndPagesMatchFrozenCollectionAndFailures(t *testing.T) {
	type row struct {
		Flags   map[string]string `json:"flags"`
		Bodies  []json.RawMessage `json:"bodies"`
		Args    []string          `json:"args"`
		Queries []url.Values      `json:"queries"`
		Calls   int               `json:"calls"`
		Error   string            `json:"error"`
		Stdout  string            `json:"stdout"`
		Stderr  string            `json:"stderr"`
		Exit    int               `json:"exit"`
	}
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var details []struct {
		Body struct {
			Illust json.RawMessage `json:"illust"`
		} `json:"body"`
	}
	if err := json.Unmarshal(data, &details); err != nil {
		t.Fatal(err)
	}
	item := func(id int, kind string, restrict int) json.RawMessage {
		var value map[string]any
		if err := json.Unmarshal(details[0].Body.Illust, &value); err != nil {
			t.Fatal(err)
		}
		value["id"], value["type"], value["x_restrict"] = id, kind, restrict
		body, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		return body
	}
	page := func(items []json.RawMessage, next int) json.RawMessage {
		value := map[string]any{"illusts": items}
		if next > 0 {
			value["next_url"] = "https://app-api.pixiv.net/v1/search/illust?offset=" + strconv.Itoa(next)
		}
		body, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		return body
	}
	bodies := []json.RawMessage{page([]json.RawMessage{item(1, "illust", 0), item(2, "manga", 1), item(3, "ugoira", 2), item(2, "manga", 1)}, 30), page([]json.RawMessage{item(4, "illust", 3), item(5, "illust", 1), item(6, "manga", 2), item(5, "illust", 1)}, 60), page([]json.RawMessage{item(7, "ugoira", 0)}, 0)}
	rows := []row{}
	for _, rating := range []string{"", "all", "sfw", "r18", "r18g", "mature"} {
		for _, content := range []string{"all", "illust", "illust-and-ugoira"} {
			for _, plan := range []map[string]string{{}, {"limit": "0"}, {"limit": "2"}, {"limit": "2", "page": "2"}} {
				flags := map[string]string{"rating": rating, "content-type": content}
				for k, v := range plan {
					flags[k] = v
				}
				rows = append(rows, row{Flags: flags, Bodies: bodies})
			}
		}
	}
	for _, rating := range []string{"all", "sfw", "r18", "mature"} {
		empty := append([]json.RawMessage{}, bodies...)
		empty[0] = page([]json.RawMessage{}, 30)
		rows = append(rows, row{Flags: map[string]string{"rating": rating}, Bodies: empty})
		broken := append([]json.RawMessage{}, bodies...)
		broken[1] = json.RawMessage(`{"illusts":[null]}`)
		rows = append(rows, row{Flags: map[string]string{"rating": rating, "limit": "0"}, Bodies: broken})
	}
	for _, flags := range []map[string]string{{"rating": "bad"}, {"rating": "bad", "content-type": "bad"}, {"rating": "bad", "search-by": "bad"}, {"limit": "-1"}, {"page": "0"}, {"page": "-1"}, {"page": "2"}, {"page": "2", "limit": "0"}, {"page": "3", "limit": "9223372036854775807"}, {"rating": "bad", "period": "bad", "limit": "-1"}} {
		rows = append(rows, row{Flags: flags, Bodies: bodies})
	}
	for index := range rows {
		current := &rows[index]
		current.Queries = []url.Values{}
		var out, diagnostics bytes.Buffer
		command := searchcmd.New(searchcmd.Dependencies{Input: strings.NewReader(""), Output: &out, ErrorOutput: &diagnostics, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			current.Calls++
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(request *http.Request) (*http.Response, error) {
				if request.Method != "GET" || request.URL.Path != "/v1/search/illust" {
					t.Fatalf("unexpected request: %s %s", request.Method, request.URL.Path)
				}
				current.Queries = append(current.Queries, request.URL.Query())
				position := 0
				switch request.URL.Query().Get("offset") {
				case "30":
					position = 1
				case "60":
					position = 2
				}
				return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(current.Bodies[position]))}, nil
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
		current.Args = []string{"search", "cat", "--json"}
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
		current.Exit = (app{out: &out, errOut: &diagnostics}).exitWithNDJSONScope(err, false, true)
		current.Stdout, current.Stderr = out.String(), diagnostics.String()
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "search-pages.json")
	if *migrationUpdateSearchPages {
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
		t.Fatal("CLI search collection differs from fixed Go reference")
	}
}
