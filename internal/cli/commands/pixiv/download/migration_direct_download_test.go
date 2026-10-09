package download_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	download "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/download"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateDirectDownload = flag.Bool("migration-update-direct-download", false, "capture direct download CLI contracts from the fixed Go reference")

type directDownloadCase struct {
	Name           string                  `json:"name"`
	Args           []string                `json:"args"`
	Input          string                  `json:"input"`
	RuntimeJSON    bool                    `json:"runtime_json"`
	RuntimePath    string                  `json:"runtime_path"`
	Failure        string                  `json:"failure"`
	Setup          map[string]string       `json:"setup"`
	ProxyOverrides []string                `json:"proxy_overrides"`
	WriterFailure  string                  `json:"writer_failure"`
	CancelBefore   bool                    `json:"cancel_before"`
	Stdout         string                  `json:"stdout"`
	Stderr         string                  `json:"stderr"`
	Error          string                  `json:"error"`
	Cause          string                  `json:"cause"`
	Pipeline       bool                    `json:"pipeline"`
	Events         []string                `json:"events"`
	Committed      []bool                  `json:"committed"`
	Requests       []directDownloadRequest `json:"requests"`
	Files          map[string]string       `json:"files"`
	TemporaryFiles int                     `json:"temporary_files"`
}
type directDownloadRequest struct {
	Method        string `json:"method"`
	URL           string `json:"url"`
	Referer       string `json:"referer"`
	Authorization string `json:"authorization"`
	Cookie        string `json:"cookie"`
}
type directDownloadTransport func(*http.Request) (*http.Response, error)

func (f directDownloadTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type directDownloadBody struct {
	reader *strings.Reader
	fail   string
	cancel context.CancelFunc
}

func (b *directDownloadBody) Read(p []byte) (int, error) {
	if b.reader.Len() > 0 {
		return b.reader.Read(p)
	}
	switch b.fail {
	case "read":
		return 0, errors.New("fixture body failure")
	case "cancel":
		b.cancel()
		return 0, context.Canceled
	}
	return 0, io.EOF
}
func (b *directDownloadBody) Close() error { return nil }

type directDownloadFailWriter struct{}

func (directDownloadFailWriter) Write([]byte) (int, error) {
	return 0, errors.New("fixture writer failure")
}

func TestMigrationDirectDownloadMatchesFrozenCLIContracts(t *testing.T) {
	const first = "https://i.pximg.net/assets/one.png?signature=private"
	const second = "https://i.pximg.net/assets/two.png"
	ref, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"ugoira_archive","id":42,"p":-1,"v":"original"}`))
	if err != nil {
		t.Fatal(err)
	}
	rows := []directDownloadCase{}
	add := func(name string, args ...string) int {
		rows = append(rows, directDownloadCase{Name: name, Args: args, RuntimePath: "$ROOT/runtime"})
		return len(rows) - 1
	}
	add("plain", first)
	add("json", first, "--json")
	add("ndjson", first, second, "--ndjson")
	i := add("runtime-json", first)
	rows[i].RuntimeJSON = true
	i = add("explicit-json-false", first, "--json=false")
	rows[i].RuntimeJSON = true
	add("fail-fast-json-success", first, "--json", "--on-error=fail-fast")
	add("fail-fast-ndjson-success", first, "--ndjson", "--on-error=fail-fast")
	add("output-alias", first, "-o", "$ROOT/override", "--json")
	add("download-path", first, "--download-path", "$ROOT/override", "--json")
	add("same-path-aliases", first, "-o", "$ROOT/override", "--download-path", "$ROOT/override", "--json")
	add("conflicting-path-aliases", first, "-o", "$ROOT/override", "--download-path", "$ROOT/other")
	add("proxy-conflict", first, "--proxy=http://fixture.invalid:1", "--no-proxy=false")
	add("proxy-override", first, "--proxy=http://fixture.invalid:1", "--json")
	add("no-proxy-override", first, "--no-proxy", "--json")
	add("output-conflict", first, "--json=false", "--ndjson=false")
	add("invalid-on-error-first", first, "--on-error=bad", "--pages=bad", "--quality=bad")
	add("invalid-pages-first", first, "--pages=bad", "--quality=bad", "--ugoira-mode=bad")
	add("invalid-quality-first", first, "--quality=bad", "--ugoira-mode=bad")
	add("invalid-ugoira-mode", first, "--ugoira-mode=bad")
	add("unknown-flag", first, "--concurrency=1")
	add("help", "--help")
	for _, quality := range []string{"original", "regular", "small", "thumb", "mini", ""} {
		add("validated-direct-quality-"+quality, first, "--quality="+quality, "--json")
	}
	for _, mode := range []string{"gif", "apng", "zip", "raw", ""} {
		add("validated-direct-ugoira-mode-"+mode, first, "--ugoira-mode="+mode, "--json")
	}
	add("direct-planning-options-ignored", first, "--pages=1,3-5", "--filename-template={bad}", "--json")
	add("opaque-reference", ref.String(), "--json")
	add("opaque-before-direct", first, ref.String(), "--ndjson")
	add("duplicate-direct", first, first, "--json")
	add("invalid-source-json", "unrecognized signed=private", "--json")
	add("forbidden-source-json", "https://example.invalid/one.png?signature=private", "--json")
	add("no-basename-json", "https://i.pximg.net/", "--json")
	add("sanitized-basename", "https://i.pximg.net/a%2Fb%3Ac%00.png?x=secret", "--json")
	i = add("empty-runtime-path", first, "--json")
	rows[i].RuntimePath = ""
	i = add("stdin-text", "--json")
	rows[i].Input = first + "\r\n"
	i = add("explicit-source-ignores-stdin", first, "--json")
	rows[i].Input = "{invalid input"
	i = add("stdin-resource-record", "--json")
	rows[i].Input = `{"id":"1","type":"resource","url":"` + first + `"}` + "\n"
	i = add("stdin-malformed-record", "--json")
	rows[i].Input = "{broken}\n"
	i = add("stdin-record-skip", "--json")
	rows[i].Input = "{broken}\n" + `{"id":"1","type":"resource","url":"` + first + `"}` + "\n"
	i = add("stdin-record-fail-fast", "--json", "--on-error=fail-fast")
	rows[i].Input = "{broken}\n" + `{"id":"1","type":"resource","url":"` + first + `"}` + "\n"
	i = add("record-mode-ignores-output-conflict", "--json", "--ndjson")
	rows[i].Input = "{broken}\n"
	add("empty-input")
	i = add("overwrite", first, "--json")
	rows[i].Setup = map[string]string{"runtime/one-b011ca290fdf.png": "previous"}
	i = add("failed-overwrite", second, "--json")
	rows[i].Failure = "read"
	rows[i].Setup = map[string]string{"runtime/two-e1b50ce4fa8e.png": "previous"}
	i = add("status-without-commit", second)
	rows[i].Failure = "status"
	i = add("cancel-without-commit", second, "--json")
	rows[i].Failure = "cancel"
	i = add("fail-fast-keeps-dispatching", first, second, "https://i.pximg.net/third.png", "--json", "--on-error=fail-fast")
	rows[i].Failure = "status"
	for _, mode := range []string{"plain", "json", "ndjson", "fail-fast"} {
		args := []string{first, second}
		if mode == "json" {
			args = append(args, "--json")
		}
		if mode == "ndjson" {
			args = append(args, "--ndjson")
		}
		if mode == "fail-fast" {
			args = append(args, "--json", "--on-error=fail-fast")
		}
		i = add("partial-status-"+mode, args...)
		rows[i].Failure = "status"
	}
	i = add("partial-body", first, second, "--json")
	rows[i].Failure = "read"
	i = add("cancel-after-commit", first, second, "--json")
	rows[i].Failure = "cancel"
	i = add("cancel-before-attempt", first, "--json")
	rows[i].CancelBefore = true
	i = add("stdout-failure", first, "--json")
	rows[i].WriterFailure = "stdout"
	i = add("record-stderr-failure", "--json")
	rows[i].Input = "{broken}\n"
	rows[i].WriterFailure = "stderr"
	for n := range rows {
		row := &rows[n]
		root := t.TempDir()
		ctx, cancel := context.WithCancel(context.Background())
		row.Events = []string{}
		row.Committed = []bool{}
		row.Requests = []directDownloadRequest{}
		row.Files = map[string]string{}
		row.ProxyOverrides = []string{}
		for name, body := range row.Setup {
			dest := filepath.Join(root, filepath.FromSlash(name))
			if err := os.MkdirAll(filepath.Dir(dest), 0o700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(dest, []byte(body), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		replace := func(s string) string { return strings.ReplaceAll(s, "$ROOT", root) }
		normalize := func(s string) string { return strings.ReplaceAll(s, root, "$ROOT") }
		client, clientErr := pixiv.NewWith("fixture-token", pixiv.Options{HTTPClient: &http.Client{Transport: directDownloadTransport(func(req *http.Request) (*http.Response, error) {
			row.Requests = append(row.Requests, directDownloadRequest{req.Method, req.URL.String(), req.Header.Get("Referer"), req.Header.Get("Authorization"), req.Header.Get("Cookie")})
			if req.URL.Host == "app-api.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/archive.zip"},"frames":[{"file":"0.jpg","delay":10}]}}`)), Request: req}, nil
			}
			if req.URL.Host != "i.pximg.net" {
				return nil, fmt.Errorf("unexpected fixture host")
			}
			failure := ""
			status := 200
			if req.URL.String() == second {
				failure = row.Failure
				if failure == "status" {
					status = 404
					failure = ""
				}
			}
			return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"image/png"}, "Content-Length": {"7"}}, Body: &directDownloadBody{strings.NewReader("payload"), failure, cancel}, Request: req}, nil
		})}})
		if clientErr != nil {
			t.Fatal(clientErr)
		}
		var stdout, stderr bytes.Buffer
		var out io.Writer = &stdout
		var errOut io.Writer = &stderr
		if row.WriterFailure == "stdout" {
			out = directDownloadFailWriter{}
		}
		if row.WriterFailure == "stderr" {
			errOut = directDownloadFailWriter{}
		}
		cmd := download.New(download.Deps{Input: strings.NewReader(row.Input), Output: out, ErrorOutput: errOut, UsageError: func(err error) error { row.Events = append(row.Events, "usage"); return err },
			JSONOut: func(override *bool) (bool, error) {
				row.Events = append(row.Events, "json-mode")
				if override != nil {
					return *override, nil
				}
				return row.RuntimeJSON, nil
			},
			Runtime: func() (download.Runtime, error) {
				row.Events = append(row.Events, "runtime")
				return download.Runtime{DownloadPath: replace(row.RuntimePath), FilenameTemplate: "{author} - {title}_{id}"}, nil
			},
			Download: func() downloader.DownloadService {
				row.Events = append(row.Events, "service")
				return downloader.DownloadService{}
			},
			Pooled: func(ctx context.Context, request download.CommandRequest, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				row.Events = append(row.Events, "pool")
				if request.HTTPSProxyOverride == nil {
					row.ProxyOverrides = append(row.ProxyOverrides, "<unset>")
				} else {
					row.ProxyOverrides = append(row.ProxyOverrides, *request.HTTPSProxyOverride)
				}
				committed, err := invoke(ctx, client)
				row.Committed = append(row.Committed, committed)
				return err
			},
		})
		cmd.SetOut(out)
		cmd.SetErr(errOut)
		cmd.SilenceErrors = true
		cmd.SilenceUsage = true
		cmd.SetContext(ctx)
		args := make([]string, len(row.Args))
		for j, arg := range row.Args {
			args[j] = replace(arg)
		}
		cmd.SetArgs(args)
		if row.CancelBefore {
			cancel()
		}
		resultErr := cmd.Execute()
		if resultErr != nil {
			row.Error = normalize(resultErr.Error())
			if errors.Is(resultErr, context.Canceled) {
				row.Cause = "cancel"
			}
			row.Pipeline = row.Error == "pipeline records failed"
		}
		row.Stdout = normalize(stdout.String())
		row.Stderr = normalize(stderr.String())
		walkErr := filepath.WalkDir(root, func(p string, e os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if !e.IsDir() {
				if strings.HasPrefix(e.Name(), ".atomic-write-") {
					row.TemporaryFiles++
				}
				body, err := os.ReadFile(p)
				if err != nil {
					return err
				}
				rel, err := filepath.Rel(root, p)
				if err != nil {
					return err
				}
				row.Files[filepath.ToSlash(rel)] = string(body)
			}
			return nil
		})
		if walkErr != nil {
			t.Fatal(walkErr)
		}
		client.CloseIdleConnections()
		cancel()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "..", "crates", "pixiv-cli", "tests", "fixtures", "download_direct.json")
	if *migrationUpdateDirectDownload {
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
		t.Fatal("direct download CLI differs from fixed Go reference")
	}
}
