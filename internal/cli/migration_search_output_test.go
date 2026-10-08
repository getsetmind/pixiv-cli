package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"testing"

	searchcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/search"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var migrationUpdateSearchOutput = flag.Bool("migration-update-search-output", false, "capture search output and streaming failures")

type migrationSearchOutputWriter struct {
	out     bytes.Buffer
	failure string
}

func (w *migrationSearchOutputWriter) Write(p []byte) (int, error) {
	if w.failure == "short" {
		n := min(len(p), max(0, 10-w.out.Len()))
		w.out.Write(p[:n])
		return n, io.ErrShortWrite
	}
	if w.failure == "broken" {
		return 0, syscall.EPIPE
	}
	if w.failure == "other" {
		return 0, errors.New("fixture write failed")
	}
	return w.out.Write(p)
}

func TestMigrationSearchOutputPreservesStreamingAndWriterFailures(t *testing.T) {
	temporary := t.TempDir()
	t.Setenv("TEMP", temporary)
	t.Setenv("TMP", temporary)
	t.Setenv("TMPDIR", temporary)
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Flags  map[string]string `json:"flags"`
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Source  int          `json:"source"`
		Mode    string       `json:"mode"`
		Failure string       `json:"failure"`
		Queries []url.Values `json:"queries"`
		Stdout  string       `json:"stdout"`
		Stderr  string       `json:"stderr"`
		Error   string       `json:"error"`
		Exit    int          `json:"exit"`
	}
	rows := []row{}
	for _, index := range []int{0, 1, 3, 24, 25, 27, 40, 72, 73, 75, 77, 79} {
		source := sources[index]
		for _, mode := range []string{"human", "ndjson", "json"} {
			failures := []string{"", "other", "broken"}
			if mode == "json" {
				failures = append(failures, "short")
			}
			for _, failure := range failures {
				current := row{Source: index, Mode: mode, Failure: failure, Queries: []url.Values{}}
				out := &migrationSearchOutputWriter{failure: failure}
				var diagnostics bytes.Buffer
				command := searchcmd.New(searchcmd.Dependencies{Input: strings.NewReader(""), Output: out, ErrorOutput: &diagnostics, JSONOut: func(_ *bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ searchcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(request *http.Request) (*http.Response, error) {
						if mode == "json" {
							files, err := os.ReadDir(temporary)
							if err != nil {
								t.Fatal(err)
							}
							local := source.Flags["rating"] != "" && source.Flags["rating"] != "all"
							if !local {
								if len(files) != 1 {
									t.Fatalf("expected spool before fetch, got %d files", len(files))
								}
								body, err := os.ReadFile(filepath.Join(temporary, files[0].Name()))
								if err != nil {
									t.Fatal(err)
								}
								if !bytes.HasPrefix(body, []byte("{\n  \"illusts\": [")) {
									t.Fatal("spool envelope missing")
								}
							}
						}
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
						return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(source.Bodies[position]))}, nil
					})}})
					if err != nil {
						return err
					}
					_, err = invoke(ctx, client)
					return err
				}})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(out)
				root.SetErr(&diagnostics)
				args := []string{"search", "cat"}
				if mode == "ndjson" {
					args = append(args, "--ndjson")
				}
				if mode == "json" {
					args = append(args, "--json")
				}
				keys := []string{}
				for key := range source.Flags {
					keys = append(keys, key)
				}
				sort.Strings(keys)
				for _, key := range keys {
					args = append(args, "--"+key, source.Flags[key])
				}
				root.SetArgs(args)
				err = root.Execute()
				files, cleanupErr := os.ReadDir(temporary)
				if cleanupErr != nil {
					t.Fatal(cleanupErr)
				}
				if len(files) != 0 {
					t.Fatal("search left temporary files behind")
				}
				if err != nil {
					current.Error = err.Error()
				}
				current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
				current.Stdout, current.Stderr = out.out.String(), diagnostics.String()
				rows = append(rows, current)
			}
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "search-output.json")
	if *migrationUpdateSearchOutput {
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
		t.Fatal("search output differs from frozen Go reference")
	}
}
