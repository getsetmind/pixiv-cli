package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	authcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	pixivaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
)

var updateDictionaryStartup = flag.Bool("migration-update-dictionary-startup", false, "capture isolated anonymous dictionary root startup contracts")

const dictionaryStartupReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type dictionaryStartupRequest struct {
	Method        string `json:"method"`
	Host          string `json:"host"`
	URI           string `json:"uri"`
	Accept        string `json:"accept"`
	UserAgent     string `json:"user_agent"`
	Authorization string `json:"authorization"`
	Cookie        string `json:"cookie"`
}
type dictionaryStartupRow struct {
	Name              string                     `json:"name"`
	Comparison        string                     `json:"comparison"`
	Args              []string                   `json:"args"`
	Input             string                     `json:"input"`
	Before            *string                    `json:"before"`
	Env               map[string]string          `json:"env"`
	Transport         string                     `json:"transport"`
	CorruptAfterFetch bool                       `json:"corrupt_after_fetch"`
	BuildVersion      string                     `json:"build_version"`
	Supported         bool                       `json:"supported"`
	CleanupError      string                     `json:"cleanup_error"`
	EnsureError       string                     `json:"ensure_error"`
	Exit              int                        `json:"exit"`
	Stdout            string                     `json:"stdout"`
	Stderr            string                     `json:"stderr"`
	Calls             []string                   `json:"calls"`
	Requests          []dictionaryStartupRequest `json:"requests"`
	StdinReads        int                        `json:"stdin_reads"`
	ExternalDials     int32                      `json:"external_dials"`
	AccountCalls      int                        `json:"account_calls"`
	Config            bool                       `json:"config"`
	Database          bool                       `json:"database"`
	After             string                     `json:"after"`
}
type dictionaryStartupFixture struct {
	Reference string                 `json:"reference"`
	Sources   map[string]string      `json:"sources"`
	Gaps      []string               `json:"gaps"`
	Rows      []dictionaryStartupRow `json:"rows"`
}
type dictionaryStartupReader struct{ reads *int }

func (reader dictionaryStartupReader) Read([]byte) (int, error) {
	*reader.reads++
	return 0, errors.New("dictionary must not read stdin")
}

func TestMigrationDictionaryStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_DICTIONARY_STARTUP_CHILD")
	if encoded == "" {
		t.Skip("isolated dictionary root helper")
	}
	var row dictionaryStartupRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if row.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	row.Calls = []string{}
	row.Requests = []dictionaryStartupRequest{}
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
	newCLIAutomaticUpdateChecker = func(proxy string) (*update.AutomaticUpdateChecker, error) {
		row.Calls = append(row.Calls, "automatic-update:"+proxy)
		return nil, errors.New("synthetic automatic update must not execute")
	}
	newCLIPixivSDKPorts = func(app) (pixivSDKPorts, error) {
		row.AccountCalls++
		return pixivSDKPorts{}, errors.New("dictionary must not acquire an account")
	}
	newCLIAccountServices = func(app) (authcommands.AccountService, pixivaccount.LoginService, error) {
		row.AccountCalls++
		return authcommands.AccountService{}, pixivaccount.LoginService{}, errors.New("dictionary must not acquire OAuth services")
	}
	if row.BuildVersion != "" {
		buildinfo.Version = row.BuildVersion
	}
	oldTransport := http.DefaultTransport
	transport := oldTransport.(*http.Transport).Clone()
	var externalDials atomic.Int32
	var requestLock sync.Mutex
	appendRequest := func(request *http.Request) {
		requestLock.Lock()
		defer requestLock.Unlock()
		row.Requests = append(row.Requests, dictionaryStartupRequest{Method: request.Method, Host: request.Host, URI: request.RequestURI, Accept: request.Header.Get("Accept"), UserAgent: request.UserAgent(), Authorization: request.Header.Get("Authorization"), Cookie: request.Header.Get("Cookie")})
	}
	var localAddress string
	if row.Transport != "" {
		server := httptest.NewServer(http.HandlerFunc(func(out http.ResponseWriter, request *http.Request) {
			appendRequest(request)
			if row.Transport == "proxy-error" {
				out.WriteHeader(http.StatusServiceUnavailable)
				return
			}
			if row.CorruptAfterFetch {
				if err := os.WriteFile(path, []byte("[unfinished\n"), 0600); err != nil {
					t.Error(err)
				}
			}
			if strings.HasPrefix(request.URL.Path, "/_api/get_article/") {
				out.Header().Set("Content-Type", "application/json")
				fmt.Fprint(out, `{"id":42,"tagName":"captured title","abstract":"captured abstract","nodes":"[]"}`)
			} else if request.URL.Path == "/search" {
				fmt.Fprint(out, `<div id="main"><article><div class="info"><a href="/a/captured">captured title</a><p class="summary">captured summary</p></div></article></div>`)
			} else {
				t.Errorf("unexpected dictionary root request: %s", request.RequestURI)
				out.WriteHeader(500)
			}
		}))
		t.Cleanup(server.Close)
		parsed, err := url.Parse(server.URL)
		if err != nil {
			t.Fatal(err)
		}
		localAddress = parsed.Host
		if row.Transport == "proxy-error" {
			if err := os.Setenv("HTTPS_PROXY", server.URL); err != nil {
				t.Fatal(err)
			}
		} else {
			transport.Proxy = nil
		}
	}
	dial := func(ctx context.Context, network, address string) (net.Conn, error) {
		if row.Transport == "proxy-error" && address == localAddress {
			return (&net.Dialer{}).DialContext(ctx, network, address)
		}
		externalDials.Add(1)
		return nil, errors.New("external dictionary network is denied")
	}
	transport.DialContext = dial
	transport.DialTLSContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		if row.Transport == "local-success" && address == "dic.pixiv.net:443" {
			return (&net.Dialer{}).DialContext(ctx, network, localAddress)
		}
		return dial(ctx, network, address)
	}
	http.DefaultTransport = transport
	t.Cleanup(func() { http.DefaultTransport = oldTransport; transport.CloseIdleConnections() })
	var out, diagnostics bytes.Buffer
	row.Exit = Run(append([]string{"pixiv"}, row.Args...), dictionaryStartupReader{reads: &row.StdinReads}, &out, &diagnostics)
	row.ExternalDials = externalDials.Load()
	if row.ExternalDials != 0 || row.StdinReads != 0 || row.AccountCalls != 0 {
		t.Fatalf("anonymous dictionary touched external=%d stdin=%d accounts=%d", row.ExternalDials, row.StdinReads, row.AccountCalls)
	}
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = dictionaryStartupDiagnostics(t, strings.ReplaceAll(diagnostics.String(), home, "<HOME>"))
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	row.Config = exists("config.toml")
	row.Database = exists("pixiv-cli.db") || exists("pixiv-cli.db-wal") || exists("pixiv-cli.db-shm")
	if row.Database {
		t.Fatal("anonymous dictionary created an account database")
	}
	if row.Config {
		body, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(body)
	}
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), body, 0600); err != nil {
		t.Fatal(err)
	}
}

func dictionaryStartupDiagnostics(t *testing.T, text string) string {
	t.Helper()
	lines := strings.Split(text, "\n")
	for index, line := range lines {
		var record map[string]json.RawMessage
		if json.Unmarshal([]byte(line), &record) != nil || record["time"] == nil {
			continue
		}
		var timestamp string
		if err := json.Unmarshal(record["time"], &timestamp); err != nil {
			t.Fatal(err)
		}
		if _, err := time.Parse(time.RFC3339Nano, timestamp); err != nil {
			t.Fatal(err)
		}
		lines[index] = strings.Replace(line, string(record["time"]), `"<TIME>"`, 1)
	}
	return strings.Join(lines, "\n")
}

func dictionaryStartupSources(t *testing.T) map[string]string {
	t.Helper()
	files := []string{
		"internal/cli/root.go", "internal/cli/execution.go", "internal/cli/composition.go", "internal/cli/commands/lifecycle.go",
		"internal/cli/commands/pixiv/dic/dic.go", "internal/cli/commands/pixiv/dic/article.go", "internal/cli/commands/pixiv/dic/search.go",
		"internal/services/dic/dic.go", "internal/services/dic/errors.go", "internal/services/dic/article.go", "internal/services/dic/search.go", "internal/services/dic/transport.go",
		"internal/cli/commands/update/automatic_check.go", "internal/cli/diagnostics/diagnostics.go", "internal/shared/buildinfo/buildinfo.go",
		"internal/config/settings/config.go", "internal/config/settings/defaults.go", "internal/config/settings/store.go", "internal/config/settings/schema.go", "internal/config/settings/snapshot.go", "internal/config/settings/values.go", "internal/config/settings/paths.go",
	}
	sources := map[string]string{}
	for _, file := range files {
		frozen, err := exec.Command("git", "show", dictionaryStartupReference+":"+file).Output()
		if err != nil {
			t.Fatal(err)
		}
		current, err := os.ReadFile(filepath.Join("../..", file))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(frozen, current) {
			t.Fatalf("dictionary root source differs from frozen Go: %s", file)
		}
		sources[file] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	return sources
}

func TestMigrationDictionaryStartup(t *testing.T) {
	sources := dictionaryStartupSources(t)
	rows := []dictionaryStartupRow{}
	add := func(name string, args ...string) *dictionaryStartupRow {
		rows = append(rows, dictionaryStartupRow{Name: name, Comparison: "exact", Args: args, Env: map[string]string{}})
		return &rows[len(rows)-1]
	}
	text := func(value string) *string { return &value }
	malformed := "[unfinished\n"
	add("group-bare", "dic").Comparison = "startup-state"
	add("group-bare-malformed", "dic").Before = text(malformed)
	add("group-extra", "dic", "extra")
	add("group-extra-before-config", "dic", "extra").Before = text(malformed)
	add("group-unknown", "dic", "--unknown")
	add("group-json-rejected", "dic", "--json")
	for _, leaf := range []string{"article", "search"} {
		add(leaf+"-missing", "dic", leaf)
		add(leaf+"-missing-piped", "dic", leaf).Input = "captured title\n"
		add(leaf+"-missing-json-false", "dic", leaf, "--json=false")
		add(leaf+"-extra", "dic", leaf, "a", "b")
		add(leaf+"-missing-before-config", "dic", leaf).Before = text(malformed)
		add(leaf+"-extra-before-config", "dic", leaf, "a", "b", "--json").Before = text(malformed)
		add(leaf+"-unknown-before-config", "dic", leaf, "a", "--unknown", "--json").Before = text(malformed)
		add(leaf+"-proxy-rejected", "dic", leaf, "a", "--proxy=http://127.0.0.1:1")
		add(leaf+"-no-proxy-rejected", "dic", leaf, "a", "--no-proxy")
		add(leaf+"-help", "dic", leaf, "--help").Comparison = "startup-state"
		help := add(leaf+"-help-before-config", "dic", leaf, "--help")
		help.Before, help.Comparison = text(malformed), "startup-state"
	}
	add("group-help", "dic", "--help").Comparison = "startup-state"
	help := add("group-help-before-config", "dic", "--help")
	help.Before, help.Comparison = text(malformed), "startup-state"
	add("article-invalid-language", "dic", "article", "a", "--lang=JA", "--json")
	add("article-config-before-language", "dic", "article", "a", "--lang=JA", "--json").Before = text(malformed)
	add("search-invalid-page", "dic", "search", "a", "--page=0", "--limit=-1", "--ndjson", "--json=false")
	add("search-invalid-limit", "dic", "search", "a", "--limit=-1", "--ndjson", "--json=false")
	add("search-output-conflict", "dic", "search", "a", "--ndjson", "--json=false")
	add("search-config-before-page", "dic", "search", "a", "--page=0", "--json").Before = text(malformed)
	add("search-config-before-limit", "dic", "search", "a", "--limit=-1", "--json=false").Before = text(malformed)
	add("search-config-before-conflict", "dic", "search", "a", "--ndjson", "--json=false").Before = text(malformed)
	add("article-ndjson-rejected", "dic", "article", "a", "--ndjson")
	add("article-empty-reference", "dic", "article", "")
	add("article-empty-reference-json-false", "dic", "article", "  ", "--json=false")
	add("article-host-validation", "dic", "article", "https://example.invalid/a/a", "--json")
	add("article-path-validation", "dic", "article", "https://dic.pixiv.net/a/", "--json=false")
	add("search-empty-query", "dic", "search", " \t ")
	add("search-empty-query-ndjson", "dic", "search", "", "--ndjson")
	add("search-ndjson-false-plain", "dic", "search", "", "--ndjson=false")
	add("configured-json-error-is-plain", "dic", "search", "").Before = text("[output]\njson = true\n")
	add("configured-json-explicit-false-envelope", "dic", "search", "", "--json=false").Before = text("[output]\njson = true\n")
	add("config-negative-interval-before-query", "dic", "search", "", "--json").Before = text("[network]\nrequest_interval = '-1s'\n")
	add("env-invalid-interval-before-language", "dic", "article", "a", "--lang=JA", "--json=false").Env["PIXIV_REQUEST_INTERVAL"] = "later"
	add("custom-config-preserved", "dic", "search", "").Before = text("# mine\r\n[unknown]\r\nkeep = true\r\n")
	add("configured-proxy-does-not-block-reference", "dic", "article", "").Before = text("[network]\nhttps_proxy = 'invalid'\n")
	add("undeclared-json-env-is-plain", "dic", "search", "").Env["PIXIV_OUTPUT_JSON"] = "true"
	for _, leaf := range []string{"article", "search"} {
		for _, mode := range []string{"human", "json", "json-false"} {
			args := []string{"dic", leaf, "captured"}
			if mode == "json" {
				args = append(args, "--json")
			}
			if mode == "json-false" {
				args = append(args, "--json=false")
			}
			row := add(leaf+"-anonymous-proxy-error-"+mode, args...)
			row.Transport = "proxy-error"
		}
	}
	row := add("search-anonymous-proxy-error-ndjson", "dic", "search", "captured", "--ndjson")
	row.Transport = "proxy-error"
	row = add("configured-proxy-not-dictionary-proxy", "dic", "article", "captured", "--json")
	row.Transport, row.Before = "proxy-error", text("[network]\nhttps_proxy = 'invalid'\n")
	row = add("article-explicit-reference-ignores-stdin", "dic", "article", "captured", "--json")
	row.Transport, row.Input = "proxy-error", "other reference\n"
	for _, leaf := range []string{"article", "search"} {
		for _, mode := range []string{"human", "json", "json-false", "resolver-after-fetch"} {
			args := []string{"dic", leaf, "captured"}
			if leaf == "article" {
				args = append(args, "--no-counters")
			}
			if mode == "json" {
				args = append(args, "--json")
			}
			if mode == "json-false" {
				args = append(args, "--json=false")
			}
			row := add(leaf+"-local-success-"+mode, args...)
			row.Transport, row.Comparison = "local-success", "go-only"
			row.CorruptAfterFetch = mode == "resolver-after-fetch" || mode == "json-false"
			if mode == "json-false" {
				row.Before = text("[output]\njson = true\n")
			}
		}
	}
	row = add("search-local-success-ndjson-skips-resolver", "dic", "search", "captured", "--ndjson")
	row.Transport, row.Comparison, row.CorruptAfterFetch = "local-success", "go-only", true
	row = add("search-local-success-ndjson-false-uses-resolver", "dic", "search", "captured", "--ndjson=false")
	row.Transport, row.Comparison, row.CorruptAfterFetch = "local-success", "go-only", true
	row = add("cleanup-failure-before-config", "dic", "search", "captured", "--json")
	row.CleanupError, row.Before, row.Comparison = "synthetic cleanup failure", text(malformed), "go-only"
	row = add("startup-handler-warning", "dic", "search", "", "--json=false")
	row.Supported, row.EnsureError, row.Comparison = true, "private synthetic handler failure /private/path", "go-only"
	row = add("startup-handler-success", "dic", "article", "")
	row.Supported, row.Comparison = true, "go-only"
	row = add("debug-root-identity", "dic", "search", "", "--json")
	row.Before, row.Comparison = text("[logging]\nlevel = 'debug'\nformat = 'json'\n"), "go-only"
	row = add("release-group-skips-update", "dic")
	row.BuildVersion, row.Comparison = "v1.0.0", "go-only"
	row = add("release-leaf-invokes-synthetic-update", "dic", "search", "captured", "--ndjson")
	row.Transport, row.BuildVersion, row.Comparison, row.Before = "local-success", "v1.0.0", "go-only", text("[network]\nhttps_proxy = 'configured-update-proxy'\n")
	row = add("release-disabled-update", "dic", "search", "captured", "--ndjson")
	row.Transport, row.BuildVersion, row.Comparison, row.Before = "local-success", "v1.0.0", "go-only", text("[update]\ncheck_enabled = false\n")
	actual := make([]dictionaryStartupRow, len(rows))
	for index, row := range rows {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			temp := filepath.Join(home, "tmp")
			if err := os.Mkdir(temp, 0700); err != nil {
				t.Fatal(err)
			}
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationDictionaryStartupChild$")
			child.Dir = home
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "TMPDIR=" + temp, "PATH=" + home, "MIGRATION_DICTIONARY_STARTUP_CHILD=" + string(body)}
			for key, value := range row.Env {
				child.Env = append(child.Env, key+"="+value)
			}
			if output, err := child.CombinedOutput(); err != nil {
				t.Fatalf("owned dictionary root child: %v\n%s", err, output)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &actual[index]); err != nil {
				t.Fatal(err)
			}
		})
	}
	if t.Failed() {
		return
	}
	fixture := dictionaryStartupFixture{Reference: dictionaryStartupReference, Sources: sources, Rows: actual, Gaps: []string{
		"Cobra help bytes are captured; Rust process help compares startup state rather than Clap/Cobra raw bytes.",
		"Synthetic native startup hooks, stable-version automatic-update hook policy and clock-normalized debug root identity are Go-only observations.",
		"Go-only successful root fetches route the genuine HTTP transport's TLS dial to a loopback plaintext server; they establish anonymous root/output-resolver order, not live HTTPS or Rust subprocess TLS success.",
		"Rust and Go subprocess transport-error comparisons use finite loopback CONNECT failures; no external DNS/HTTPS, accounts, OAuth, database, native registration or actual update request occurs.",
		"Piped stdout remains human unless an output flag/config selects machine output; native terminal behavior and other platform execution remain outside this fixture.",
	}}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	const path = "../../crates/pixiv-cli/tests/fixtures/cli-dictionary-startup.json"
	if *updateDictionaryStartup {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var want dictionaryStartupFixture
	if err := json.Unmarshal(expected, &want); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(want, fixture) {
		t.Fatalf("dictionary startup contract changed\ngot: %s\nwant: %s", body, expected)
	}
}

var _ io.Reader = dictionaryStartupReader{}
