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

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	mypixivcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/mypixiv"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCLIMyPixiv = flag.Bool("migration-update-cli-mypixiv", false, "capture mypixiv command execution contracts")

type migrationMyPixivRequest struct {
	Path  string     `json:"path"`
	Query url.Values `json:"query"`
}

type migrationMyPixivCase struct {
	Name       string                    `json:"name"`
	Operation  string                    `json:"operation"`
	Kind       string                    `json:"kind"`
	Args       []string                  `json:"args"`
	Source     string                    `json:"source"`
	Mode       string                    `json:"mode"`
	Writer     string                    `json:"writer"`
	Input      string                    `json:"input"`
	ReadError  bool                      `json:"read_error"`
	ParserOnly bool                      `json:"parser_only"`
	Requests   []migrationMyPixivRequest `json:"requests"`
	Commits    []bool                    `json:"commits"`
	Error      string                    `json:"error"`
	Stdout     string                    `json:"stdout"`
	Stderr     string                    `json:"stderr"`
	Exit       int                       `json:"exit"`
	Bytes      int                       `json:"bytes"`
	Proxy      *string                   `json:"proxy"`
	JSON       *bool                     `json:"json"`
}

func migrationMyPixivBodies(t *testing.T) map[string][]json.RawMessage {
	t.Helper()
	sources := map[string][]json.RawMessage{}
	original := migrationTimelineBodies(t)
	for _, kind := range []string{"artwork", "novel"} {
		oldPath, newPath := "/v2/illust/follow", "/v2/illust/mypixiv"
		if kind == "novel" {
			oldPath, newPath = "/v1/novel/follow", "/v1/novel/mypixiv"
		}
		for _, suffix := range []string{"", "-empty-first", "-null-last"} {
			for _, body := range original["following-"+kind+suffix] {
				sources["works-"+kind+suffix] = append(sources["works-"+kind+suffix], json.RawMessage(strings.ReplaceAll(string(body), oldPath, newPath)))
			}
		}
		userPath := "/v1/user/illusts"
		if kind == "novel" {
			userPath = "/v1/user/novels"
		}
		for _, body := range sources["works-"+kind] {
			sources["user-"+kind] = append(sources["user-"+kind], json.RawMessage(strings.ReplaceAll(string(body), newPath, userPath)))
		}
	}
	sources["users-user"] = []json.RawMessage{json.RawMessage(`{"user_previews":[{"user":{"id":31,"name":"first<&\nline"}},{"user":{"id":32,"name":"second"}}],"next_url":"https://app-api.pixiv.net/v1/user/mypixiv?offset=30"}`), json.RawMessage(`{"user_previews":[{"user":{"id":32,"name":"duplicate"}},{"user":{"id":33,"name":"last"}}],"next_url":null}`)}
	sources["users-user-empty-first"] = []json.RawMessage{json.RawMessage(`{"user_previews":[],"next_url":"https://app-api.pixiv.net/v1/user/mypixiv?offset=30"}`), sources["users-user"][1]}
	sources["users-user-null-last"] = []json.RawMessage{sources["users-user"][0], json.RawMessage(`{"user_previews":null}`)}
	return sources
}

func migrationMyPixivReadBodies(t *testing.T) map[string][]json.RawMessage {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "cli-mypixiv-bodies.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources map[string][]json.RawMessage
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	return sources
}

func TestMigrationMyPixivPreservesExecutionOutputFilteringWindowsAndNoInput(t *testing.T) {
	sources := migrationMyPixivBodies(t)
	recommendedFixture(t, "cli-mypixiv-bodies.json", sources, *updateCLIMyPixiv)
	var rows []migrationMyPixivCase
	add := func(name, op, kind, mode string, args ...string) {
		current := migrationMyPixivCase{Name: name, Operation: op, Kind: kind, Args: append([]string{}, args...), Source: op + "-" + kind, Mode: mode, Requests: []migrationMyPixivRequest{}, Commits: []bool{}}
		if mode == "json" || mode == "ndjson" {
			current.Args = append(current.Args, "--"+mode)
		}
		if mode == "false" {
			current.Args = append(current.Args, "--json=false")
		}
		rows = append(rows, current)
	}

	for _, current := range []struct{ op, kind string }{{"works", "artwork"}, {"works", "novel"}, {"users", "user"}} {
		for _, mode := range []string{"human", "json", "ndjson", "auto", "false"} {
			args := []string{"--limit=0"}
			if current.op == "works" {
				args = append(args, "--type="+current.kind)
			}
			add(current.op+"-"+current.kind+"-"+mode, current.op, current.kind, mode, args...)
		}
	}
	for _, kind := range []string{"artwork", "illust", "manga", "novel"} {
		source := "user-artwork"
		if kind == "novel" {
			source = "user-novel"
		}
		for _, mode := range []string{"human", "json", "ndjson"} {
			add("user-"+kind+"-"+mode, "works", "artwork", mode, "123", "--type="+kind, "--limit=0")
			rows[len(rows)-1].Source = source
		}
	}
	for _, current := range []struct {
		name string
		args []string
	}{
		{"alias", []string{"--type=illust"}}, {"missing-type", nil}, {"unsupported", []string{"--type=manga"}}, {"uppercase", []string{"--type=ARTWORK"}},
		{"invalid-type-before-id", []string{"bad", "--type=invalid", "--page=0"}}, {"invalid-id", []string{"bad", "--type=artwork"}}, {"url-id", []string{"https://www.pixiv.net/users/123?secret=hidden", "--type=artwork"}},
		{"zero-id", []string{"0", "--type=artwork"}}, {"negative-id", []string{"--type=artwork", "--", "-1"}}, {"overflow-id", []string{"9223372036854775808", "--type=artwork"}},
		{"spaced-id", []string{" 123 ", "--type=novel"}}, {"plus-id", []string{"+123", "--type=novel"}},
		{"limit-window", []string{"--type=artwork", "--limit=2", "--page=2"}}, {"past-end", []string{"--type=novel", "--limit=1", "--page=9"}},
		{"default-page", []string{"--type=novel", "--page=2"}}, {"invalid-page", []string{"--type=artwork", "--page=0"}},
		{"invalid-limit", []string{"--type=novel", "--limit=-1"}}, {"all-page", []string{"--type=artwork", "--limit=0", "--page=2"}},
		{"overflow-window", []string{"--type=artwork", "--limit=3", "--page=4000000000000000000"}},
		{"proxy-conflict", []string{"--type=artwork", "--proxy=", "--no-proxy"}}, {"output-conflict", []string{"--type=novel", "--ndjson", "--json=false"}},
		{"type-output-priority", []string{"--type=invalid", "--ndjson", "--json"}},
	} {
		kind := "artwork"
		for _, arg := range current.args {
			if arg == "--type=novel" {
				kind = "novel"
			}
		}
		add(current.name, "works", kind, "human", current.args...)
		if current.name == "plus-id" {
			rows[len(rows)-1].Source = "user-novel"
		}
	}
	for _, op := range []string{"works", "users"} {
		kind := "artwork"
		args := []string{}
		if op == "works" {
			args = append(args, "--type=artwork")
		} else {
			kind = "user"
		}
		add(op+"-empty-first", op, kind, "json", append(args, "--limit=0")...)
		rows[len(rows)-1].Source = op + "-" + kind + "-empty-first"
		add(op+"-malformed-last", op, kind, "ndjson", append(args, "--limit=0")...)
		rows[len(rows)-1].Source = op + "-" + kind + "-null-last"
		for _, mode := range []string{"human", "json", "ndjson"} {
			for _, failure := range []string{"broken", "other", "short"} {
				if failure == "short" && mode != "json" {
					continue
				}
				add(op+"-writer:"+mode+":"+failure, op, kind, mode, append(args, "--limit=0")...)
				rows[len(rows)-1].Writer = failure
			}
		}
	}
	add("stdin-id", "works", "artwork", "json", "--type=artwork")
	rows[len(rows)-1].Input = "123\r\n"
	rows[len(rows)-1].Source = "user-artwork"
	add("stdin-empty", "works", "novel", "json", "--type=novel")
	rows[len(rows)-1].Input = "\n"
	add("stdin-reader-error", "works", "novel", "human", "--type=novel")
	rows[len(rows)-1].ReadError = true
	add("stdin-ignored-explicit", "works", "artwork", "json", "123", "--type=artwork")
	rows[len(rows)-1].Input = "456\n"
	rows[len(rows)-1].Source = "user-artwork"
	add("users-stdin-ignored", "users", "user", "json")
	rows[len(rows)-1].Input = "123\n"
	add("users-reader-ignored", "users", "user", "human")
	rows[len(rows)-1].ReadError = true
	add("users-positional-rejected", "users", "user", "json", "123")
	add("works-extra-positionals", "works", "artwork", "human", "123", "456", "--type=artwork")
	add("unknown-flag", "users", "user", "human", "--type=artwork")
	rows[len(rows)-1].ParserOnly = true
	add("malformed-limit", "works", "artwork", "json", "--type=artwork", "--limit=bad")
	rows[len(rows)-1].ParserOnly = true
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
		command := mypixivcmd.New(deps.Data{Input: reader, Output: out, UsageError: newUsageError, JSONOut: func(value *bool) (bool, error) { current.JSON = value; return current.Mode == "json", nil }, Pooled: func(ctx context.Context, request deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			current.Proxy = request.HTTPSProxyOverride
			client, _, err := pixiv.OpenWith(ctx, "fixture-refresh-42", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
				if req.URL.Host == "oauth.secure.pixiv.net" {
					return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"access_token":"fixture-access-42","refresh_token":"fixture-rotated-42","expires_in":3600,"user":{"id":42}}`)), Request: req}, nil
				}
				if req.Method != "GET" || req.Header.Get("Authorization") != "Bearer fixture-access-42" {
					t.Fatal("unexpected mypixiv request")
				}
				bodies := sources[current.Source]
				step := len(current.Requests)
				current.Requests = append(current.Requests, migrationMyPixivRequest{req.URL.Path, req.URL.Query()})
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
		root.SetArgs(append([]string{"mypixiv", current.Operation}, current.Args...))
		err := root.Execute()
		if err != nil {
			current.Error = err.Error()
		}
		current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, current.Mode == "ndjson" || current.Mode == "auto", current.Mode != "human" && current.Mode != "auto")
		current.Stdout, current.Stderr, current.Bytes = output.String(), diagnostics.String(), reader.bytes
		if current.Operation == "users" && current.Bytes != 0 {
			t.Fatal("mypixiv consumed stdin")
		}
		if current.Operation == "users" && reader.calls != 0 {
			t.Fatal("mypixiv called stdin reader")
		}
		if current.Operation == "users" && current.ReadError && len(current.Requests) == 0 {
			t.Fatal("failed reader prevented mypixiv execution")
		}
	}
	recommendedFixture(t, "cli-mypixiv.json", rows, *updateCLIMyPixiv)
}
