//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
	"unsafe"

	fanboxapp "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox"
	fanboxaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	"golang.org/x/sys/unix"
)

var migrationCaptureFanboxHelpRouting = flag.Bool("migration-capture-fanbox-help-routing", false, "capture isolated frozen Go FANBOX help routing")

const migrationFanboxHelpReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type migrationFanboxHelpInput struct {
	Args         []string `json:"args"`
	ConfigBefore *string  `json:"config_before"`
}

type migrationFanboxHelpFile struct {
	Path   string `json:"path"`
	Mode   string `json:"mode"`
	SHA256 string `json:"sha256,omitempty"`
}

type migrationFanboxHelpObservation struct {
	Exit                int                       `json:"exit"`
	Stdout              string                    `json:"stdout"`
	Stderr              string                    `json:"stderr"`
	Calls               []string                  `json:"calls"`
	StdinReads          int                       `json:"stdin_reads"`
	HomeBefore          []migrationFanboxHelpFile `json:"home_before"`
	HomeAfter           []migrationFanboxHelpFile `json:"home_after"`
	SocketDenied        bool                      `json:"socket_denied"`
	ExecDenied          bool                      `json:"exec_denied"`
	GoOnlyFindCommand   string                    `json:"go_only_find_command"`
	GoOnlyFindArgs      []string                  `json:"go_only_find_args"`
	GoOnlyFindError     string                    `json:"go_only_find_error"`
	GoOnlyChildExitCode int                       `json:"go_only_child_exit_code"`
}

type migrationFanboxHelpCase struct {
	Name        string                         `json:"name"`
	Comparison  string                         `json:"comparison"`
	Input       migrationFanboxHelpInput       `json:"input"`
	Observation migrationFanboxHelpObservation `json:"observation"`
}

type migrationFanboxHelpFixture struct {
	Reference           string                    `json:"reference"`
	Environment         string                    `json:"environment"`
	Sources             map[string]string         `json:"sources"`
	GoOnlyParserSources map[string]string         `json:"go_only_parser_sources"`
	Limitations         []string                  `json:"limitations"`
	Cases               []migrationFanboxHelpCase `json:"cases"`
}

type migrationFanboxHelpReader struct{ reads *int }

func (r migrationFanboxHelpReader) Read([]byte) (int, error) {
	*r.reads++
	return 0, errors.New("help routing must not read stdin")
}

type migrationFanboxHelpBrowser struct{ calls *[]string }

func (b migrationFanboxHelpBrowser) ReadSession(context.Context, string, string) (string, error) {
	*b.calls = append(*b.calls, "browser-session")
	return "", errors.New("help routing must not read browser sessions")
}

func migrationFanboxHelpTree(t *testing.T, home string) []migrationFanboxHelpFile {
	t.Helper()
	files := []migrationFanboxHelpFile{}
	err := filepath.WalkDir(home, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if path == home {
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(home, path)
		if err != nil {
			return err
		}
		file := migrationFanboxHelpFile{Path: filepath.ToSlash(relative), Mode: info.Mode().String()}
		if info.Mode().IsRegular() {
			body, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			file.SHA256 = fmt.Sprintf("%x", sha256.Sum256(body))
		} else if !entry.IsDir() {
			return fmt.Errorf("unexpected owned-home entry %s", relative)
		}
		files = append(files, file)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return files
}

func migrationFanboxHelpDenyExternal(t *testing.T) (bool, bool) {
	t.Helper()
	filter := []unix.SockFilter{
		{Code: unix.BPF_LD | unix.BPF_W | unix.BPF_ABS, K: 4},
		{Code: unix.BPF_JMP | unix.BPF_JEQ | unix.BPF_K, Jt: 1, K: unix.AUDIT_ARCH_X86_64},
		{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_KILL_PROCESS},
		{Code: unix.BPF_LD | unix.BPF_W | unix.BPF_ABS, K: 0},
		{Code: unix.BPF_JMP | unix.BPF_JGE | unix.BPF_K, Jf: 1, K: 0x40000000},
		{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ERRNO | uint32(unix.EPERM)},
	}
	for _, number := range []uint32{unix.SYS_SOCKET, unix.SYS_SOCKETPAIR, unix.SYS_CONNECT, unix.SYS_EXECVE, unix.SYS_EXECVEAT} {
		filter = append(filter,
			unix.SockFilter{Code: unix.BPF_JMP | unix.BPF_JEQ | unix.BPF_K, Jf: 1, K: number},
			unix.SockFilter{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ERRNO | uint32(unix.EPERM)},
		)
	}
	filter = append(filter, unix.SockFilter{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ALLOW})
	program := unix.SockFprog{Len: uint16(len(filter)), Filter: &filter[0]}
	if err := unix.Prctl(unix.PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0); err != nil {
		t.Fatal(err)
	}
	result, _, errno := unix.Syscall(unix.SYS_SECCOMP, unix.SECCOMP_SET_MODE_FILTER, unix.SECCOMP_FILTER_FLAG_TSYNC, uintptr(unsafe.Pointer(&program)))
	if errno != 0 || result != 0 {
		t.Fatalf("process-wide help isolation failed: result=%d errno=%v", result, errno)
	}
	fd, socketErr := unix.Socket(unix.AF_INET, unix.SOCK_STREAM, 0)
	if socketErr == nil {
		_ = unix.Close(fd)
	}
	execErr := unix.Exec("/proc/self/exe", []string{"owned-help-isolation-probe"}, []string{})
	if !errors.Is(socketErr, unix.EPERM) || !errors.Is(execErr, unix.EPERM) {
		t.Fatalf("external isolation not enforced: socket=%v exec=%v", socketErr, execErr)
	}
	return true, true
}

func TestMigrationFanboxHelpRoutingChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_FANBOX_HELP_ROUTING_CHILD")
	if encoded == "" {
		t.Skip("isolated FANBOX help helper")
	}
	var row migrationFanboxHelpCase
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || filepath.Dir(os.Getenv("XDG_CONFIG_HOME")) != home {
		t.Fatal("helper does not own its home and XDG paths")
	}
	if row.Input.ConfigBefore != nil {
		path := filepath.Join(home, ".pixiv-cli", "config.toml")
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Input.ConfigBefore), 0600); err != nil {
			t.Fatal(err)
		}
	}
	observation := migrationFanboxHelpObservation{Calls: []string{}, GoOnlyFindArgs: []string{}}
	forbidden := func(name string) error {
		observation.Calls = append(observation.Calls, name)
		return errors.New("help routing must not invoke " + name)
	}
	cleanupPendingWindowsUpdate = func() error { return forbidden("startup-cleanup") }
	automaticPersistentHandlerSupported = func() bool {
		observation.Calls = append(observation.Calls, "startup-supported")
		return false
	}
	ensureURLSchemeRelay = func(context.Context) error { return forbidden("startup-relay") }
	newCLIAutomaticUpdateChecker = func(string) (*update.AutomaticUpdateChecker, error) {
		return nil, forbidden("automatic-update")
	}
	newCLIFanboxService = func(app) (*fanboxapp.Facade, error) {
		return nil, forbidden("fanbox-service")
	}
	newCLIFanboxAccountService = func(app) (*fanboxaccount.Service, error) {
		return nil, forbidden("fanbox-account")
	}
	fanboxBrowserSessionReader = migrationFanboxHelpBrowser{calls: &observation.Calls}
	canPrompt = func(app) bool {
		observation.Calls = append(observation.Calls, "can-prompt")
		return false
	}
	reader := migrationFanboxHelpReader{reads: &observation.StdinReads}
	var out, diagnostics bytes.Buffer
	observation.SocketDenied, observation.ExecDenied = migrationFanboxHelpDenyExternal(t)
	observation.HomeBefore = migrationFanboxHelpTree(t, home)
	root := (app{in: reader, out: &out, errOut: &diagnostics}).newRootCommand()
	found, remaining, findErr := root.Find(row.Input.Args)
	if found != nil {
		observation.GoOnlyFindCommand = found.CommandPath()
	}
	observation.GoOnlyFindArgs = append(observation.GoOnlyFindArgs, remaining...)
	if findErr != nil {
		observation.GoOnlyFindError = findErr.Error()
	}
	observation.Exit = Run(append([]string{"pixiv"}, row.Input.Args...), reader, &out, &diagnostics)
	observation.Stdout, observation.Stderr = out.String(), diagnostics.String()
	observation.HomeAfter = migrationFanboxHelpTree(t, home)
	if len(observation.Calls) != 0 || observation.StdinReads != 0 || !reflect.DeepEqual(observation.HomeBefore, observation.HomeAfter) {
		t.Fatalf("help/parser route had side effects: %+v", observation)
	}
	if strings.Contains(observation.Stdout+observation.Stderr, home) {
		t.Fatal("owned-home path unexpectedly entered output; output must remain exact")
	}
	row.Observation = observation
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), body, 0600); err != nil {
		t.Fatal(err)
	}
}

func migrationFanboxHelpSources(t *testing.T) (map[string]string, map[string]string) {
	t.Helper()
	files := []string{
		"go.mod", "go.sum", "internal/cli/root.go", "internal/cli/execution.go", "internal/cli/composition.go",
		"internal/cli/commands/lifecycle.go", "internal/cli/invocation/invocation.go",
		"internal/cli/pipeline/pipeline.go", "internal/cli/pipeline/action.go", "internal/cli/pipeline/records.go",
		"internal/cli/commands/fanbox/fanbox.go", "internal/cli/commands/fanbox/auth/auth.go", "internal/cli/commands/fanbox/auth/account.go",
		"internal/cli/commands/fanbox/mcp/mcp.go", "internal/cli/commands/fanbox/post/post.go", "internal/cli/commands/fanbox/download/download.go",
		"internal/config/paths/paths.go", "internal/config/settings/paths.go", "internal/config/settings/store.go",
	}
	sources := map[string]string{}
	for _, file := range files {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		frozen, err := exec.CommandContext(ctx, "git", "show", migrationFanboxHelpReference+":"+file).Output()
		cancel()
		if err != nil {
			t.Fatalf("frozen source %s: %v", file, err)
		}
		current, err := os.ReadFile(filepath.Join("../..", file))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(current, frozen) {
			t.Fatalf("FANBOX help source differs from frozen Go: %s", file)
		}
		sources[file] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	output, err := exec.CommandContext(ctx, "go", "env", "GOMODCACHE").Output()
	if err != nil {
		t.Fatal(err)
	}
	parserSources := map[string]string{}
	for _, file := range []string{"github.com/spf13/cobra@v1.10.1/command.go", "github.com/spf13/pflag@v1.0.9/flag.go"} {
		body, err := os.ReadFile(filepath.Join(strings.TrimSpace(string(output)), file))
		if err != nil {
			t.Fatal(err)
		}
		parserSources[file] = fmt.Sprintf("%x", sha256.Sum256(body))
	}
	return sources, parserSources
}

func TestMigrationFanboxHelpRouting(t *testing.T) {
	sources, parserSources := migrationFanboxHelpSources(t)
	rows := []migrationFanboxHelpCase{}
	add := func(name string, args ...string) *migrationFanboxHelpCase {
		rows = append(rows, migrationFanboxHelpCase{Name: name, Comparison: "exact", Input: migrationFanboxHelpInput{Args: args}})
		return &rows[len(rows)-1]
	}
	add("fanbox-group-help", "fanbox", "--help")
	add("fanbox-group-bare", "fanbox")
	add("proxy-auth-space-is-group", "fanbox", "--proxy", "auth", "--help")
	add("proxy-auth-equals-is-group", "fanbox", "--proxy=auth", "--help")
	add("proxy-mcp-space-is-group", "fanbox", "--proxy", "mcp", "--help")
	add("proxy-auth-before-fanbox-is-group", "--proxy", "auth", "fanbox", "--help")
	add("auth-group-help", "fanbox", "auth", "--help")
	add("auth-before-proxy-auth-value", "fanbox", "auth", "--proxy", "auth", "--help")
	add("auth-after-proxy-auth-value", "fanbox", "--proxy", "auth", "auth", "--help")
	add("mcp-leaf-help", "fanbox", "mcp", "--help")
	add("import-help-bypasses-browser", "fanbox", "auth", "import", "--from-browser", "chrome", "--help")
	add("status-help-remains-plain-with-json", "fanbox", "auth", "status", "--json", "--help")
	add("help-before-auth-token", "fanbox", "--help", "auth")
	add("terminator-makes-auth-and-help-positional", "fanbox", "--", "auth", "--help")
	add("help-value-scan-past-terminator-finds-auth", "fanbox", "--help", "--", "auth")
	add("terminator-as-proxy-value-retains-auth-route", "fanbox", "--proxy", "--", "auth", "--help")
	add("missing-proxy-value", "fanbox", "--proxy").Comparison = "go-only-parser-spelling"
	add("unknown-flag-precedes-help", "fanbox", "--unknown", "--help")
	add("invalid-no-proxy-boolean-precedes-help", "fanbox", "--no-proxy=invalid", "--help").Comparison = "go-only-parser-spelling"
	add("auth-help-bypasses-extra-args", "fanbox", "auth", "extra", "--help")
	malformed := "[unfinished\n"
	add("proxy-auth-group-help-bypasses-malformed-config", "fanbox", "--proxy", "auth", "--help").Input.ConfigBefore = &malformed
	add("mcp-help-bypasses-malformed-config", "fanbox", "mcp", "--help").Input.ConfigBefore = &malformed
	fixture := migrationFanboxHelpFixture{
		Reference: migrationFanboxHelpReference, Environment: "linux/amd64", Sources: sources, GoOnlyParserSources: parserSources,
		Limitations: []string{
			"Go-only recovery checkpoint; this fixture makes no Rust FANBOX parity claim.",
			"Actual cli.Run invocations execute inside bounded disposable Go test children, not separately built cmd/pixiv executables.",
			"Owned HOME/USERPROFILE/XDG paths, process-wide socket/connect/exec denial, forbidden dependency canaries and unchanged owned-home file modes/content establish no observed CLI side effects; timestamps are not compared.",
			"Linux/amd64 capture and replay only; native browser, accounts, network, platform integrations, content operations and denied supplemental native HTTP2/upload probes are outside this fixture.",
			"Go-only parser spelling rows retain exact pflag diagnostics; go_only_find_* records Cobra Find before execution rather than a portable final-command identity.",
		},
		Cases: rows,
	}
	for index := range fixture.Cases {
		row := fixture.Cases[index]
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxHelpRoutingChild$", "-test.count=1", "-test.timeout=8s")
			child.WaitDelay = time.Second
			child.Dir = home
			child.Env = []string{
				"HOME=" + home, "USERPROFILE=" + home, "XDG_CONFIG_HOME=" + filepath.Join(home, "xdg-config"),
				"XDG_DATA_HOME=" + filepath.Join(home, "xdg-data"), "XDG_CACHE_HOME=" + filepath.Join(home, "xdg-cache"),
				"XDG_STATE_HOME=" + filepath.Join(home, "xdg-state"), "XDG_RUNTIME_DIR=" + filepath.Join(home, "xdg-runtime"),
				"APPDATA=" + filepath.Join(home, "appdata"), "LOCALAPPDATA=" + filepath.Join(home, "local-appdata"),
				"TMPDIR=" + home, "PATH=", "LANG=C", "LC_ALL=C", "GOMAXPROCS=2", "GORACE=atexit_sleep_ms=0",
				"MIGRATION_FANBOX_HELP_ROUTING_CHILD=" + string(body),
			}
			if output, err := child.CombinedOutput(); err != nil {
				t.Fatalf("bounded help child: %v\n%s", err, output)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &fixture.Cases[index]); err != nil {
				t.Fatal(err)
			}
			fixture.Cases[index].Observation.GoOnlyChildExitCode = child.ProcessState.ExitCode()
		})
	}
	if t.Failed() {
		return
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := "../../crates/pixiv-cli/tests/fixtures/fanbox-help-routing.json"
	if *migrationCaptureFanboxHelpRouting {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(body, want) {
		t.Fatal("FANBOX help routing differs from frozen Go fixture")
	}
}
