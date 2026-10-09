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
	commentcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/comment"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCommentReads = flag.Bool("migration-update-comment-reads", false, "capture bookmark detail and tag outputs, input, identity, flags, and logical pages")

type commentReadsRow struct {
	bookmarkListsRow
	Operation   string  `json:"operation"`
	Status      int     `json:"status,omitempty"`
	WireBody    *string `json:"wire_body,omitempty"`
	DefaultType bool    `json:"default_type,omitempty"`
}

func commentReadsBodies(operation, kind string) []json.RawMessage {
	if operation == "stamps" {
		return []json.RawMessage{json.RawMessage(`{"stamps":[{"stamp_id":1,"stamp_url":"https://s.pximg.net/common/images/stamp/generated-stamps/1.png"}]}`)}
	}
	path := "/v3/illust/comments"
	if kind == "novel" {
		path = "/v2/novel/comments"
	}
	return []json.RawMessage{json.RawMessage(fmt.Sprintf(`{"comments":[{"id":1,"created_at":"2026-01-01T00:00:00Z","comment":"hello<&\nline","user":{"id":7,"name":"raw\nname"},"parent_comment":{"id":9,"created_at":"2026-01-01T00:00:00Z","comment":"parent","user":{"id":8,"name":"parent"}}},{"id":2,"created_at":"2026-01-01T00:00:00Z","comment":"second"}],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, path)), json.RawMessage(`{"comments":[{"id":3,"created_at":"2026-01-01T00:00:00Z","comment":"third"}],"total_comments":7,"access_control":{"can_comment":true,"is_locked":false},"next_url":null}`)}
}
func commentReadsCommand(operation, kind string, input io.Reader, output, diagnostics io.Writer, jsonOut func(*bool) (bool, error), invoke func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error) (*cobra.Command, []string) {
	prefix := []string{"comment", "--type=" + kind}
	if operation == "stamps" {
		prefix = []string{"comment", "stamps"}
	}
	return commentcmd.New(deps.Data{Input: input, Output: output, ErrorOutput: diagnostics, UsageError: newUsageError, JSONOut: jsonOut, Pooled: invoke}), prefix
}

func commentReadsCapture(t *testing.T, current commentReadsRow) commentReadsRow {
	t.Helper()
	current.Paths, current.Queries, current.Committed = []string{}, []url.Values{}, []bool{}
	var output, diagnostics bytes.Buffer
	out := io.Writer(&output)
	if current.Writer == "short_success" {
		out = commentShortSuccessWriter{out}
	} else if current.Writer != "" {
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
	command, prefix := commentReadsCommand(current.Operation, current.Kind, reader, out, &diagnostics, func(value *bool) (bool, error) {
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
				if req.URL.Path != "/v3/illust/comments" && req.URL.Path != "/v2/novel/comments" && req.URL.Path != "/v1/stamps" {
					t.Fatalf("unexpected comment request %s", req.URL)
				}
				if req.URL.Query().Get("offset") != "" {
					index++
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
		prefix = prefix[:1]
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

func commentReadsCase(operation, kind, mode string, args []string) commentReadsRow {
	current := commentReadsRow{Operation: operation, bookmarkListsRow: bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Identity: 42, Mode: mode, Bodies: commentReadsBodies(operation, kind)}}
	switch mode {
	case "json", "ndjson":
		current.Args = append(current.Args, "--"+mode)
	case "false":
		current.Args = append(current.Args, "--json=false")
	}
	return current
}

func TestMigrationCommentReadsPreservesReadStampsAndLogicalPages(t *testing.T) {
	var rows []commentReadsRow
	for _, kind := range []string{"artwork", "novel"} {
		for _, mode := range []string{"human", "json", "ndjson", "auto", "false", "config_json"} {
			for _, args := range [][]string{{"42"}, {"42", "--limit=0"}, {"42", "--limit=2", "--page=2"}} {
				rows = append(rows, commentReadsCapture(t, commentReadsCase("read", kind, mode, args)))
			}
		}
		for _, source := range []string{"0", "https://www.pixiv.net/artworks/42", "https://www.pixiv.net/novel/show.php?id=42", "https://www.pixiv.net/users/42"} {
			rows = append(rows, commentReadsCapture(t, commentReadsCase("read", kind, "json", []string{source})))
		}
		for _, args := range [][]string{{"42", "--limit=-1"}, {"42", "--page=0"}, {"42", "--ndjson", "--json"}, {"42", "--no-proxy"}, {"42", "--proxy=http://fixture.invalid:8080"}} {
			rows = append(rows, commentReadsCapture(t, commentReadsCase("read", kind, "human", args)))
		}
	}
	for _, mode := range []string{"human", "json", "ndjson", "auto", "false", "config_json"} {
		rows = append(rows, commentReadsCapture(t, commentReadsCase("stamps", "", mode, nil)))
	}
	for _, operation := range []string{"read", "stamps"} {
		for _, mode := range []string{"human", "json", "ndjson"} {
			for _, writer := range []string{"other", "broken", "short"} {
				args := []string{"42"}
				if operation == "stamps" {
					args = nil
				}
				current := commentReadsCase(operation, "artwork", mode, args)
				current.Writer = writer
				rows = append(rows, commentReadsCapture(t, current))
			}
		}
	}
	for _, kind := range []string{"", "invalid"} {
		current := commentReadsCase("read", kind, "json", []string{"42"})
		current.DefaultType = kind == ""
		rows = append(rows, commentReadsCapture(t, current))
	}
	for _, input := range []string{"42\n", "42\r\n", "{\"id\":42}\n", ""} {
		current := commentReadsCase("read", "artwork", "json", nil)
		current.Input = input
		rows = append(rows, commentReadsCapture(t, current))
	}
	for _, kind := range []string{"artwork", "novel"} {
		for _, mode := range []string{"human", "json", "ndjson"} {
			current := commentReadsCase("read", kind, mode, []string{"42", "--limit=0"})
			current.Bodies[1] = json.RawMessage(`{"comments":{}}`)
			rows = append(rows, commentReadsCapture(t, current))
		}
		current := commentReadsCase("read", kind, "json", []string{"42", "--limit=0"})
		var first map[string]any
		if err := json.Unmarshal(current.Bodies[0], &first); err != nil {
			t.Fatal(err)
		}
		first["total_comments"] = 0
		first["comment_access_control"] = 3
		body, err := json.Marshal(first)
		if err != nil {
			t.Fatal(err)
		}
		current.Bodies[0] = body
		rows = append(rows, commentReadsCapture(t, current))
	}
	current := commentReadsCase("stamps", "", "json", nil)
	current.Input = "ignored input"
	current.ReadError = true
	rows = append(rows, commentReadsCapture(t, current))
	current = commentReadsCase("read", "artwork", "json", nil)
	current.ReadError = true
	rows = append(rows, commentReadsCapture(t, current))
	for _, mode := range []string{"human", "json", "ndjson"} {
		current := commentReadsCase("read", "artwork", mode, []string{"42"})
		current.Bodies[0] = json.RawMessage(`{"comments":[],"next_url":null}`)
		rows = append(rows, commentReadsCapture(t, current))
	}
	current = commentReadsCase("read", "artwork", "human", []string{"42"})
	current.Writer = "short_success"
	rows = append(rows, commentReadsCapture(t, current))
	recommendedFixture(t, "cli-comment-reads.json", rows, *updateCommentReads)
}

type commentShortSuccessWriter struct{ io.Writer }

func (w commentShortSuccessWriter) Write(body []byte) (int, error) {
	if len(body) > 10 {
		body = body[:10]
	}
	return w.Writer.Write(body)
}
