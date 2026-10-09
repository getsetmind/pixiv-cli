package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	auth "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
)

var updateLoginHiddenStartup = flag.Bool("migration-update-login-hidden-startup", false, "capture isolated hidden login commands and startup hooks")

type migrationHiddenStartupRow struct {
	Name          string   `json:"name"`
	Args          []string `json:"args"`
	Input         string   `json:"input"`
	ReadError     bool     `json:"read_error"`
	Before        *string  `json:"before"`
	ConfigBlocked bool     `json:"config_blocked,omitempty"`
	Supported     bool     `json:"supported"`
	CleanupError  string   `json:"cleanup_error"`
	EnsureError   string   `json:"ensure_error"`
	CallbackKind  string   `json:"callback_kind"`
	OpenError     bool     `json:"open_error"`
	ClearError    bool     `json:"clear_error"`
	RemoteBody    string   `json:"remote_body"`
	Exit          int      `json:"exit"`
	Stdout        string   `json:"stdout"`
	Stderr        string   `json:"stderr"`
	Calls         []string `json:"calls"`
	StdinReads    int      `json:"stdin_reads"`
	Config        bool     `json:"config"`
	Database      bool     `json:"database"`
	After         string   `json:"after"`
	ActiveRemote  bool     `json:"active_remote"`
}

type migrationHiddenReader struct {
	input io.Reader
	reads *int
	fail  bool
}

func (r migrationHiddenReader) Read(body []byte) (int, error) {
	*r.reads++
	if r.fail {
		return 0, errors.New("synthetic stdin must not be read")
	}
	return r.input.Read(body)
}

type migrationHiddenTransport func(*http.Request) (*http.Response, error)

func (f migrationHiddenTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationHiddenBody struct {
	io.Reader
	calls *[]string
}

func (b migrationHiddenBody) Read(p []byte) (int, error) {
	*b.calls = append(*b.calls, "read")
	return b.Reader.Read(p)
}
func (b migrationHiddenBody) Close() error { *b.calls = append(*b.calls, "close"); return nil }

func TestMigrationLoginHiddenStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_HIDDEN_STARTUP_CHILD")
	if encoded == "" {
		t.Skip("isolated hidden login startup helper")
	}
	var row migrationHiddenStartupRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	row.Calls = []string{}
	oldCleanup, oldSupported, oldEnsure := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay
	t.Cleanup(func() {
		cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay = oldCleanup, oldSupported, oldEnsure
	})
	cleanupPendingWindowsUpdate = func() error {
		row.Calls = append(row.Calls, "cleanup")
		if row.CleanupError != "" {
			return errors.New(row.CleanupError)
		}
		return nil
	}
	automaticPersistentHandlerSupported = func() bool { row.Calls = append(row.Calls, "supported"); return row.Supported }
	ensureURLSchemeRelay = func(context.Context) error {
		row.Calls = append(row.Calls, "ensure")
		if row.EnsureError != "" {
			return errors.New(row.EnsureError)
		}
		return nil
	}
	t.Cleanup(loginhelper.SetDelegateToPreviousForHandler(func(_ context.Context, raw string) error { row.Calls = append(row.Calls, "delegate:"+raw); return nil }))
	t.Cleanup(auth.SetOpenBrowser(func(raw string) error {
		row.Calls = append(row.Calls, "open:"+raw)
		if row.OpenError {
			return errors.New("private browser failure /private/path?secret=synthetic")
		}
		return nil
	}))
	t.Cleanup(auth.SetClearRemoteLoginHandoffForHandler(func(start loginhelper.RemoteLoginStart) error {
		row.Calls = append(row.Calls, "clear")
		if row.ClearError {
			return errors.New("private clear failure")
		}
		return loginhelper.ClearRemoteLoginHandoff(start)
	}))
	const authorizationURL = "https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic&state=synthetic"
	t.Cleanup(loginhelper.SetHandoffHTTPClient(&http.Client{Transport: migrationHiddenTransport(func(r *http.Request) (*http.Response, error) {
		if r.Method != "POST" || r.URL.Scheme != "https" || r.URL.Host != "relay.example" || r.URL.RawQuery != "" {
			t.Fatalf("unexpected handoff route: %s %s", r.Method, r.URL)
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Fatal(err)
		}
		var request map[string]string
		if err := json.Unmarshal(body, &request); err != nil {
			t.Fatal(err)
		}
		if request["proof"] != "synthetic" {
			t.Fatalf("unexpected proof form: %q", body)
		}
		response := &http.Response{StatusCode: 200, Header: make(http.Header), Request: r}
		switch r.URL.Path {
		case "/start/synthetic":
			row.Calls = append(row.Calls, "handoff-start")
			if len(request) != 1 {
				t.Fatal("start request fields changed")
			}
			response.Body = io.NopCloser(strings.NewReader(`{"authorization_url":"` + authorizationURL + `"}`))
		case "/callback/synthetic":
			row.Calls = append(row.Calls, "handoff-callback")
			if len(request) != 2 || request["callback_url"] != "pixiv://account/login?code=synthetic" {
				t.Fatalf("callback request changed: %q", body)
			}
			response.Header.Set(loginhelper.RelayResultURLHeader, "https://relay.example/result/c3ludGhldGlj")
			body := row.RemoteBody
			if body == "" {
				body = `{"success":true}`
			}
			response.Body = migrationHiddenBody{Reader: strings.NewReader(body), calls: &row.Calls}
		default:
			t.Fatalf("unexpected handoff path %s", r.URL.Path)
		}
		return response, nil
	})}))
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if row.ConfigBlocked {
		if err := os.WriteFile(directory, []byte("synthetic config directory obstruction"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if row.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	switch row.CallbackKind {
	case "local":
		if _, err := loginhelper.WriteCallbackEndpoint("http://127.0.0.1:41871/callback"); err != nil {
			t.Fatal(err)
		}
	case "invalid-endpoint":
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(directory, "url-handler-endpoint"), []byte("https://private.invalid/callback"), 0600); err != nil {
			t.Fatal(err)
		}
	case "remote":
		if err := loginhelper.SaveActiveRemoteLogin(loginhelper.ActiveRemoteLogin{Version: 1, Origin: "https://relay.example", SessionID: "synthetic", Proof: "synthetic"}); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	row.StdinReads = 0
	row.Exit = Run(append([]string{"pixiv"}, row.Args...), migrationHiddenReader{input: strings.NewReader(row.Input), reads: &row.StdinReads, fail: row.ReadError}, &out, &diagnostics)
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = strings.ReplaceAll(diagnostics.String(), home, "<HOME>")
	_, err := os.Stat(path)
	row.Config = err == nil
	_, err = os.Stat(filepath.Join(directory, "pixiv-cli.db"))
	row.Database = err == nil
	row.After = ""
	if row.Config {
		body, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(body)
	}
	activePath, err := loginhelper.ActiveRemoteLoginPath()
	if err != nil {
		t.Fatal(err)
	}
	_, err = os.Stat(activePath)
	row.ActiveRemote = err == nil
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), body, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestMigrationLoginHiddenStartup(t *testing.T) {
	path := "../../crates/pixiv-cli/tests/fixtures/cli_login_hidden_startup.json"
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var fixture map[string]json.RawMessage
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	var rows []migrationHiddenStartupRow
	if err := json.Unmarshal(fixture["cli"], &rows); err != nil {
		t.Fatal(err)
	}
	actual := make([]migrationHiddenStartupRow, len(rows))
	for index, row := range rows {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			data, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			child := exec.Command(os.Args[0], "-test.run=^TestMigrationLoginHiddenStartupChild$")
			for _, entry := range os.Environ() {
				key, _, _ := strings.Cut(entry, "=")
				switch key {
				case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY":
					continue
				}
				child.Env = append(child.Env, entry)
			}
			child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_HIDDEN_STARTUP_CHILD="+string(data))
			if out, err := child.CombinedOutput(); err != nil {
				t.Fatalf("child: %v\n%s", err, out)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &actual[index]); err != nil {
				t.Fatal(err)
			}
			if !*updateLoginHiddenStartup && !reflect.DeepEqual(row, actual[index]) {
				t.Fatalf("hidden login/startup contract changed\ngot: %s\nwant: %s", result, data)
			}
		})
	}
	if t.Failed() {
		return
	}
	if *updateLoginHiddenStartup {
		fixture["cli"], err = json.Marshal(actual)
		if err != nil {
			t.Fatal(err)
		}
		body, err = json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
	}
}
