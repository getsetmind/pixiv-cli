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

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var migrationUpdateSearchDateOptions = flag.Bool("migration-update-search-date-options", false, "capture frozen CLI search date flags, queries and errors")

type migrationDateTransport func(*http.Request) (*http.Response, error)

func (f migrationDateTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	return f(request)
}

func TestMigrationSearchDateFlagsValidateBeforeAccountAndMatchQueries(t *testing.T) {
	type row struct {
		Period  string       `json:"period"`
		Start   string       `json:"start_date"`
		End     string       `json:"end_date"`
		Mode    string       `json:"mode"`
		Args    []string     `json:"args"`
		Queries []url.Values `json:"queries"`
		Calls   int          `json:"calls"`
		Error   string       `json:"error"`
		Stdout  string       `json:"stdout"`
		Stderr  string       `json:"stderr"`
		Exit    int          `json:"exit"`
	}
	inputs := [][3]string{{"", "", ""}, {"day", "", ""}, {"week", "", ""}, {"month", "", ""}, {"", "2024-02-29", "2024-03-01"}, {"", "2024-02-29", "2024-02-29"}, {"", "0000-02-29", "9999-12-31"}, {"", "2024-02-29", ""}, {"", "", "2024-02-29"}, {"", " \t2024-02-29\n", "\u30002024-03-01\u00a0"}, {"", " \t", "\u3000"}}
	for _, invalid := range []string{"2023-02-29", "1900-02-29", "2024-04-31", "2024-13-01", "2024-00-01", "2024-01-00", "2024-01-32", "24-01-01", "2024-1-01", "2024-01-1", "2024/01/01", "+2024-01-01", "-0001-01-01", "10000-01-01", "２０２４-01-01", "2024-01-01T00:00:00Z", "2024-01-01\x00", "\ufeff2024-01-01"} {
		inputs = append(inputs, [3]string{"", invalid, ""}, [3]string{"", "", invalid})
	}
	inputs = append(inputs, [3]string{"", "2024-03-01", "2024-02-29"}, [3]string{"", "bad", "bad"})
	for _, period := range []string{"day", "week", "month", "half-year", "year", "bad", "DAY", " day ", "within_last_day"} {
		inputs = append(inputs, [3]string{period, "2024-02-29", ""}, [3]string{period, "", "bad"})
	}
	for _, period := range []string{"bad", "DAY", " day ", "within_last_day"} {
		inputs = append(inputs, [3]string{period, "", ""})
	}
	rows := []row{}
	for _, input := range inputs {
		for _, mode := range []string{"human", "json", "ndjson"} {
			current := row{Period: input[0], Start: input[1], End: input[2], Mode: mode, Queries: []url.Values{}}
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
			current.Args = []string{"search", "cat", "--period", input[0], "--start-date", input[1], "--end-date", input[2]}
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "search-date-options.json")
	if *migrationUpdateSearchDateOptions {
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
		t.Fatal("CLI search date options differ from the fixed Go reference")
	}
}
