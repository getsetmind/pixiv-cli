package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	recommendedcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/recommended"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

var updateCLIRecommended = flag.Bool("migration-update-cli-recommended", false, "capture recommended CLI output")

func TestMigrationRecommendedPreservesCommandOutputsAndWindows(t *testing.T) {
	type row struct {
		Args    []string                     `json:"args"`
		Bodies  map[string][]json.RawMessage `json:"bodies"`
		Queries []string                     `json:"queries"`
		Error   string                       `json:"error"`
		Stdout  string                       `json:"stdout"`
		Stderr  string                       `json:"stderr"`
		Exit    int                          `json:"exit"`
	}
	visual := []json.RawMessage{json.RawMessage(`{"illusts":[{"id":101,"create_date":"2024-01-02T03:04:05+09:00","title":"first<&","type":"manga","user":{"id":9,"name":"author"}},{"id":102,"create_date":"2024-01-02T03:04:05+09:00","title":"second","type":"illust","user":{"id":9,"name":"author"}},{"id":103,"create_date":"2024-01-02T03:04:05+09:00","title":"animation","type":"ugoira","user":{"id":9,"name":"author"}}],"next_url":"https://app-api.pixiv.net/v1/illust/recommended?offset=30"}`), json.RawMessage(`{"illusts":[{"id":102,"create_date":"2024-01-02T03:04:05+09:00","title":"duplicate","type":"illust","user":{"id":9,"name":"author"}},{"id":104,"create_date":"2024-01-02T03:04:05+09:00","title":"last","type":"manga","user":{"id":9,"name":"author"}}],"next_url":null}`)}
	novels := []json.RawMessage{json.RawMessage(`{"novels":[{"id":201,"create_date":"2024-01-02T03:04:05+09:00","title":"novel","user":{"id":9,"name":"author"}}],"next_url":"https://app-api.pixiv.net/v1/novel/recommended?offset=30"}`), json.RawMessage(`{"novels":[{"id":201,"create_date":"2024-01-02T03:04:05+09:00","title":"duplicate novel","user":{"id":9,"name":"author"}}],"next_url":null}`)}
	users := []json.RawMessage{json.RawMessage(`{"user_previews":[{"user":{"id":301,"name":"user","account":"account","comment":"comment"}}],"next_url":"https://app-api.pixiv.net/v1/user/recommended?offset=30"}`), json.RawMessage(`{"user_previews":[{"user":{"id":302,"name":"last user"}}],"next_url":null}`)}
	standard := map[string][]json.RawMessage{"/v1/illust/recommended": visual, "/v1/novel/recommended": novels, "/v1/user/recommended": users}
	variants := [][]string{{"illust"}, {"manga"}, {"novel"}, {"user"}, {"all"}, {"artwork"}, {"invalid"}, {}, {"--type=artwork"}, {"--type=novel"}, {"--type=user"}, {"--type=all"}, {"--type=illust"}, {"illust", "--type=artwork"}, {"--type=artwork", "--content-type=illust"}, {"--type=artwork", "--content-type=manga"}, {"--type=artwork", "--content-type=invalid"}, {"novel", "--content-type=all"}, {"artwork", "--content-type=illust"}, {"all", "extra"}, {"illust", "--limit=-1"}, {"illust", "--page=0"}, {"illust", "--page=2"}, {"illust", "--limit=0", "--page=2"}, {"illust", "--limit=3", "--page=4000000000000000000"}, {"illust", "--proxy=http://fixture", "--no-proxy"}, {"illust", "--ndjson", "--json=false"}}
	variants = append(variants, []string{"--type=artwork", "--content-type=illust", "--limit=1"}, []string{"--type=artwork", "--content-type=illust", "--limit=2", "--page=2"}, []string{"--type=artwork", "--content-type=manga", "--limit=1"})
	for _, kind := range []string{"illust", "manga", "novel", "user", "all"} {
		for _, extra := range [][]string{{"--limit=0"}, {"--limit=1"}, {"--limit=2"}, {"--limit=4"}, {"--limit=2", "--page=2"}, {"--limit=1", "--page=9"}} {
			variants = append(variants, append([]string{kind}, extra...))
		}
	}
	rows := []row{}
	for scenario := 0; scenario < 6; scenario++ {
		candidates := variants
		bodies := standard
		if scenario > 0 {
			candidates = [][]string{{"illust"}, {"all"}, {"all", "--limit=0"}}
			bodies = map[string][]json.RawMessage{}
			for key, value := range standard {
				bodies[key] = append([]json.RawMessage{}, value...)
			}
			switch scenario {
			case 1:
				bodies["/v1/illust/recommended"] = []json.RawMessage{json.RawMessage(`{"illusts":[],"next_url":"https://app-api.pixiv.net/v1/illust/recommended?offset=30"}`), visual[1]}
			case 2:
				bodies["/v1/illust/recommended"] = []json.RawMessage{visual[0], json.RawMessage(`{"illusts":null}`)}
			case 3:
				bodies["/v1/novel/recommended"] = []json.RawMessage{json.RawMessage(`{"novels":null}`)}
			case 4:
				bodies["/v1/user/recommended"] = []json.RawMessage{json.RawMessage(`{"user_previews":[]}`)}
			case 5:
				candidates = [][]string{{"illust"}, {"manga"}, {"all"}, {"--type=artwork", "--content-type=illust"}, {"--type=artwork", "--content-type=manga"}}
				bodies["/v1/illust/recommended"] = []json.RawMessage{json.RawMessage(`{"illusts":[{"id":501,"type":"","create_date":"2024-01-02T03:04:05+09:00"},{"id":502,"type":"illustration","create_date":"2024-01-02T03:04:05+09:00"},{"id":503,"type":" MANGA ","create_date":"2024-01-02T03:04:05+09:00"},{"id":504,"type":"ILLUST","create_date":"2024-01-02T03:04:05+09:00"},{"id":505,"type":"illust","create_date":"2024-01-02T03:04:05+09:00"},{"id":506,"type":"manga","create_date":"2024-01-02T03:04:05+09:00"}],"next_url":null}`)}

			}
		}
		for _, args := range candidates {
			for _, mode := range []string{"human", "json", "ndjson"} {
				current := row{Args: append([]string{}, args...), Bodies: bodies, Queries: []string{}}
				if mode == "json" {
					current.Args = append(current.Args, "--json")
				}
				if mode == "ndjson" {
					current.Args = append(current.Args, "--ndjson")
				}
				var output, diagnostics bytes.Buffer
				cmd := recommendedcmd.New(recommendedcmd.Dependencies{Input: bytes.NewReader(nil), Output: &output, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return mode == "json", nil }, Pooled: func(ctx context.Context, _ recommendedcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
						if req.Method != "GET" || req.Header.Get("Authorization") != "Bearer fixture-access" {
							t.Fatal("unexpected recommended request")
						}
						query := req.URL.Query()
						current.Queries = append(current.Queries, req.URL.Path+"?"+query.Encode())
						pages, ok := bodies[req.URL.Path]
						if !ok {
							t.Fatal(req.URL.Path)
						}
						index := 0
						if len(pages) > 1 && query.Get("offset") != "" {
							index = 1
						}
						return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(pages[index])), Request: req}, nil
					})}})
					if err != nil {
						return err
					}
					_, err = invoke(ctx, client)
					return err
				}})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(cmd)
				root.SetOut(&output)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"recommended"}, current.Args...))
				err := root.Execute()
				if err != nil {
					current.Error = err.Error()
				}
				current.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
				if scenario == 0 && len(args) > 0 && (args[0] == "--type=artwork" || args[0] == "--type=novel" || args[0] == "--type=user" || args[0] == "--type=all") && err == nil {
					if output.Len() == 0 && !(mode == "ndjson" && len(args) > 2) {
						t.Fatal("successful type flag path did not produce output")
					}
					if len(current.Queries) == 0 {
						t.Fatal("successful type flag path did not fetch")
					}
				}
				current.Stdout = output.String()
				current.Stderr = diagnostics.String()
				rows = append(rows, current)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli-recommended.json")
	if *updateCLIRecommended {
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
		t.Fatal("recommended CLI differs from Go reference")
	}
}

func TestMigrationRecommendedInvalidUTF8StdinPreservesKindValidation(t *testing.T) {
	for _, args := range [][]string{{}, {"--type=novel"}} {
		for _, input := range [][]byte{{0xff}, {'i', 'l', 'l', 'u', 's', 't', 0xff, '\n'}} {
			var output bytes.Buffer
			command := recommendedcmd.New(recommendedcmd.Dependencies{Input: bytes.NewReader(input), Output: &output, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return false, nil }, Pooled: func(context.Context, recommendedcmd.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
				t.Fatal("invalid input reached pool")
				return nil
			}})
			root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
			root.AddCommand(command)
			root.SetArgs(append([]string{"recommended"}, args...))
			err := root.Execute()
			want := "recommendation kind must be one of: all, illust, manga, novel, user"
			if len(args) > 0 {
				want = "KIND cannot be combined with --type"
			}
			if err == nil || err.Error() != want {
				t.Fatalf("invalid UTF-8 input: got %v, want %s", err, want)
			}
			if output.Len() != 0 {
				t.Fatal("invalid input emitted output")
			}
		}
	}
}
