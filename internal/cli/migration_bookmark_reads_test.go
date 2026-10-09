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
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateBookmarkReads = flag.Bool("migration-update-bookmark-reads", false, "capture bookmark detail and tag outputs, input, identity, flags, and logical pages")

type bookmarkReadsRow struct {
	bookmarkListsRow
	Operation   string  `json:"operation"`
	Status      int     `json:"status,omitempty"`
	WireBody    *string `json:"wire_body,omitempty"`
	DefaultType bool    `json:"default_type,omitempty"`
}

func bookmarkReadsBodies(operation, kind string) []json.RawMessage {
	if operation == "detail" {
		return []json.RawMessage{json.RawMessage(`{"bookmark_detail":{"is_bookmarked":true,"restrict":"private","tags":[{"name":"cat<&\nline","is_registered":true},{"name":"skip"},{"name":"cat<&\nline","is_registered":true},{"name":"日本語","is_registered":true},{"name":"","is_registered":true}]}}`)}
	}
	paths := []string{"/v1/user/bookmark-tags/illust"}
	if kind == "novel" {
		paths = []string{"/v1/user/bookmark-tags/novel"}
	}
	if kind == "all" {
		paths = append(paths, "/v1/user/bookmark-tags/novel")
	}
	var bodies []json.RawMessage
	for index, path := range paths {
		count := 3 + index
		if kind == "novel" {
			count = 4
		}
		next := fmt.Sprintf(`"https://app-api.pixiv.net%s?offset=30"`, path)
		if strings.HasSuffix(path, "/novel") {
			next = "null"
		}
		bodies = append(bodies, json.RawMessage(fmt.Sprintf(`{"bookmark_tags":[{"name":"same<&\nline","count":%d},{"name":"second","count":-1}],"next_url":%s}`, count, next)))
		bodies = append(bodies, json.RawMessage(`{"bookmark_tags":[{"name":"second","count":9},{"name":"日本語","count":9223372036854775807}],"next_url":null}`))
	}
	return bodies
}

func bookmarkReadsCommand(operation, kind string, input io.Reader, output, diagnostics io.Writer, jsonOut func(*bool) (bool, error), invoke func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error) (*cobra.Command, []string) {
	return bookmarkcmd.New(deps.Data{Input: input, Output: output, ErrorOutput: diagnostics, UsageError: newUsageError, JSONOut: jsonOut, Pooled: invoke}), []string{"bookmark", operation, "--type=" + kind}
}

func bookmarkReadsCapture(t *testing.T, current bookmarkReadsRow) bookmarkReadsRow {
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
	command, prefix := bookmarkReadsCommand(current.Operation, current.Kind, reader, out, &diagnostics, func(value *bool) (bool, error) {
		current.JSON = value
		return current.Mode == "json" || current.Mode == "config_json", nil
	}, func(ctx context.Context, request deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
		if request.UserID != 0 {
			t.Fatal("bookmark read target selected authentication account")
		}
		current.Proxy = request.HTTPSProxyOverride
		transport := migrationDateTransport(func(req *http.Request) (*http.Response, error) {
			payload := []byte(fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d}}`, current.Identity))
			status := http.StatusOK
			if req.URL.Host != "oauth.secure.pixiv.net" {
				index := 0
				if current.Operation == "detail" {
					if req.URL.Path != "/v2/illust/bookmark/detail" && req.URL.Path != "/v2/novel/bookmark/detail" {
						t.Fatalf("unexpected detail request %s", req.URL)
					}
				} else {
					switch req.URL.Path {
					case "/v1/user/bookmark-tags/novel":
						if current.Kind == "all" {
							index = 2
						} else if current.Kind != "novel" {
							t.Fatalf("unexpected novel request %s", req.URL)
						}
					case "/v1/user/bookmark-tags/illust":
					default:
						t.Fatalf("unexpected tags request %s", req.URL)
					}
					if req.URL.Query().Get("offset") != "" {
						index++
					}
				}
				if req.Method != "GET" || req.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatal("unexpected bookmark read method or credentials")
				}
				current.Paths = append(current.Paths, req.URL.Path)
				current.Queries = append(current.Queries, req.URL.Query())
				payload = current.Bodies[index]
				if current.WireBody != nil {
					payload = []byte(*current.WireBody)
				}
				if current.Status != 0 {
					status = current.Status
				}
			}
			return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(payload)), Request: req}, nil
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
	if current.DefaultType {
		prefix = prefix[:2]
	}
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

func bookmarkReadsCase(operation, kind, mode string, args []string) bookmarkReadsRow {
	current := bookmarkReadsRow{Operation: operation, bookmarkListsRow: bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Identity: 42, Mode: mode, Bodies: bookmarkReadsBodies(operation, kind)}}
	switch mode {
	case "json", "ndjson":
		current.Args = append(current.Args, "--"+mode)
	case "false":
		current.Args = append(current.Args, "--json=false")
	}
	return current
}

func TestMigrationBookmarkReadsPreservesDetailTagsFlagsIdentityAndLogicalPages(t *testing.T) {
	var rows []bookmarkReadsRow
	for _, operation := range []string{"detail", "tags"} {
		kinds := []string{"artwork", "novel"}
		if operation == "tags" {
			kinds = append(kinds, "all")
		}
		for _, kind := range kinds {
			variants := [][]string{{"42"}, {}, {" 42 "}, {"+42"}, {"00042"}, {"0"}, {"bad"}, {"9223372036854775808"}, {"42", "43"}, {"--", "-1"}, {"42", "--type="}, {"42", "--type= artwork "}, {"42", "--type= novel "}, {"42", "--type=illust"}, {"42", "--type=artworks"}, {"42", "--type=ALL"}, {"42", "--type=invalid"}, {"0", "--type=invalid"}, {"42", "-t=" + kind}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=http://fixture"}, {"42", "--no-proxy"}, {"42", "--no-proxy=false"}, {"42", "--json", "--json=false"}, {"https://www.pixiv.net/artworks/42"}, {"https://www.pixiv.net/novel/show.php?id=42"}, {"https://www.pixiv.net/users/42"}, {"https://www.pixiv.net/users/42/bookmarks/artworks"}, {"https://www.pixiv.net/users/42/bookmarks/novels"}, {"https://www.pixiv.net/users/42/bookmarks"}, {"https://www.pixiv.net/users/42/bookmarks/artworks?tag=cat&rest=hide"}}
			if operation == "detail" {
				variants = append(variants, []string{"42", "--type=all"}, []string{"42", "--limit=1"}, []string{"42", "--page=1"}, []string{"42", "--restrict=private"})
			} else {
				variants = append(variants, []string{"42", "--limit=0"}, []string{"42", "--limit=1"}, []string{"42", "--limit=2"}, []string{"42", "--limit=3"}, []string{"42", "--limit=4"}, []string{"42", "--limit=5"}, []string{"42", "--limit=8"}, []string{"42", "--limit=2", "--page=2"}, []string{"42", "--limit=2", "--page=3"}, []string{"42", "--limit=3", "--page=2"}, []string{"42", "--limit=1", "--page=10"}, []string{"42", "--page=2"}, []string{"42", "--page=0"}, []string{"42", "--limit=-1"}, []string{"42", "--limit=0", "--page=2"}, []string{"42", "--limit=3", "--page=4000000000000000000"}, []string{"0", "--page=0"}, []string{"42", "--restrict=private"}, []string{"42", "--restrict="}, []string{"42", "--restrict=invalid"}, []string{"42", "--restrict=PUBLIC"}, []string{"42", "--ndjson", "--json=false"}, []string{"42", "--ndjson=false"}, []string{"42", "--tag=cat"}, []string{"42", "-l=1", "-p=2"})
			}
			for _, args := range variants {
				for _, mode := range []string{"human", "json", "ndjson", "false", "auto", "config_json"} {
					captured := bookmarkReadsCapture(t, bookmarkReadsCase(operation, kind, mode, args))
					if len(args) == 1 && args[0] == "42" && (mode != "ndjson" || operation == "tags") {
						if captured.Exit != 0 || captured.Error != "" || captured.Stdout == "" || len(captured.Paths) != 1 || len(captured.Queries) != 1 || len(captured.Committed) != 1 {
							t.Fatalf("normal %s/%s read failed: %#v", operation, kind, captured)
						}
						expectedPath, expectedID := "/v1/user/bookmark-tags/illust", "user_id"
						if operation == "detail" {
							expectedPath, expectedID = "/v2/illust/bookmark/detail", "illust_id"
						}
						if kind == "novel" {
							expectedPath = strings.Replace(expectedPath, "illust", "novel", 1)
							if operation == "detail" {
								expectedID = "novel_id"
							}
						}
						if captured.Paths[0] != expectedPath || captured.Queries[0].Get(expectedID) != "42" {
							t.Fatalf("normal request differs: %#v", captured)
						}
					}
					rows = append(rows, captured)
				}
			}
			if kind == "artwork" {
				for _, mode := range []string{"human", "json", "ndjson", "false", "auto", "config_json"} {
					current := bookmarkReadsCase(operation, kind, mode, []string{"42"})
					current.DefaultType = true
					captured := bookmarkReadsCapture(t, current)
					if mode != "ndjson" || operation == "tags" {
						if captured.Exit != 0 || captured.Error != "" || captured.Stdout == "" || len(captured.Paths) != 1 {
							t.Fatalf("default artwork read failed: %#v", captured)
						}
					}
					rows = append(rows, captured)
				}
			}
			for _, identity := range []int64{0, 43} {
				for _, args := range [][]string{{}, {"99"}} {
					for _, mode := range []string{"human", "json", "ndjson", "auto"} {
						current := bookmarkReadsCase(operation, kind, mode, args)
						current.Identity = identity
						rows = append(rows, bookmarkReadsCapture(t, current))
					}
				}
			}
			for _, writer := range []string{"broken", "short", "other"} {
				for _, mode := range []string{"human", "json", "ndjson", "auto"} {
					current := bookmarkReadsCase(operation, kind, mode, []string{"42"})
					current.Writer = writer
					rows = append(rows, bookmarkReadsCapture(t, current))
				}
			}
			for _, mode := range []string{"human", "json", "ndjson", "auto"} {
				current := bookmarkReadsCase(operation, kind, mode, []string{"42"})
				current.Status = 401
				rows = append(rows, bookmarkReadsCapture(t, current))
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-reads.json", rows, *updateBookmarkReads)
}
