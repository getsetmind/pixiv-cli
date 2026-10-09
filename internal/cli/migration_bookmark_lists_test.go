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
	"strings"
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	bookmarkcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/bookmark"
	usercmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/user"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateBookmarkLists = flag.Bool("migration-update-bookmark-lists", false, "capture bookmark list output, input, and logical page contracts")

type bookmarkListsRow struct {
	Kind       string            `json:"kind"`
	Args       []string          `json:"args"`
	Input      string            `json:"input"`
	InputBytes []byte            `json:"input_bytes,omitempty"`
	ReadError  bool              `json:"read_error,omitempty"`
	Identity   int64             `json:"identity"`
	Mode       string            `json:"mode"`
	Writer     string            `json:"writer"`
	Bodies     []json.RawMessage `json:"bodies"`
	Paths      []string          `json:"paths"`
	Queries    []url.Values      `json:"queries"`
	Committed  []bool            `json:"committed"`
	Error      string            `json:"error"`
	Stdout     string            `json:"stdout"`
	Stderr     string            `json:"stderr"`
	Exit       int               `json:"exit"`
	Bytes      int               `json:"bytes"`
	Proxy      *string           `json:"proxy"`
	JSON       *bool             `json:"json"`
}

func bookmarkListsBodies(kind string) []json.RawMessage {
	keys, paths := []string{"illusts"}, []string{"/v1/user/bookmarks/illust"}
	if kind == "novel" {
		keys, paths = []string{"novels"}, []string{"/v1/user/bookmarks/novel"}
	}
	if kind == "all" {
		keys, paths = []string{"illusts", "novels"}, []string{"/v1/user/bookmarks/illust", "/v1/user/bookmarks/novel"}
	}
	var bodies []json.RawMessage
	for index, key := range keys {
		base := 100 + index*100
		if key == "novels" {
			base = 200
		}
		bodies = append(bodies, json.RawMessage(fmt.Sprintf(`{"%s":[{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":"first<&\nline","type":"illust","total_bookmarks":2,"total_view":3,"tags":[{"name":"tag\nline"}],"user":{"id":9,"name":"author\nline"}},{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":"second","type":"manga","user":{"id":9,"name":"author"}}],"next_url":"https://app-api.pixiv.net%s?max_bookmark_id=30"}`, key, base+1, base+2, paths[index])))
		bodies = append(bodies, json.RawMessage(fmt.Sprintf(`{"%s":[{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":"duplicate","type":"manga","user":{"id":9,"name":"author"}},{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":"last","type":"ugoira","user":{"id":9,"name":"author"}}],"next_url":null}`, key, base+2, base+3)))
	}
	return bodies
}

func bookmarkListsCommand(kind string, input io.Reader, output, diagnostics io.Writer, jsonOut func(*bool) (bool, error), invoke func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error) (*cobra.Command, []string) {
	if kind == "user" {
		return usercmd.New(usercmd.Dependencies{Input: input, Output: output, UsageError: newUsageError, JSONOut: jsonOut, Pooled: func(ctx context.Context, request usercmd.Request, attempt func(context.Context, *pixiv.Client) (bool, error)) error {
			return invoke(ctx, deps.Request(request), attempt)
		}}), []string{"user", "bookmarks"}
	}
	return bookmarkcmd.New(deps.Data{Input: input, Output: output, ErrorOutput: diagnostics, UsageError: newUsageError, JSONOut: jsonOut, Pooled: invoke}), []string{"bookmark", "list", "--type=" + kind}
}

func bookmarkListsCapture(t *testing.T, current bookmarkListsRow) bookmarkListsRow {
	t.Helper()
	current.Paths, current.Queries, current.Committed = []string{}, []url.Values{}, []bool{}
	var output, diagnostics bytes.Buffer
	out := io.Writer(&output)
	if current.Writer != "" {
		out = migrationUserSearchWriter{&output, current.Writer}
	}
	if current.Mode == "auto" {
		out = migrationBookmarkListsPipeWriter{Writer: out}
	}
	var input io.Reader = strings.NewReader(current.Input)
	if current.InputBytes != nil {
		input = bytes.NewReader(current.InputBytes)
	}
	if current.ReadError {
		input = migrationSearchFailedRead{}
	}
	reader := &migrationBookmarkListsReader{input: input}
	command, prefix := bookmarkListsCommand(current.Kind, reader, out, &diagnostics, func(value *bool) (bool, error) { current.JSON = value; return current.Mode == "json", nil }, func(ctx context.Context, request deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
		if request.UserID != 0 {
			t.Fatal("bookmark target selected authentication account")
		}
		current.Proxy = request.HTTPSProxyOverride
		transport := migrationDateTransport(func(req *http.Request) (*http.Response, error) {
			payload := []byte(fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d}}`, current.Identity))
			if req.URL.Host != "oauth.secure.pixiv.net" {
				index := 0
				if req.URL.Path == "/v1/user/bookmarks/novel" {
					if current.Kind == "all" {
						index = 2
					} else if current.Kind != "novel" {
						t.Fatalf("unexpected novel request %s", req.URL)
					}
				} else if req.URL.Path != "/v1/user/bookmarks/illust" {
					t.Fatalf("unexpected request %s", req.URL)
				}
				if req.Method != "GET" || req.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatal("unexpected bookmark method or credentials")
				}
				current.Paths = append(current.Paths, req.URL.Path)
				current.Queries = append(current.Queries, req.URL.Query())
				if req.URL.Query().Get("max_bookmark_id") != "" {
					index++
				}
				payload = current.Bodies[index]
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(payload)), Request: req}, nil
		})
		var client *pixiv.Client
		var err error
		if current.Identity > 0 {
			client, _, err = pixiv.OpenWith(ctx, "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		} else {
			client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		}
		if err != nil {
			return err
		}
		committed, err := invoke(ctx, client)
		current.Committed = append(current.Committed, committed)
		return err
	})
	root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
	root.AddCommand(command)
	root.SetOut(out)
	root.SetErr(&diagnostics)
	args := append(prefix, current.Args...)
	root.SetArgs(args)
	target := root
	if found, _, findErr := root.Find(args); findErr == nil && found != nil {
		target = found
	}
	err := root.Execute()
	if err != nil {
		current.Error = err.Error()
	}
	current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, commandWritesNDJSON(target) || commandAutoWritesNDJSON(target, out), commandWritesNDJSON(target) || commandExplicitJSON(target))
	current.Stdout, current.Stderr, current.Bytes = output.String(), diagnostics.String(), reader.bytes
	pipeline.Clear(root)
	return current
}

func TestMigrationBookmarkListsPreservesOutputsIdentityWindowsAndInput(t *testing.T) {
	var rows []bookmarkListsRow
	for _, kind := range []string{"artwork", "novel", "all", "user"} {
		bodies := bookmarkListsBodies(kind)
		variants := [][]string{{"42"}, {}, {"https://www.pixiv.net/users/42"}, {" 42 "}, {"+42"}, {"00042"}, {"0"}, {"bad"}, {"9223372036854775808"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--limit=0"}, {"42", "--limit=1"}, {"42", "--limit=2"}, {"42", "--limit=3"}, {"42", "--limit=4"}, {"42", "--limit=5"}, {"42", "--limit=2", "--page=2"}, {"42", "--limit=2", "--page=3"}, {"42", "--limit=1", "--page=5"}, {"42", "--limit=1", "--page=10"}, {"42", "--page=2"}, {"42", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--limit=3", "--page=4000000000000000000"}, {"0", "--page=0"}, {"42", "--restrict=private", "--tag=ねこ & dog"}, {"42", "--restrict="}, {"42", "--restrict=invalid"}, {"42", "--restrict=PUBLIC"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=http://fixture"}, {"42", "--no-proxy"}, {"42", "--no-proxy=false"}, {"42", "--ndjson", "--json=false"}, {"42", "--ndjson=false"}, {"42", "--json", "--json=false"}, {"42", "-l=1", "-p=2"}, {"--", "-1"}}
		for _, suffix := range []string{"bookmarks/artworks", "bookmarks/novels", "bookmarks", "bookmarks/artworks?tag=cat&rest=hide"} {
			variants = append(variants, []string{"https://www.pixiv.net/users/42/" + suffix})
		}
		if kind != "user" {
			for _, typ := range []string{"", "illust", "artworks", "ALL", "invalid"} {
				variants = append(variants, []string{"42", "--type=" + typ})
			}
			variants = append(variants, []string{"0", "--type=invalid", "--page=0"}, []string{"42", "-t=" + kind})
		}
		for _, args := range variants {
			for _, mode := range []string{"human", "json", "ndjson", "false", "auto"} {
				current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Identity: 42, Mode: mode, Bodies: bodies}
				if mode == "json" || mode == "ndjson" {
					current.Args = append(current.Args, "--"+mode)
				}
				if mode == "false" {
					current.Args = append(current.Args, "--json=false")
				}
				captured := bookmarkListsCapture(t, current)
				if len(args) == 1 && args[0] == "42" && mode == "human" && (captured.Exit != 0 || captured.Stdout == "") {
					t.Fatalf("normal %s list failed: %#v", kind, captured)
				}
				rows = append(rows, captured)
			}
		}
		inputs := []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "https://www.pixiv.net/users/43\r\n", `{"id":"43","type":"user","url":"https://www.pixiv.net/users/43"}`, `{"id":43,"type":"user","url":"anything"}`, `{"id":"43","type":"artwork","url":"anything"}`, `{"id":"43","type":"novel","url":"anything"}`, `{"id":"43","type":"user","url":""}`, `{"id":0,"type":"user","url":"anything"}`, `{"id":1.5,"type":"user","url":"anything"}`, `{"id":9223372036854775808,"type":"user","url":"anything"}`, `{"id":"43","type":null,"url":"anything"}`, `{"type":"user","url":"anything"}`, "{", "[]", `{"id":"43","type":"user","url":"anything"} trailing`, `{"id":"43","type":"user","url":"anything"}` + "\n" + `{"id":"44","type":"user","url":"anything"}`, "\n " + `{"id":"43","type":"user","url":"anything"}` + "\r\n"}
		for _, input := range inputs {
			for _, args := range [][]string{{"--limit=0"}, {"42", "--limit=0"}} {
				for _, mode := range []string{"human", "json", "ndjson"} {
					current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Input: input, Identity: 42, Mode: mode, Bodies: bodies}
					if mode != "human" {
						current.Args = append(current.Args, "--"+mode)
					}
					rows = append(rows, bookmarkListsCapture(t, current))
				}
			}
		}
		for scenario := 0; scenario < 6; scenario++ {
			currentBodies := append([]json.RawMessage{}, bodies...)
			identity, writer := int64(42), ""
			for index := 0; index < len(currentBodies); index += 2 {
				key, endpoint := "illusts", "/v1/user/bookmarks/illust"
				if kind == "novel" || kind == "all" && index == 2 {
					key, endpoint = "novels", "/v1/user/bookmarks/novel"
				}
				switch scenario {
				case 0:
					currentBodies[index] = json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net%s?max_bookmark_id=30"}`, key, endpoint))
				case 1:
					currentBodies[index+1] = json.RawMessage(fmt.Sprintf(`{"%s":null}`, key))
				case 2:
					currentBodies[index+1] = currentBodies[index]
				case 3:
					identity = 0
				case 4:
					writer = "short"
				case 5:
					currentBodies[index], currentBodies[index+1] = json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":null}`, key)), json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":null}`, key))
				}
			}
			for _, args := range [][]string{{"42", "--limit=0"}, {"--limit=0"}, {}} {
				for _, mode := range []string{"human", "json", "ndjson", "auto"} {
					current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Identity: identity, Mode: mode, Writer: writer, Bodies: currentBodies}
					if mode == "json" || mode == "ndjson" {
						current.Args = append(current.Args, "--"+mode)
					}
					rows = append(rows, bookmarkListsCapture(t, current))
				}
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-lists.json", rows, *updateBookmarkLists)
}

type migrationBookmarkListsPipeWriter struct{ io.Writer }

func (migrationBookmarkListsPipeWriter) Fd() uintptr { return ^uintptr(0) }

type migrationBookmarkListsReader struct {
	input io.Reader
	bytes int
}

func (r *migrationBookmarkListsReader) Read(body []byte) (int, error) {
	n, err := r.input.Read(body)
	r.bytes += n
	return n, err
}
