package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	commentcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/comment"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"io"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

var updateCommentMutations = flag.Bool("migration-update-comment-mutations", false, "capture frozen comment mutation contracts")

type commentMutationsRow struct {
	commentReadsRow
	Forms []url.Values `json:"forms"`
}

func commentMutationsCommand(operation, kind string, input io.Reader, output, diagnostics io.Writer, jsonOut func(*bool) (bool, error), invoke func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error) (*cobra.Command, []string) {
	return commentcmd.New(deps.Data{Input: input, Output: output, ErrorOutput: diagnostics, UsageError: newUsageError, JSONOut: jsonOut, Pooled: invoke}), []string{"comment", operation, "--type=" + kind}
}
func commentMutationsCapture(t *testing.T, current commentMutationsRow) commentMutationsRow {
	t.Helper()
	current.Paths, current.Queries, current.Forms, current.Committed = []string{}, []url.Values{}, []url.Values{}, []bool{}
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
	command, prefix := commentMutationsCommand(current.Operation, current.Kind, reader, out, &diagnostics, func(value *bool) (bool, error) {
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
				if req.Method != "POST" || req.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatal("unexpected comment mutation method or credentials")
				}
				if err := req.ParseForm(); err != nil {
					t.Fatal(err)
				}
				current.Paths = append(current.Paths, req.URL.Path)
				current.Forms = append(current.Forms, req.PostForm)
				payload = current.Bodies[0]

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

func commentMutationsCase(operation, kind, mode string, args []string) commentMutationsRow {
	current := commentMutationsRow{commentReadsRow: commentReadsRow{Operation: operation, bookmarkListsRow: bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Identity: 42, Mode: mode, Bodies: []json.RawMessage{json.RawMessage(`{"comment":{"id":123}}`)}}}}
	if operation == "create" || operation == "reply" {
		current.Args = append(current.Args, "--comment=hello<&\n日本語")
	}
	if operation == "reply" {
		current.Args = append(current.Args, "--parent-comment-id=9")
	}
	if operation == "stamp" {
		current.Args = append(current.Args, "--stamp-id=7")
	}
	if mode == "json" {
		current.Args = append(current.Args, "--json")
	}
	if mode == "false" {
		current.Args = append(current.Args, "--json=false")
	}
	return current
}
func TestMigrationCommentMutationsPreservesInputsOutputsAndCommit(t *testing.T) {
	var rows []commentMutationsRow
	for _, operation := range []string{"create", "delete", "reply", "stamp"} {
		for _, kind := range []string{"artwork", "novel"} {
			for _, mode := range []string{"human", "json", "auto", "false", "config_json"} {
				rows = append(rows, commentMutationsCapture(t, commentMutationsCase(operation, kind, mode, []string{"42"})))
			}
		}
		for _, source := range []string{"0", " 42 ", "https://www.pixiv.net/artworks/42", "bad"} {
			rows = append(rows, commentMutationsCapture(t, commentMutationsCase(operation, "artwork", "json", []string{source})))
		}
		for _, kind := range []string{"", "invalid"} {
			c := commentMutationsCase(operation, kind, "json", []string{"42"})
			c.DefaultType = kind == ""
			rows = append(rows, commentMutationsCapture(t, c))
		}
		for _, input := range []string{"42\n", "42\r\n", `{"id":42}` + "\n", ""} {
			c := commentMutationsCase(operation, "novel", "json", nil)
			c.Input = input
			rows = append(rows, commentMutationsCapture(t, c))
		}
		for _, writer := range []string{"other", "broken", "short", "short_success"} {
			c := commentMutationsCase(operation, "artwork", "human", []string{"42"})
			c.Writer = writer
			rows = append(rows, commentMutationsCapture(t, c))
		}
		for _, status := range []int{403, 429, 500} {
			c := commentMutationsCase(operation, "artwork", "json", []string{"42"})
			c.Status = status
			rows = append(rows, commentMutationsCapture(t, c))
		}
		c := commentMutationsCase(operation, "artwork", "json", nil)
		c.ReadError = true
		rows = append(rows, commentMutationsCapture(t, c))
		for _, args := range [][]string{{"42", "43"}, {"42", "--ndjson"}, {"42", "--proxy=http://fixture.invalid:8080", "--no-proxy"}, {"42", "--no-proxy"}, {"42", "--type=novel"}} {
			rows = append(rows, commentMutationsCapture(t, commentMutationsCase(operation, "artwork", "human", args)))
		}
	}
	for _, operation := range []string{"create", "reply", "stamp"} {
		c := commentMutationsCase(operation, "artwork", "json", []string{"0"})
		if operation == "stamp" {
			c.Args = append(c.Args, "--stamp-id=0")
		} else {
			c.Args = append(c.Args, "--comment=")
		}
		rows = append(rows, commentMutationsCapture(t, c))
	}
	c := commentMutationsCase("reply", "artwork", "json", []string{"0"})
	c.Args = append(c.Args, "--parent-comment-id=0")
	rows = append(rows, commentMutationsCapture(t, c))

	for _, args := range [][]string{{"42", "--comment= "}, {"42", "-t", "novel"}, {"42", "--proxy=http://fixture.invalid:8080"}, {"https://www.pixiv.net/novel/show.php?id=42"}, {"42", "--stamp-id=7"}, {"42", "--parent-comment-id=9"}} {
		c := commentMutationsCase("create", "artwork", "json", []string{"42"})
		if args[0] != "42" {
			c.Args[0] = args[0]
		} else {
			c.Args = append(c.Args, args[1:]...)
		}
		rows = append(rows, commentMutationsCapture(t, c))
	}
	for _, writer := range []string{"other", "broken", "short", "short_success"} {
		c := commentMutationsCase("create", "artwork", "json", []string{"42"})
		c.Writer = writer
		rows = append(rows, commentMutationsCapture(t, c))
	}
	recommendedFixture(t, "cli-comment-mutations.json", rows, *updateCommentMutations)
}
