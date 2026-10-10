package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	configapp "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var updateRecordDownloadStartup = flag.Bool("migration-update-record-download-startup", false, "capture isolated record download root boundaries")
var recordDownloadNativeBinary = flag.String("migration-record-download-native-binary", "", "replay safe preflight rows with an already built native Go binary")

type migrationRecordDownloadStartupRow struct {
	migrationDownloadStartupRow
	Synthetic       bool      `json:"synthetic"`
	Native          bool      `json:"native"`
	ConfigBlocked   bool      `json:"config_blocked"`
	ReadFailure     string    `json:"read_failure"`
	WriterFailure   string    `json:"writer_failure"`
	WriterCause     string    `json:"writer_cause"`
	Cancel          string    `json:"cancel"`
	PortFailure     string    `json:"port_failure"`
	PoolFailure     string    `json:"pool_failure"`
	ManagerFailure  string    `json:"manager_failure"`
	ResolvedSources []string  `json:"resolved_sources"`
	ResolvedError   string    `json:"resolved_error"`
	StdinBytes      int       `json:"stdin_bytes"`
	PoolCalls       int       `json:"pool_calls"`
	IDs             [][]int64 `json:"ids"`
	WriterCalls     []string  `json:"writer_calls"`
	Signals         []string  `json:"signals"`
}

type migrationRecordDownloadStartupFixture struct {
	PinnedGo    string                              `json:"pinned_go"`
	SourceSHA   map[string]string                   `json:"source_sha"`
	Evidence    string                              `json:"evidence"`
	Limitations []string                            `json:"limitations"`
	Rows        []migrationRecordDownloadStartupRow `json:"rows"`
}

type migrationRecordStartupReader struct {
	row      *migrationRecordDownloadStartupRow
	position int
}

func (r *migrationRecordStartupReader) Read(p []byte) (int, error) {
	r.row.StdinReads++
	if r.row.ReadError {
		return 0, errors.New("synthetic stdin must not be read")
	}
	if r.row.ReadFailure == "classification" || (r.row.ReadFailure == "record-before-line" && r.position > 0) {
		return 0, errors.New("synthetic input failure")
	}
	if r.row.ReadFailure == "zero-once" && r.row.StdinReads == 1 {
		return 0, nil
	}
	if r.position == len(r.row.Input) {
		return 0, io.EOF
	}
	n := copy(p, r.row.Input[r.position:])
	r.position += n
	r.row.StdinBytes += n
	if r.row.ReadFailure == "classification-data" || ((r.row.ReadFailure == "record-tail" || r.row.ReadFailure == "text-tail") && len(p) > 1) {
		return n, errors.New("synthetic input failure")
	}
	return n, nil
}

type migrationRecordStartupWriter struct {
	row    *migrationRecordDownloadStartupRow
	buffer *bytes.Buffer
	stream string
	writes int
}

func (w *migrationRecordStartupWriter) Write(p []byte) (int, error) {
	w.writes++
	w.row.WriterCalls = append(w.row.WriterCalls, w.stream+":"+string(p))
	fail := w.row.WriterFailure == w.stream+"-always" || (w.row.WriterFailure == w.stream+"-once" && w.writes == 1)
	if fail {
		if w.row.WriterCause == "pipe" {
			return 0, syscall.EPIPE
		}
		return 0, errors.New("synthetic diagnostic writer failure")
	}
	return w.buffer.Write(p)
}

type migrationRecordStartupManager func(context.Context, downloader.DownloadRequest) (downloader.DownloadBatchResult, error)

func (f migrationRecordStartupManager) Download(ctx context.Context, request downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	return f(ctx, request)
}

func migrationRecordStartupEnv(home string) []string {
	var env []string
	for _, entry := range os.Environ() {
		key, _, _ := strings.Cut(entry, "=")
		switch key {
		case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY", "PIXIV_ACCESS_TOKEN", "PIXIV_REFRESH_TOKEN", "REQUEST_INTERVAL", "PIXIV_CLIENT_DIR", "PIXIV_CONFIG_DIR", "PIXIV_DATA_DIR", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "MIGRATION_RECORD_DOWNLOAD_STARTUP_CHILD":
			continue
		}
		env = append(env, entry)
	}
	return append(env, "HOME="+home, "USERPROFILE="+home)
}

func migrationRecordStartupConfig(t *testing.T, home string, row migrationRecordDownloadStartupRow) {
	t.Helper()
	if row.Before == nil && !row.ConfigBlocked {
		return
	}
	directory := filepath.Join(home, ".pixiv-cli")
	if err := os.MkdirAll(directory, 0700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(directory, "config.toml")
	if row.ConfigBlocked {
		if err := os.Mkdir(path, 0700); err != nil {
			t.Fatal(err)
		}
	} else if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
		t.Fatal(err)
	}
}

func migrationRecordStartupState(t *testing.T, home string, row *migrationRecordDownloadStartupRow) {
	t.Helper()
	directory := filepath.Join(home, ".pixiv-cli")
	for name, target := range map[string]*bool{"config.toml": &row.Config, "pixiv-cli.db": &row.Database} {
		info, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		*target = err == nil && info.Mode().IsRegular()
	}
	row.After = ""
	if row.Config {
		body, err := os.ReadFile(filepath.Join(directory, "config.toml"))
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(body)
	}
}

func migrationRecordStartupSources(row migrationRecordDownloadStartupRow) ([]string, string) {
	reader := &migrationRecordStartupReader{row: &row}
	root := (app{in: reader, out: io.Discard, errOut: io.Discard, closeState: &closeState{}}).newRootCommand()
	defer pipeline.Clear(root)
	command, args, err := root.Find(row.Args)
	if err != nil {
		return []string{}, err.Error()
	}
	if err := command.ParseFlags(args); err != nil {
		return []string{}, err.Error()
	}
	args = command.Flags().Args()
	if err := command.Args(command, args); err != nil {
		return []string{}, err.Error()
	}
	return append([]string{}, pipeline.ResolvedArgs(command, args)...), ""
}

func TestMigrationRecordDownloadStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_RECORD_DOWNLOAD_STARTUP_CHILD")
	if encoded == "" {
		t.Skip("isolated record download root helper")
	}
	var row migrationRecordDownloadStartupRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	row.ResolvedSources, row.ResolvedError = migrationRecordStartupSources(row)
	migrationRecordStartupConfig(t, home, row)
	row.Calls, row.IDs, row.WriterCalls, row.Signals = []string{}, [][]int64{}, []string{}, []string{}
	oldCleanup, oldSupported, oldEnsure, oldRuntime := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay, loadCLIRuntimeConfig
	oldPorts, oldDownload := newCLIPixivSDKPorts, newCLIDownloadService
	t.Cleanup(func() {
		cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay, loadCLIRuntimeConfig = oldCleanup, oldSupported, oldEnsure, oldRuntime
		newCLIPixivSDKPorts, newCLIDownloadService = oldPorts, oldDownload
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
	loadCLIRuntimeConfig = func() (configapp.RuntimeConfig, error) {
		row.Calls = append(row.Calls, "runtime")
		return oldRuntime()
	}
	oldTransport := http.DefaultTransport
	transport := oldTransport.(*http.Transport).Clone()
	var networkCalls atomic.Int32
	denyDial := func(context.Context, string, string) (net.Conn, error) {
		networkCalls.Add(1)
		return nil, errors.New("synthetic network must not execute")
	}
	transport.DialContext, transport.DialTLSContext = denyDial, denyDial
	http.DefaultTransport = transport
	t.Cleanup(func() { http.DefaultTransport = oldTransport; transport.CloseIdleConnections() })
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if row.Synthetic {
		client, err := pixiv.NewWith("synthetic-root-token", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(client.CloseIdleConnections)
		newCLIPixivSDKPorts = func(app) (pixivSDKPorts, error) {
			row.Calls = append(row.Calls, "ports")
			if row.PortFailure != "" {
				return pixivSDKPorts{}, errors.New(row.PortFailure)
			}
			return pixivSDKPorts{execute: func(ctx context.Context, _ pixivdeps.Request, attempt func(context.Context, *pixiv.Client) (bool, error)) error {
				row.Calls = append(row.Calls, "pool")
				row.PoolCalls++
				if row.Cancel == "during-pool" {
					cancel()
					return errors.New("synthetic canceled action")
				}
				if row.PoolFailure == "fatal" {
					return pipeline.FatalRecordPipeline(errors.New("synthetic fatal pipeline failure"))
				}
				if row.PoolFailure == "fatal-pipe" {
					return pipeline.FatalRecordPipeline(syscall.EPIPE)
				}
				if row.PoolFailure != "" {
					return errors.New(row.PoolFailure)
				}
				_, err := attempt(ctx, client)
				if row.Cancel == "after-pool" {
					cancel()
				}
				return err
			}}, nil
		}
		newCLIDownloadService = func() downloader.DownloadService {
			row.Calls = append(row.Calls, "service")
			return downloader.DownloadService{NewManager: func(downloader.DownloadClient, string, string) (downloader.DownloadManager, error) {
				row.Calls = append(row.Calls, "manager")
				return migrationRecordStartupManager(func(_ context.Context, request downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
					row.Calls = append(row.Calls, "download")
					row.IDs = append(row.IDs, append([]int64(nil), request.IllustIDs...))
					if row.ManagerFailure != "" {
						return downloader.DownloadBatchResult{}, errors.New(row.ManagerFailure)
					}
					return downloader.DownloadBatchResult{}, nil
				}), nil
			}}
		}
	}
	if row.Cancel == "before" {
		cancel()
	}
	var out, diagnostics bytes.Buffer
	stdout := &migrationRecordStartupWriter{row: &row, buffer: &out, stream: "stdout"}
	stderr := &migrationRecordStartupWriter{row: &row, buffer: &diagnostics, stream: "stderr"}
	row.Exit = RunContextWithBrokenPipeSignals(ctx, append([]string{"pixiv"}, row.Args...), &migrationRecordStartupReader{row: &row}, stdout, stderr, func() func() {
		row.Signals = append(row.Signals, "enable")
		return func() { row.Signals = append(row.Signals, "stop") }
	}, nil)
	row.Stdout, row.Stderr = strings.ReplaceAll(out.String(), home, "<HOME>"), strings.ReplaceAll(diagnostics.String(), home, "<HOME>")
	for index := range row.WriterCalls {
		row.WriterCalls[index] = strings.ReplaceAll(row.WriterCalls[index], home, "<HOME>")
	}
	row.NetworkCalls = networkCalls.Load()
	if row.NetworkCalls != 0 {
		t.Fatalf("root attempted %d network connections", row.NetworkCalls)
	}
	migrationRecordStartupState(t, home, &row)
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), body, 0600); err != nil {
		t.Fatal(err)
	}
}

func migrationRecordDownloadStartupRows() []migrationRecordDownloadStartupRow {
	const first = `{"id":"42","type":"artwork","url":"synthetic"}`
	const second = `{"id":"43","type":"artwork","url":"synthetic"}`
	var rows []migrationRecordDownloadStartupRow
	add := func(name, input string, args ...string) *migrationRecordDownloadStartupRow {
		rows = append(rows, migrationRecordDownloadStartupRow{migrationDownloadStartupRow: migrationDownloadStartupRow{Name: name, Input: input, Args: append([]string{"download"}, args...)}, Synthetic: true})
		return &rows[len(rows)-1]
	}
	add("explicit-pid-unread-record-input", first+"\n", "42").ReadError = true
	add("explicit-pids-unread-input", "", "42", "43", "--json=false").ReadError = true
	add("explicit-dash-is-source-and-unread", first+"\n", "-").ReadError = true
	add("ascii-prefix-replayed-physical-lines", " \t\r\n\n"+first+"\n"+second+"\r\n")
	add("record-final-line-without-newline", first)
	add("record-after-ascii-space-prefix", " \t"+first+"\n")
	add("unicode-leading-object-selects-text", "\u00a0"+first+"\n")
	add("ascii-then-unicode-object-selects-text", " \t\u2003"+first+"\n")
	add("text-keeps-multiline-source", "42\n43\n")
	add("text-strips-one-crlf", " 42 \r\n")
	add("text-keeps-extra-line-ending", "42\n\n")
	add("text-lone-cr-not-stripped", "42\r")
	add("empty-input", "").Native = true
	add("one-blank-line-empty-input", "\n").Native = true
	add("blank-crlf-empty-input", "\r\n").Native = true
	add("two-blank-lines-remain-one-text-source", "\n\n")
	add("whitespace-text-source", " \t\r\n")
	add("unicode-whitespace-text-source", "\u00a0\n")
	add("classification-read-error", "", "--json").ReadFailure = "classification"
	add("classification-data-plus-error", " ", "--ndjson").ReadFailure = "classification-data"
	add("text-data-plus-error", "42", "--json").ReadFailure = "text-tail"
	add("classification-zero-once", first+"\n").ReadFailure = "zero-once"
	add("record-read-error-before-action", first+"\n", "--json").ReadFailure = "record-before-line"
	add("record-read-error-after-complete-prefix", first+"\n"+second, "--json").ReadFailure = "record-tail"
	add("record-read-error-human", first+"\n"+second).ReadFailure = "record-tail"
	add("record-read-error-ndjson-false", first, "--ndjson=false").ReadFailure = "record-before-line"
	add("record-read-error-json-false", first, "--json=false").ReadFailure = "record-before-line"
	add("record-success-json-false-and-ndjson-false", first+"\n", "--json=false", "--ndjson=false")
	add("record-success-json-and-ndjson", first+"\n", "--json", "--ndjson")
	jsonConfig := "[output]\njson=true\n"
	add("runtime-json-record-no-stdout", first+"\n").Before = &jsonConfig
	add("record-success-never-writes-stdout", first+"\n", "--ndjson").WriterFailure = "stdout-always"
	add("pipeline-marker-only-diagnostic", "{broken}\n", "--json").Native = true
	add("pipeline-marker-false-flags", "{broken}\n", "--json=false", "--ndjson=false").Native = true
	add("ordinary-action-errors-become-marker", first+"\n"+second+"\n", "--json").PoolFailure = "synthetic pool failure"
	add("cached-port-construction-error-across-records", first+"\n"+second+"\n", "--json").PortFailure = "synthetic port construction failure"
	add("manager-error-fail-fast-marker", first+"\n"+second+"\n", "--on-error=fail-fast", "--json").ManagerFailure = "synthetic operation failure"
	add("fatal-pipeline-human", first+"\n"+second+"\n").PoolFailure = "fatal"
	add("fatal-pipeline-json-envelope", first+"\n", "--json=false").PoolFailure = "fatal"
	add("fatal-pipe-ndjson-success", first+"\n", "--ndjson").PoolFailure = "fatal-pipe"
	add("fatal-pipe-json-failure", first+"\n", "--json").PoolFailure = "fatal-pipe"
	add("diagnostic-writer-once-root-envelope", "{broken}\n", "--json").WriterFailure = "stderr-once"
	add("diagnostic-writer-once-root-human", "{broken}\n").WriterFailure = "stderr-once"
	add("diagnostic-writer-persistent-failure", "{broken}\n", "--json").WriterFailure = "stderr-always"
	pipe := add("diagnostic-writer-pipe-ndjson-success", "{broken}\n", "--ndjson")
	pipe.WriterFailure, pipe.WriterCause = "stderr-once", "pipe"
	add("cancel-before-first-record-envelope", first+"\n", "--json").Cancel = "before"
	add("cancel-after-success-prevents-next-record", first+"\n"+second+"\n", "--json").Cancel = "after-pool"
	add("cancel-during-failed-action-overrides-diagnostic", first+"\n"+second+"\n", "--json").Cancel = "during-pool"
	add("runtime-json-cancellation-human-error", first+"\n").Before = &jsonConfig
	rows[len(rows)-1].Cancel = "before"
	malformed := "[unfinished\n"
	add("config-malformed-before-record-quality-validation", first+"\n", "--quality=bad", "--json").Before = &malformed
	rows[len(rows)-1].Native = true
	add("record-quality-validation-before-actions", first+"\n", "--quality=bad", "--json").Native = true
	add("record-on-error-validation-before-actions", "{broken}\n", "--on-error=bad", "--json").Native = true
	add("record-path-conflict-before-actions", first+"\n", "--output=one", "--download-path=two", "--json").Native = true
	add("record-proxy-presence-conflict-before-actions", first+"\n", "--proxy=http://fixture.invalid:1", "--no-proxy=false", "--json").Native = true
	add("record-cleanup-failure-before-config", first+"\n", "--json").CleanupError = "synthetic cleanup failure"
	warning := add("startup-warning-before-record-diagnostic", "{broken}\n", "--json")
	warning.Supported, warning.EnsureError = true, "private synthetic handler failure /private/path"
	add("config-blocked-before-record-actions", first+"\n", "--json").ConfigBlocked = true
	rows[len(rows)-1].Native = true
	add("natural-no-account-record-marker", first+"\n"+second+"\n", "--json").Native = true
	rows[len(rows)-1].Synthetic = false
	add("natural-no-account-record-output-conflict-bypassed", first+"\n", "--json=false", "--ndjson=false").Native = true
	rows[len(rows)-1].Synthetic = false
	add("natural-no-account-explicit-source-unread", "", "42", "--json").Native = true
	rows[len(rows)-1].Synthetic, rows[len(rows)-1].ReadError = false, true
	return rows
}

func TestMigrationRecordDownloadStartup(t *testing.T) {
	fixture := migrationRecordDownloadStartupFixture{
		PinnedGo: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA: map[string]string{},
		Evidence:    "Actual RunContextWithBrokenPipeSignals root in isolated test-binary children, existing startup/composition seams, synthetic managers/readers/writers/contexts and owned config files; resolved_sources separately observes the actual root command Args codec without startup or action execution. No authenticated SDK/media traffic. Native rows separately replay safe preflight and no-account errors only.",
		Limitations: []string{"Synthetic successful action IDs establish root/controller wiring, not saved SDK/media/account execution or native successful downloads.", "Reader calls and bytes describe these readers and legal bufio prefetch; cancellation does not make an arbitrary blocking Reader interruptible.", "Startup hooks are synthetic; native preflight is Linux only and does not execute browser, association, registry or Windows restore paths."},
	}
	for _, name := range []string{"internal/cli/root.go", "internal/cli/execution.go", "internal/cli/composition.go", "internal/cli/pipeline/pipeline.go", "internal/cli/pipeline/action.go", "internal/cli/pipeline/records.go", "internal/cli/commands/pixiv/download/execution.go"} {
		body, err := os.ReadFile(filepath.Join("../..", name))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(body)
		fixture.SourceSHA[name] = hex.EncodeToString(digest[:])
	}
	for _, input := range migrationRecordDownloadStartupRows() {
		t.Run(input.Name, func(t *testing.T) {
			home := t.TempDir()
			body, err := json.Marshal(input)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationRecordDownloadStartupChild$")
			child.Dir, child.Env = home, append(migrationRecordStartupEnv(home), "MIGRATION_RECORD_DOWNLOAD_STARTUP_CHILD="+string(body))
			if out, err := child.CombinedOutput(); err != nil {
				t.Fatalf("isolated record root child: %v\n%s", err, out)
			}
			body, err = os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			var actual migrationRecordDownloadStartupRow
			if err := json.Unmarshal(body, &actual); err != nil {
				t.Fatal(err)
			}
			if strings.HasPrefix(actual.Name, "record-success-") && (actual.Exit != 0 || actual.Stdout != "" || actual.Stderr != "" || !reflect.DeepEqual(actual.IDs, [][]int64{{42}})) {
				t.Fatalf("successful record did not reach the root's manager without output: %+v", actual)
			}
			fixture.Rows = append(fixture.Rows, actual)
		})
	}
	if t.Failed() {
		return
	}
	if len(fixture.Rows) != 59 {
		t.Fatalf("root row count = %d, want 59", len(fixture.Rows))
	}
	const path = "../../crates/pixiv-cli/tests/fixtures/download_record_startup.json"
	if *updateRecordDownloadStartup {
		body, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected migrationRecordDownloadStartupFixture
	if err := json.Unmarshal(body, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(expected, fixture) {
		got, _ := json.MarshalIndent(fixture, "", "  ")
		t.Fatalf("record download root contract changed\ngot: %s\nwant: %s", got, body)
	}
}

func TestMigrationRecordDownloadNativePreflight(t *testing.T) {
	if *recordDownloadNativeBinary == "" || runtime.GOOS != "linux" {
		t.Skip("safe native preflight requires an explicitly supplied Linux Go binary")
	}
	body, err := os.ReadFile("../../crates/pixiv-cli/tests/fixtures/download_record_startup.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationRecordDownloadStartupFixture
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	compared := 0
	for _, expected := range fixture.Rows {
		if !expected.Native {
			continue
		}
		t.Run(expected.Name, func(t *testing.T) {
			home := t.TempDir()
			migrationRecordStartupConfig(t, home, expected)
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, *recordDownloadNativeBinary, expected.Args...)
			child.Dir, child.Env = home, migrationRecordStartupEnv(home)
			child.Env = append(child.Env, "PATH="+home)
			var out, diagnostics bytes.Buffer
			child.Stdout, child.Stderr = &out, &diagnostics
			input, err := child.StdinPipe()
			if err != nil {
				t.Fatal(err)
			}
			if err := child.Start(); err != nil {
				t.Fatal(err)
			}
			if _, err := io.WriteString(input, expected.Input); err != nil {
				t.Fatal(err)
			}
			if expected.StdinReads != 0 {
				if err := input.Close(); err != nil {
					t.Fatal(err)
				}
			}
			waitErr := child.Wait()
			input.Close()
			if ctx.Err() != nil {
				t.Fatalf("native preflight waited for unexpected input: %v", ctx.Err())
			}
			exit := 0
			if waitErr != nil {
				var exitErr *exec.ExitError
				if !errors.As(waitErr, &exitErr) {
					t.Fatal(waitErr)
				}
				exit = exitErr.ExitCode()
			}
			actual := migrationRecordDownloadStartupRow{}
			migrationRecordStartupState(t, home, &actual)
			normalize := func(s string) string { return strings.ReplaceAll(s, home, "<HOME>") }
			if exit != expected.Exit || normalize(out.String()) != expected.Stdout || normalize(diagnostics.String()) != expected.Stderr || actual.Config != expected.Config || actual.Database != expected.Database || actual.After != expected.After {
				t.Fatalf("native preflight differs: exit=%d stdout=%q stderr=%q config=%v database=%v after=%q; expected=%+v", exit, normalize(out.String()), normalize(diagnostics.String()), actual.Config, actual.Database, actual.After, expected)
			}
			compared++
		})
	}
	if compared != 14 {
		t.Fatalf("native preflight row count = %d, want 14", compared)
	}
}
