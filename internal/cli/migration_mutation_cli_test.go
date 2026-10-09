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
	bookmarkcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/bookmark"
	followcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/follow"
	usercmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/user"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateCLIMutations = flag.Bool("migration-update-cli-mutations", false, "capture frozen bookmark and follow CLI contracts")

type migrationMutationRow struct {
	StartupUnverified string       `json:"startup_unverified,omitempty"`
	StartupOnly       bool         `json:"startup_only,omitempty"`
	Args              []string     `json:"args"`
	Input             string       `json:"input"`
	Status            int          `json:"status"`
	Paths             []string     `json:"paths"`
	Forms             []url.Values `json:"forms"`
	Committed         []bool       `json:"committed"`
	Error             string       `json:"error"`
	Stdout            string       `json:"stdout"`
	Stderr            string       `json:"stderr"`
	Exit              int          `json:"exit"`
	StartupStdout     string       `json:"startup_stdout"`
	StartupStderr     string       `json:"startup_stderr"`
	StartupExit       int          `json:"startup_exit"`
	Config            bool         `json:"config"`
	Database          bool         `json:"database"`
}

func TestMigrationBookmarkFollowCLIContracts(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }

	rows := []migrationMutationRow{}
	for _, group := range []string{"bookmark", "follow", "user follow"} {
		for _, action := range []string{"add", "remove"} {
			prefix := append(strings.Fields(group), action)
			extras := [][]string{{"42"}, {"+42"}, {"00042"}, {"0"}, {"-1"}, {"9223372036854775808"}, {"https://www.pixiv.net/artworks/42"}, {"https://www.pixiv.net/users/42"}, {" 42 "}, {"bad"}, {"42", "--on-error=bad"}}
			if action == "add" {
				extras = append(extras, []string{"42", "--restrict=private"}, []string{"42", "--restrict="}, []string{"42", "--restrict=all"})
			}
			if group == "bookmark" {
				extras = append(extras, []string{"42", "--type=novel"}, []string{"42", "--type=all"})
				if action == "add" {
					extras = append(extras, []string{"42", "--type=novel", "--tag=a,b", "--tag=日本語", "--tag="})
				}
			}
			for _, extra := range extras {
				rows = append(rows, migrationMutationRow{Args: append(append([]string{}, prefix...), extra...), Status: 200})
			}
			for _, status := range []int{403, 429, 500} {
				rows = append(rows, migrationMutationRow{Args: append(append([]string{}, prefix...), "42"), Status: status})
			}
			for _, strategy := range []string{"skip", "fail-fast"} {
				for _, typ := range []string{"artwork", "illust", "manga", "ugoira", "novel", "user"} {
					input := `{"id":"42","type":"` + typ + `","url":"anything"}` + "\n" + `{"id":43,"type":"` + typ + `","url":"anything"}`
					rows = append(rows, migrationMutationRow{Args: append(append([]string{}, prefix...), "--on-error="+strategy), Input: input, Status: 200})
				}
			}
			for _, input := range []string{"", "\n", "{", "[]", `{"type":"user","url":"anything"}`, `{"id":"0","type":"user","url":"anything"}`, `{"id":1.5,"type":"user","url":"anything"}`, `{"id":"42","type":null,"url":"anything"}`, `{"id":"42","type":"user","url":""}`} {
				rows = append(rows, migrationMutationRow{Args: append([]string{}, prefix...), Input: input, Status: 200})
			}
		}
	}
	for _, action := range []string{"add", "remove"} {
		for _, strategy := range []string{"skip", "fail-fast"} {
			for _, status := range []int{200, 403} {
				rows = append(rows, migrationMutationRow{Args: []string{"bookmark", action, "--type=novel", "--on-error=" + strategy}, Input: `{"id":"42","type":"novel","url":"anything"}` + "\n" + `{"id":43,"type":"novel","url":"anything"}`, Status: status})
			}
		}
	}
	for _, input := range []string{`{"id":9223372036854775808,"type":"user","url":"anything"}`, `{"id":123456789012345678901234567890,"type":"user","url":"anything"}`, `{"id":"42","type":"user","url":"anything"} trailing`, `{"id":"42","type":"user","url":"anything"}` + "\n" + `{"id":"43","type":"user","url":"anything"}`} {
		for _, strategy := range []string{"skip", "fail-fast"} {
			rows = append(rows, migrationMutationRow{Args: []string{"follow", "add", "--on-error=" + strategy}, Input: input, Status: 403})
		}
	}
	for _, group := range []string{"bookmark", "follow"} {
		for _, input := range []string{"42\n", "42\r\n", "\n\n", " 42 \n", "https://www.pixiv.net/users/42\n", "bad\n", "42\n43\n", "42\n\n", "\n" + `{"id":"42","type":"user","url":"anything"}`, `{"id":"42","type":"user","url":"anything"}` + "\n\n"} {
			rows = append(rows, migrationMutationRow{Args: []string{group, "add"}, Input: input, Status: 200})
		}
	}
	for _, group := range []string{"bookmark", "follow", "user follow"} {
		prefix := strings.Fields(group)
		variants := [][]string{
			append(append(append([]string{}, prefix...), "add", "42"), "--no-proxy"),
			append(append([]string{"--no-proxy"}, prefix...), "add", "42"),
			append(append(append([]string{}, prefix...), "--no-proxy"), "add", "42"),
			append(append(append([]string{}, prefix...), "add", "42"), "--proxy=http://127.0.0.1:9911", "--no-proxy"),
		}
		for _, args := range variants {
			rows = append(rows, migrationMutationRow{Args: args, Status: 200, StartupOnly: true})
		}
	}
	for _, group := range []string{"bookmark", "follow"} {
		for _, input := range []string{"", "0\n", `{"id":"42","type":"novel","url":"anything"}`} {
			rows = append(rows, migrationMutationRow{Args: []string{group, "add", "--proxy=http://127.0.0.1:9911", "--no-proxy"}, Input: input, Status: 200, StartupOnly: true})
		}
	}
	for i := range rows {
		row := &rows[i]
		row.Paths = []string{}
		row.Forms = []url.Values{}
		row.Committed = []bool{}
		var output, diagnostics bytes.Buffer
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "POST" {
				t.Fatalf("mutation method %s", req.Method)
			}
			if err := req.ParseForm(); err != nil {
				return nil, err
			}
			row.Paths = append(row.Paths, req.URL.Path)
			row.Forms = append(row.Forms, req.PostForm)
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader("{}")), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		data := deps.Data{Input: strings.NewReader(row.Input), Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError, Pooled: func(ctx context.Context, _ deps.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			committed, err := invoke(ctx, client)
			row.Committed = append(row.Committed, committed)
			return err
		}}
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(bookmarkcmd.New(data), followcmd.New(data), usercmd.New(usercmd.Dependencies{Input: data.Input, Output: data.Output, UsageError: data.UsageError, Follow: func() *cobra.Command { return followcmd.New(data) }}))
		root.SetOut(&output)
		root.SetErr(&diagnostics)
		root.SetArgs(row.Args)
		err = root.Execute()
		if err != nil {
			row.Error = err.Error()
		}
		row.Exit = (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, false, false)
		row.Stdout = output.String()
		row.Stderr = diagnostics.String()
		home := t.TempDir()
		t.Setenv("HOME", home)
		t.Setenv("USERPROFILE", home)
		t.Setenv("HTTPS_PROXY", "")
		t.Setenv("https_proxy", "")
		t.Setenv("REQUEST_INTERVAL", "0")
		output.Reset()
		diagnostics.Reset()
		row.StartupExit = Run(append([]string{"pixiv"}, row.Args...), strings.NewReader(row.Input), &output, &diagnostics)
		if len(row.Args) > 0 && row.Args[0] == "--no-proxy" {
			row.StartupUnverified = "pre-existing root parser grammar: option before command selection"
		}
		row.StartupStdout = output.String()
		row.StartupStderr = diagnostics.String()
		exists := func(name string) bool {
			_, err := os.Stat(filepath.Join(home, ".pixiv-cli", name))
			if err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			return err == nil
		}
		row.Config, row.Database = exists("config.toml"), exists("pixiv-cli.db")

	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli-bookmark-follow.json")
	if *updateCLIMutations {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("bookmark/follow CLI contract changed")
	}
}

type migrationMutationPartialRead struct{ calls int }

func (r *migrationMutationPartialRead) Read(p []byte) (int, error) {
	r.calls++
	return copy(p, `{"id":"42","type":"user","url":"anything"}`), io.ErrUnexpectedEOF
}

type migrationMutationCountRead struct {
	reader io.Reader
	calls  int
}

func (r *migrationMutationCountRead) Read(p []byte) (int, error) { r.calls++; return r.reader.Read(p) }

func TestMigrationMutationPipelineCancellationAndReadFailures(t *testing.T) {
	for _, scenario := range []string{"cancelled before read", "cancelled during invoke", "partial read failure"} {
		t.Run(scenario, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			input := &migrationMutationCountRead{reader: strings.NewReader(`{"id":"42","type":"user","url":"anything"}` + "\n" + `{"id":"43","type":"user","url":"anything"}` + "\n")}
			var source io.Reader = input
			partial := &migrationMutationPartialRead{}
			if scenario == "partial read failure" {
				source = partial
			}
			if scenario == "cancelled before read" {
				cancel()
			}
			var output, diagnostics bytes.Buffer
			calls := 0
			data := deps.Data{Input: source, Output: &output, ErrorOutput: &diagnostics, UsageError: newUsageError, Pooled: func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
				calls++
				cancel()
				return context.Canceled
			}}
			command := followcmd.New(data)
			command.SetContext(ctx)
			command.SetArgs([]string{"add"})
			command.SilenceErrors = true
			command.SilenceUsage = true
			var err error
			if scenario == "cancelled before read" {
				err = pipeline.ConsumeActionRecords(ctx, source, &diagnostics, "follow_add", "skip", map[string]struct{}{"user": {}}, func(context.Context, int64) error { calls++; return nil }, newUsageError)
			} else {
				err = command.Execute()
			}
			want := "context canceled"
			if scenario == "partial read failure" {
				want = "read NDJSON input: unexpected EOF"
			}
			if err == nil || err.Error() != want {
				t.Fatalf("error=%v want %s", err, want)
			}
			if diagnostics.Len() != 0 || output.Len() != 0 {
				t.Fatalf("output=%q diagnostics=%q", output.String(), diagnostics.String())
			}
			expected := 0
			if scenario == "cancelled during invoke" {
				expected = 1
			}
			if calls != expected {
				t.Fatalf("invocations=%d want %d", calls, expected)
			}
			if scenario == "cancelled before read" && input.calls != 0 {
				t.Fatalf("pre-cancel read calls=%d", input.calls)
			}
		})
	}
}
