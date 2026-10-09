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
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	timelinecmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/timeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCLITimeline = flag.Bool("migration-update-cli-timeline", false, "capture timeline command execution contracts")

type migrationTimelineRequest struct {
	Path  string     `json:"path"`
	Query url.Values `json:"query"`
}

type migrationTimelineCase struct {
	Name       string                     `json:"name"`
	Operation  string                     `json:"operation"`
	Kind       string                     `json:"kind"`
	Args       []string                   `json:"args"`
	Source     string                     `json:"source"`
	Mode       string                     `json:"mode"`
	Writer     string                     `json:"writer"`
	Input      string                     `json:"input"`
	ReadError  bool                       `json:"read_error"`
	ParserOnly bool                       `json:"parser_only"`
	Requests   []migrationTimelineRequest `json:"requests"`
	Commits    []bool                     `json:"commits"`
	Error      string                     `json:"error"`
	Stdout     string                     `json:"stdout"`
	Stderr     string                     `json:"stderr"`
	Exit       int                        `json:"exit"`
	Bytes      int                        `json:"bytes"`
	Proxy      *string                    `json:"proxy"`
	JSON       *bool                      `json:"json"`
}

func migrationTimelineBodies(t *testing.T) map[string][]json.RawMessage {
	t.Helper()
	sources := map[string][]json.RawMessage{}
	batch := func(key string, items []json.RawMessage, next any) json.RawMessage {
		body, err := json.Marshal(map[string]any{key: items, "next_url": next})
		if err != nil {
			t.Fatal(err)
		}
		return body
	}
	artwork := func(id int, kind, title string) json.RawMessage {
		return json.RawMessage(fmt.Sprintf(`{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":%q,"type":%q,"total_bookmarks":12,"total_view":34,"tags":[{"name":"tag\nline"}],"user":{"id":9,"name":"author\nline"}}`, id, title, kind))
	}
	novel := func(id int, title string) json.RawMessage {
		return json.RawMessage(fmt.Sprintf(`{"id":%d,"create_date":"2024-01-02T03:04:05+09:00","title":%q,"text_length":123,"total_bookmarks":12,"total_view":34,"tags":[{"name":"tag\nline"}],"user":{"id":9,"name":"author\nline"}}`, id, title))
	}
	for _, op := range []string{"following", "latest"} {
		for _, kind := range []string{"artwork", "novel"} {
			name := op + "-" + kind
			key, path, continuation := "illusts", "/v2/illust/follow", "offset"
			first := []json.RawMessage{artwork(101, "illust", "first<&\nline"), artwork(102, "manga", "second")}
			last := []json.RawMessage{artwork(102, "manga", "duplicate"), artwork(103, "ugoira", "last"), artwork(104, "illust", "fourth")}
			if kind == "novel" {
				key, path = "novels", "/v1/novel/follow"
				first = []json.RawMessage{novel(201, "first<&\nline"), novel(202, "second")}
				last = []json.RawMessage{novel(202, "duplicate"), novel(203, "last"), novel(204, "fourth")}
			}
			if op == "latest" {
				if kind == "artwork" {
					path, continuation = "/v1/illust/new", "max_illust_id"
				} else {
					path, continuation = "/v1/novel/new", "max_novel_id"
				}
			}
			next := "https://app-api.pixiv.net" + path + "?" + continuation + "=30"
			sources[name] = []json.RawMessage{batch(key, first, next), batch(key, last, nil)}
			sources[name+"-empty-first"] = []json.RawMessage{batch(key, []json.RawMessage{}, next), batch(key, last, nil)}
			sources[name+"-null-last"] = []json.RawMessage{batch(key, first, next), json.RawMessage(fmt.Sprintf(`{"%s":null}`, key))}
		}
	}
	sources["following-artwork-filter-empty"] = []json.RawMessage{batch("illusts", []json.RawMessage{artwork(101, "manga", "excluded"), artwork(102, "manga", "excluded")}, "https://app-api.pixiv.net/v2/illust/follow?offset=30"), sources["following-artwork"][1]}
	sources["following-artwork-manga-filter-empty"] = []json.RawMessage{batch("illusts", []json.RawMessage{artwork(101, "illust", "excluded"), artwork(102, "illust", "excluded")}, "https://app-api.pixiv.net/v2/illust/follow?offset=30"), batch("illusts", []json.RawMessage{artwork(103, "manga", "third"), artwork(104, "illust", "excluded"), artwork(105, "manga", "fifth")}, nil)}
	sources["following-artwork-repeated"] = []json.RawMessage{sources["following-artwork"][0], sources["following-artwork"][0]}
	sources["latest-artwork-bad-next"] = []json.RawMessage{batch("illusts", []json.RawMessage{artwork(101, "illust", "first")}, "https://app-api.pixiv.net/v1/illust/new?offset=30"), sources["latest-artwork"][1]}
	return sources
}

func migrationTimelineReadBodies(t *testing.T) map[string][]json.RawMessage {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "cli-timeline-bodies.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources map[string][]json.RawMessage
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	return sources
}

func TestMigrationTimelinePreservesExecutionOutputFilteringWindowsAndNoInput(t *testing.T) {
	sources := migrationTimelineBodies(t)
	recommendedFixture(t, "cli-timeline-bodies.json", sources, *updateCLITimeline)
	var rows []migrationTimelineCase
	add := func(name, op, kind, mode string, args ...string) {
		current := migrationTimelineCase{Name: name, Operation: op, Kind: kind, Args: append([]string{}, args...), Source: op + "-" + kind, Mode: mode, Requests: []migrationTimelineRequest{}, Commits: []bool{}}
		if mode == "json" || mode == "ndjson" {
			current.Args = append(current.Args, "--"+mode)
		}
		if mode == "false" {
			current.Args = append(current.Args, "--json=false")
		}
		rows = append(rows, current)
	}
	for _, op := range []string{"following", "latest"} {
		for _, kind := range []string{"artwork", "novel"} {
			for _, mode := range []string{"human", "json", "ndjson"} {
				add(op+"-"+kind+"-"+mode, op, kind, mode, "--type="+kind, "--limit=0")
			}
		}
	}
	add("following-artwork-auto", "following", "artwork", "auto", "--type=artwork", "--limit=0")
	add("latest-novel-auto", "latest", "novel", "auto", "--type=novel", "--limit=0")
	add("following-artwork-false", "following", "artwork", "false", "--type=artwork", "--limit=0")
	add("latest-novel-false", "latest", "novel", "false", "--type=novel", "--limit=0")

	for _, value := range []string{"illust-and-ugoira", "illust", "manga", "ugoira", "illustration", "", " ILLUST ", "invalid"} {
		add("following-subtype:"+value, "following", "artwork", "json", "--type=artwork", "--content-type="+value, "--limit=0")
	}
	add("following-alias", "following", "artwork", "human", "--type=illust", "--restrict=private")
	add("following-private-novel", "following", "novel", "json", "--type=novel", "--restrict=private")
	add("following-empty-restrict", "following", "artwork", "json", "--type=artwork", "--restrict=")
	add("following-invalid-restrict", "following", "novel", "ndjson", "--type=novel", "--restrict=invalid")
	add("following-novel-subtype-changed", "following", "novel", "human", "--type=novel", "--content-type=all", "--page=0")
	add("latest-alias-illust", "latest", "artwork", "json", "--type=illust")
	add("latest-alias-manga", "latest", "artwork", "ndjson", "--type=manga", "--limit=0")
	add("latest-manga", "latest", "artwork", "human", "--type=artwork", "--content-type=manga")
	for _, value := range []string{"all", "ugoira", "ILLUST"} {
		add("latest-invalid-subtype:"+value, "latest", "artwork", "json", "--type=artwork", "--content-type="+value)
	}
	add("latest-alias-conflict", "latest", "artwork", "human", "--type=illust", "--content-type=illust", "--page=0")
	add("latest-novel-subtype-changed", "latest", "novel", "json", "--type=novel", "--content-type=illust")
	add("following-missing-type", "following", "artwork", "human")
	add("latest-missing-type", "latest", "artwork", "json")
	add("following-uppercase-type", "following", "artwork", "human", "--type=ARTWORK", "--page=0")
	add("latest-invalid-type", "latest", "artwork", "human", "--type=ugoira")
	for _, current := range []struct {
		name string
		args []string
	}{
		{"limit-one", []string{"--limit=1"}}, {"limit-two-page-two", []string{"--limit=2", "--page=2"}}, {"past-end", []string{"--limit=1", "--page=9"}}, {"page-default-limit", []string{"--page=2"}}, {"invalid-page", []string{"--page=0"}}, {"invalid-limit", []string{"--limit=-1"}}, {"all-page-conflict", []string{"--limit=0", "--page=2"}}, {"overflow-window", []string{"--limit=3", "--page=4000000000000000000"}}, {"proxy-conflict", []string{"--proxy=", "--no-proxy"}}, {"proxy", []string{"--proxy=http://fixture"}}, {"no-proxy", []string{"--no-proxy"}}, {"output-conflict", []string{"--ndjson", "--json=false"}},
	} {
		add(current.name, "following", "artwork", "human", append([]string{"--type=artwork"}, current.args...)...)
	}
	add("filtered-first-empty", "following", "artwork", "ndjson", "--type=artwork", "--content-type=illust", "--limit=2")
	rows[len(rows)-1].Source = "following-artwork-filter-empty"
	for _, variant := range []struct {
		name string
		args []string
	}{
		{"manga-empty-first-limit", []string{"--limit=1"}}, {"manga-empty-first-window", []string{"--limit=1", "--page=2"}}, {"manga-empty-first-default", nil},
	} {
		add(variant.name, "following", "artwork", "json", append([]string{"--type=artwork", "--content-type=manga"}, variant.args...)...)
		rows[len(rows)-1].Source = "following-artwork-manga-filter-empty"
	}
	add("restrict-output-conflict", "following", "novel", "human", "--type=novel", "--restrict=invalid", "--ndjson", "--json")

	add("empty-novel-first", "latest", "novel", "json", "--type=novel", "--limit=0")
	rows[len(rows)-1].Source = "latest-novel-empty-first"
	add("artwork-malformed-after-page", "following", "artwork", "ndjson", "--type=artwork", "--limit=0")
	rows[len(rows)-1].Source = "following-artwork-null-last"
	add("novel-malformed-collected", "latest", "novel", "json", "--type=novel", "--limit=0")
	rows[len(rows)-1].Source = "latest-novel-null-last"
	add("repeated-cursor", "following", "artwork", "json", "--type=artwork", "--limit=0")
	rows[len(rows)-1].Source = "following-artwork-repeated"
	add("latest-offset-next", "latest", "artwork", "json", "--type=artwork", "--limit=0")
	rows[len(rows)-1].Source = "latest-artwork-bad-next"
	for _, mode := range []string{"human", "json", "ndjson"} {
		for _, failure := range []string{"broken", "short", "other"} {
			if failure == "short" && mode != "json" {
				continue
			}
			add("writer:"+mode+":"+failure, "following", "artwork", mode, "--type=artwork", "--limit=0")
			rows[len(rows)-1].Writer = failure
		}
	}
	add("stdin-ignored", "latest", "novel", "json", "--type=novel")
	rows[len(rows)-1].Input = "{\"id\":42}\nhttps://www.pixiv.net/users/43\n"
	add("failed-reader-ignored", "following", "artwork", "human", "--type=artwork")
	rows[len(rows)-1].ReadError = true
	add("positional-rejected", "following", "artwork", "human", "--type=artwork", "42")
	add("unknown-flag-leaf", "latest", "novel", "human", "--type=novel", "--restrict=public")
	rows[len(rows)-1].ParserOnly = true
	add("malformed-limit-leaf", "following", "artwork", "json", "--type=artwork", "--limit=bad")
	rows[len(rows)-1].ParserOnly = true
	if len(rows) > 75 {
		t.Fatalf("timeline main matrix grew to %d rows", len(rows))
	}
	for index := range rows {
		current := &rows[index]
		var output, diagnostics bytes.Buffer
		out := io.Writer(&output)
		if current.Writer != "" {
			out = migrationUserSearchWriter{&output, current.Writer}
		}
		if current.Mode == "auto" {
			out = &migrationUserWorksPipeWriter{Buffer: &output}
		}
		input := io.Reader(strings.NewReader(current.Input))
		if current.ReadError {
			input = migrationSearchFailedRead{}
		}
		reader := &migrationTimelineReader{input: input}
		command := timelinecmd.New(deps.Data{Input: reader, Output: out, UsageError: newUsageError, JSONOut: func(value *bool) (bool, error) { current.JSON = value; return current.Mode == "json", nil }, Pooled: func(ctx context.Context, request deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			current.Proxy = request.HTTPSProxyOverride
			client, _, err := pixiv.OpenWith(ctx, "fixture-refresh-42", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
				if req.URL.Host == "oauth.secure.pixiv.net" {
					return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"access_token":"fixture-access-42","refresh_token":"fixture-rotated-42","expires_in":3600,"user":{"id":42}}`)), Request: req}, nil
				}
				if req.Method != "GET" || req.Header.Get("Authorization") != "Bearer fixture-access-42" {
					t.Fatal("unexpected timeline request")
				}
				bodies := sources[current.Source]
				step := len(current.Requests)
				current.Requests = append(current.Requests, migrationTimelineRequest{req.URL.Path, req.URL.Query()})
				if step >= len(bodies) {
					step = len(bodies) - 1
				}
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(bodies[step])), Request: req}, nil
			})}})
			if err != nil {
				return err
			}
			committed, err := invoke(ctx, client)
			current.Commits = append(current.Commits, committed)
			return err
		}})
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(command)
		root.SetOut(out)
		root.SetErr(&diagnostics)
		root.SetArgs(append([]string{"timeline", current.Operation}, current.Args...))
		err := root.Execute()
		if err != nil {
			current.Error = err.Error()
		}
		current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, current.Mode == "ndjson" || current.Mode == "auto", current.Mode != "human" && current.Mode != "auto")
		current.Stdout, current.Stderr, current.Bytes = output.String(), diagnostics.String(), reader.bytes
		if current.Bytes != 0 {
			t.Fatal("timeline consumed stdin")
		}
		if reader.calls != 0 {
			t.Fatal("timeline called stdin reader")
		}
		if current.ReadError && len(current.Requests) == 0 {
			t.Fatal("failed reader prevented timeline execution")
		}
	}
	recommendedFixture(t, "cli-timeline.json", rows, *updateCLITimeline)
}

type migrationTimelineReader struct {
	input io.Reader
	bytes int
	calls int
}

func (r *migrationTimelineReader) Read(p []byte) (int, error) {
	r.calls++
	n, err := r.input.Read(p)
	r.bytes += n
	return n, err
}
