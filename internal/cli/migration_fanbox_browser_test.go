//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"
	"unsafe"

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies/system"
	fanboxauth "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox/auth"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
	"golang.org/x/sys/unix"
)

var captureMigrationFanboxBrowserImport = flag.Bool("migration-capture-fanbox-browser-import", false, "capture connected frozen Go saved-browser FANBOX import")

const migrationBrowserImportSQLiteSHA256 = "a948747fbdbbf3827101254067b7ea480bad8b13732decbf750da75026c3e38d"

type migrationBrowserImportCookie struct {
	Host         string `json:"host"`
	Name         string `json:"name"`
	ValueHex     string `json:"value_hex"`
	EncryptedHex string `json:"encrypted_hex"`
}
type migrationBrowserImportProfile struct {
	ID      string                         `json:"id"`
	Path    string                         `json:"path"`
	Mode    string                         `json:"mode"`
	Cookies []migrationBrowserImportCookie `json:"cookies"`
}
type migrationBrowserImportFile struct {
	Path     string `json:"path"`
	Mode     string `json:"mode"`
	BytesHex string `json:"bytes_hex"`
}
type migrationBrowserImportCommand struct {
	Program    string            `json:"program"`
	Args       []string          `json:"args"`
	Parameters map[string]string `json:"parameters,omitempty"`
}
type migrationBrowserImportObservation struct {
	migrationAuthFanboxObservation
	Commands           []migrationBrowserImportCommand `json:"commands"`
	NativeSessionHex   string                          `json:"native_session_hex"`
	NativeSessionError string                          `json:"native_session_error"`
}
type migrationBrowserImportDiscovery struct {
	Profiles []system.Profile `json:"profiles"`
	Error    string           `json:"error"`
}
type migrationBrowserImportCase struct {
	Name              string                              `json:"name"`
	Browser           string                              `json:"browser"`
	XDGEmpty          bool                                `json:"xdg_empty,omitempty"`
	Profiles          []migrationBrowserImportProfile     `json:"profiles"`
	FirefoxINI        *string                             `json:"firefox_ini"`
	Files             []migrationBrowserImportFile        `json:"files"`
	SecretMode        string                              `json:"secret_mode"`
	SecretHex         string                              `json:"secret_hex"`
	SQLiteUnavailable bool                                `json:"sqlite_unavailable,omitempty"`
	ConfigBefore      *string                             `json:"config_before"`
	Seed              []int64                             `json:"seed"`
	Steps             []migrationAuthFanboxStep           `json:"steps"`
	Discovery         migrationBrowserImportDiscovery     `json:"discovery"`
	Observations      []migrationBrowserImportObservation `json:"observations"`
}
type migrationBrowserImportFixture struct {
	Reference         string                       `json:"reference"`
	Environment       string                       `json:"environment"`
	GoVersion         string                       `json:"go_version"`
	Sources           map[string]string            `json:"sources"`
	Dependencies      map[string]string            `json:"dependencies"`
	DependencySources map[string]string            `json:"dependency_sources"`
	StdlibSources     map[string]string            `json:"stdlib_sources"`
	ReusedFixtures    map[string]string            `json:"reused_fixtures"`
	SupportSources    map[string]string            `json:"support_sources"`
	SQLiteTool        map[string]string            `json:"sqlite_tool"`
	IsolationScope    []string                     `json:"isolation_scope"`
	Limitations       []string                     `json:"limitations"`
	Cases             []migrationBrowserImportCase `json:"cases"`
}

func init() {
	if os.Getenv("MIGRATION_BROWSER_NATIVE_HELPER") != "1" {
		return
	}
	program := filepath.Base(os.Args[0])
	if program != "sqlite3" && program != "secret-tool" {
		return
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || os.Getenv("PATH") != filepath.Join(home, "bin") {
		os.Exit(91)
	}
	call := migrationBrowserImportCommand{Program: program, Args: []string{}}
	if program == "sqlite3" {
		call.Parameters = map[string]string{}
		for i := 1; i < len(os.Args); i++ {
			if os.Args[i] == "-cmd" {
				i++
				if i >= len(os.Args) {
					os.Exit(92)
				}
				parameter := strings.TrimPrefix(os.Args[i], ".parameter set ")
				name, value, ok := strings.Cut(parameter, " ")
				if !ok || (name != "@h1" && name != "@h2" && name != "@n") {
					os.Exit(93)
				}
				call.Parameters[name] = value
			} else {
				call.Args = append(call.Args, strings.ReplaceAll(os.Args[i], home, "$HOME"))
			}
		}
	} else {
		call.Args = append(call.Args, os.Args[1:]...)
	}
	encoded, e := json.Marshal(call)
	if e != nil {
		os.Exit(94)
	}
	log, e := os.OpenFile(filepath.Join(home, "commands.jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if e != nil {
		os.Exit(95)
	}
	_, e = log.Write(append(encoded, '\n'))
	closeErr := log.Close()
	if e != nil || closeErr != nil {
		os.Exit(96)
	}
	if program == "sqlite3" {
		executable := filepath.Join(home, "bin", "sqlite3-official")
		if e = unix.Exec(executable, append([]string{executable}, os.Args[1:]...), os.Environ()); e != nil {
			os.Exit(97)
		}
	}
	switch os.Getenv("MIGRATION_BROWSER_SECRET_MODE") {
	case "error":
		fmt.Fprint(os.Stderr, "owned-private-secret-canary permission denied")
		os.Exit(7)
	case "empty":
		os.Exit(0)
	}
	value, e := hex.DecodeString(os.Getenv("MIGRATION_BROWSER_SECRET_HEX"))
	if e != nil {
		os.Exit(98)
	}
	if _, e = os.Stdout.Write(value); e != nil {
		os.Exit(99)
	}
	os.Exit(0)
}

func migrationBrowserImportDenyNetwork(t *testing.T) bool {
	t.Helper()
	filter := []unix.SockFilter{
		{Code: unix.BPF_LD | unix.BPF_W | unix.BPF_ABS, K: 4},
		{Code: unix.BPF_JMP | unix.BPF_JEQ | unix.BPF_K, Jt: 1, K: unix.AUDIT_ARCH_X86_64},
		{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_KILL_PROCESS},
		{Code: unix.BPF_LD | unix.BPF_W | unix.BPF_ABS, K: 0},
		{Code: unix.BPF_JMP | unix.BPF_JGE | unix.BPF_K, Jf: 1, K: 0x40000000},
		{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ERRNO | uint32(unix.EPERM)},
	}
	for _, number := range []uint32{unix.SYS_SOCKET, unix.SYS_SOCKETPAIR, unix.SYS_CONNECT} {
		filter = append(filter, unix.SockFilter{Code: unix.BPF_JMP | unix.BPF_JEQ | unix.BPF_K, Jf: 1, K: number}, unix.SockFilter{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ERRNO | uint32(unix.EPERM)})
	}
	filter = append(filter, unix.SockFilter{Code: unix.BPF_RET | unix.BPF_K, K: unix.SECCOMP_RET_ALLOW})
	program := unix.SockFprog{Len: uint16(len(filter)), Filter: &filter[0]}
	if e := unix.Prctl(unix.PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0); e != nil {
		t.Fatal(e)
	}
	result, _, errno := unix.Syscall(unix.SYS_SECCOMP, unix.SECCOMP_SET_MODE_FILTER, unix.SECCOMP_FILTER_FLAG_TSYNC, uintptr(unsafe.Pointer(&program)))
	if errno != 0 || result != 0 {
		t.Fatalf("network-only isolation failed: %d/%v", result, errno)
	}
	fd, e := unix.Socket(unix.AF_INET, unix.SOCK_STREAM, 0)
	if e == nil {
		_ = unix.Close(fd)
	}
	if !errors.Is(e, unix.EPERM) {
		t.Fatalf("socket denial unverified: %v", e)
	}
	return true
}

func migrationBrowserImportCommands(t *testing.T, home string) []migrationBrowserImportCommand {
	t.Helper()
	result := []migrationBrowserImportCommand{}
	body, e := os.ReadFile(filepath.Join(home, "commands.jsonl"))
	if errors.Is(e, os.ErrNotExist) {
		return result
	}
	if e != nil {
		t.Fatal(e)
	}
	for _, line := range bytes.Split(bytes.TrimSpace(body), []byte{'\n'}) {
		var call migrationBrowserImportCommand
		if e = json.Unmarshal(line, &call); e != nil {
			t.Fatal(e)
		}
		result = append(result, call)
	}
	return result
}

func migrationBrowserImportDiscover(t *testing.T, home, browser string) migrationBrowserImportDiscovery {
	t.Helper()
	observation := migrationBrowserImportDiscovery{Profiles: []system.Profile{}}
	provider, e := system.New(browser)
	if e != nil {
		observation.Error = e.Error()
		return observation
	}
	profiles, e := provider.DiscoverProfiles(context.Background())
	if e != nil {
		observation.Error = e.Error()
	}
	for _, profile := range profiles {
		profile.Path = strings.ReplaceAll(profile.Path, home, "$HOME")
		observation.Profiles = append(observation.Profiles, profile)
	}
	if e = provider.Close(); e != nil {
		t.Fatal(e)
	}
	return observation
}

func TestMigrationFanboxBrowserImportChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_FANBOX_BROWSER_IMPORT_CHILD")
	if encoded == "" {
		t.Skip("owned FANBOX auth child")
	}
	var row migrationBrowserImportCase
	if e := json.Unmarshal([]byte(encoded), &row); e != nil {
		t.Fatal(e)
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || filepath.Dir(os.Getenv("XDG_CONFIG_HOME")) != home {
		t.Fatal("child must own home/XDG")
	}
	migrationBrowserImportPrepare(t, home, row)
	start := time.Now().Unix()
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	created := map[int64]int64{}
	if row.ConfigBefore != nil {
		if e := os.MkdirAll(directory, 0700); e != nil {
			t.Fatal(e)
		}
		if e := os.WriteFile(path, []byte(*row.ConfigBefore), 0600); e != nil {
			t.Fatal(e)
		}
	}
	if len(row.Seed) > 0 {
		db, e := database.Open(directory)
		if e != nil {
			t.Fatal(e)
		}
		for _, id := range row.Seed {
			a := account.New(id, fmt.Sprintf("seed %d", id), "", []byte(fmt.Sprintf("synthetic-seed-%d", id)))
			a.ValidatedAt = 1700000000
			if e = db.SaveFanboxCredential(context.Background(), a); e != nil {
				t.Fatal(e)
			}
		}
		if e = db.Close(); e != nil {
			t.Fatal(e)
		}
	}
	socketDenied := migrationBrowserImportDenyNetwork(t)
	row.Discovery = migrationBrowserImportDiscover(t, home, row.Browser)
	row.Observations = []migrationBrowserImportObservation{}
	commandOffset := 0
	for _, step := range row.Steps {
		obs := migrationAuthFanboxObservation{OutputWrites: []migrationAuthFanboxWrite{}, ErrorWrites: []migrationAuthFanboxWrite{}, Trace: []string{}, Requests: []map[string]any{}, SocketDenied: socketDenied, ExecDenied: false}
		obs.Before = migrationAuthFanboxStateAt(t, home, start, created)
		input, e := hex.DecodeString(step.InputHex)
		if e != nil {
			t.Fatal(e)
		}
		reader := &migrationAuthFanboxReader{value: bytes.NewReader(input), step: step, obs: &obs}
		out := &migrationAuthFanboxWriter{mode: step.Writer, name: "stdout", obs: &obs, writes: []migrationAuthFanboxWrite{}}
		errOut := &migrationAuthFanboxWriter{mode: step.StderrWriter, name: "stderr", obs: &obs, writes: []migrationAuthFanboxWrite{}}
		ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationAuthFanboxKey{}, "command"))
		if step.Canceled {
			cancel()
		}
		cleanupPendingWindowsUpdate = func() error {
			obs.Trace = append(obs.Trace, "startup.cleanup")
			if step.StartupError {
				return errors.New("owned fixture startup failure")
			}
			return nil
		}
		automaticPersistentHandlerSupported = func() bool { obs.Trace = append(obs.Trace, "startup.supported"); return step.StartupRelay }
		ensureURLSchemeRelay = func(context.Context) error {
			obs.Trace = append(obs.Trace, "startup.relay")
			return errors.New("owned fixture relay failure")
		}
		canPrompt = func(app) bool { obs.Trace = append(obs.Trace, "can-prompt"); return step.CanPrompt }
		promptSecret = func(_ app, m string) (string, error) {
			obs.Trace = append(obs.Trace, "prompt.secret/"+m)
			if step.PromptError {
				return "", errors.New("owned fixture secret prompt failure")
			}
			return step.PromptValue, nil
		}
		promptConfirm = func(_ app, m string, d bool) (bool, error) {
			obs.Trace = append(obs.Trace, fmt.Sprintf("prompt.confirm/%s/default=%v", m, d))
			if step.PromptError {
				return false, errors.New("owned fixture confirmation failure")
			}
			return step.Confirm, nil
		}
		nativeSessionHex, nativeSessionError := "", ""
		fanboxBrowserSessionReader = migrationBrowserImportReader{obs: &obs, valueHex: &nativeSessionHex, errorMessage: &nativeSessionError}
		newCLIFanboxAccountService = func(a app) (*account.Service, error) {
			obs.Trace = append(obs.Trace, "account.factory")
			if step.FactoryError {
				return nil, errors.New("owned fixture account factory failure")
			}
			if step.FactoryNil {
				return nil, nil
			}
			db, e := openCLIAuthDatabase()
			if e != nil {
				return nil, e
			}
			obs.Trace = append(obs.Trace, "database.open")
			a.closeState.add(func() error {
				obs.Trace = append(obs.Trace, "database.close")
				e := db.Close()
				if step.CloseError {
					return errors.Join(e, errors.New("owned fixture database close failure"))
				}
				return e
			})
			files := &migrationAuthFanboxFiles{path: path, fail: step.FileFailure, obs: &obs}
			defaults := &migrationAuthFanboxDefaults{store: settings.Store{Files: files}, step: step, obs: &obs}
			service := account.NewService(&migrationAuthFanboxRepository{db: db, obs: &obs, fail: step.RepositoryFailure}, defaults)
			transport := &migrationAuthFanboxTransport{step: step, obs: &obs, cancel: cancel}
			options := func() (fanboxsdk.Options, error) {
				obs.Trace = append(obs.Trace, "options.load")
				if step.OptionsError {
					return fanboxsdk.Options{}, errors.New("owned fixture options failure")
				}
				return fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}, UserAgent: "fixture-auth-agent"}, nil
			}
			service.LoadOptionsFunc = options
			if step.InjectSession {
				service.OpenSessionFunc = func(value string) (*fanboxsdk.Client, error) {
					obs.Trace = append(obs.Trace, "session.open.injected")
					return fanboxsdk.OpenWith(fanboxsdk.SessionCredentials{FANBOXSESSID: value}, fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}, UserAgent: "fixture-auth-agent"})
				}
			}
			return service, nil
		}
		buildinfo.Version = "dev"
		if step.Release {
			buildinfo.Version = "v1.2.3"
		}
		loadCLIRuntimeConfig = func() (settings.RuntimeConfig, error) {
			obs.Trace = append(obs.Trace, "update.runtime")
			if step.UpdateMode == "runtime-error" {
				return settings.RuntimeConfig{}, errors.New("owned fixture update runtime failure")
			}
			return settings.RuntimeConfig{UpdateCheckEnabled: step.UpdateMode != "disabled", HTTPSProxy: "http://fixture-global-proxy:8080"}, nil
		}
		newCLIAutomaticUpdateChecker = func(proxy string) (*update.AutomaticUpdateChecker, error) {
			obs.Trace = append(obs.Trace, "update.factory/proxy="+proxy)
			return nil, errors.New("owned fixture update factory failure")
		}
		obs.Exit = RunContext(ctx, append([]string{"pixiv"}, step.Args...), reader, out, errOut)
		cancel()
		obs.Stdout = out.bytes.String()
		obs.Stderr = errOut.bytes.String()
		obs.OutputWrites = out.writes
		obs.ErrorWrites = errOut.writes
		obs.After = migrationAuthFanboxStateAt(t, home, start, created)
		for _, call := range obs.Trace {
			if strings.Contains(call, "FORBIDDEN") {
				t.Fatal(call)
			}
		}
		for _, canary := range []string{home, "/owned-private-path-canary", "/owned-private-profile-canary", "owned-private-secret-canary", "owned-native-session", "owned-decrypted-session", "owned-password", "plaintext-private-canary", "synthetic-browser-secret", "synthetic-seed-", "opaque-secret-canary"} {
			if strings.Contains(obs.Stdout+obs.Stderr, canary) {
				t.Fatal("private fixture path/secret leaked into exact CLI output")
			}
		}
		commands := migrationBrowserImportCommands(t, home)
		row.Observations = append(row.Observations, migrationBrowserImportObservation{migrationAuthFanboxObservation: obs, NativeSessionHex: nativeSessionHex, NativeSessionError: nativeSessionError, Commands: append([]migrationBrowserImportCommand{}, commands[commandOffset:]...)})
		commandOffset = len(commands)
	}
	b, e := json.Marshal(row)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(filepath.Join(home, "result.json"), b, 0600); e != nil {
		t.Fatal(e)
	}
}

type migrationBrowserImportReader struct {
	obs          *migrationAuthFanboxObservation
	valueHex     *string
	errorMessage *string
}

func (p migrationBrowserImportReader) ReadSession(ctx context.Context, browser, profile string) (string, error) {
	p.obs.Trace = append(p.obs.Trace, "browser.read/browser="+browser+"/profile="+profile+"/context="+migrationAuthFanboxContext(ctx))
	value, e := (fanboxauth.SystemBrowserProvider{}).ReadSession(ctx, browser, profile)
	*p.valueHex = hex.EncodeToString([]byte(value))
	if e != nil {
		*p.errorMessage = e.Error()
	}
	return value, e
}

func migrationBrowserImportPrepare(t *testing.T, home string, row migrationBrowserImportCase) {
	t.Helper()
	bin := filepath.Join(home, "bin")
	if e := os.MkdirAll(bin, 0700); e != nil {
		t.Fatal(e)
	}
	testExecutable, e := filepath.Abs(os.Args[0])
	if e != nil {
		t.Fatal(e)
	}
	if !row.SQLiteUnavailable {
		body, e := os.ReadFile(os.Getenv("MIGRATION_BROWSER_SQLITE_SOURCE"))
		if e != nil {
			t.Fatal(e)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != migrationBrowserImportSQLiteSHA256 {
			t.Fatal("unverified SQLite shell")
		}
		if e = os.WriteFile(filepath.Join(bin, "sqlite3-official"), body, 0700); e != nil {
			t.Fatal(e)
		}
		if e = os.Symlink(testExecutable, filepath.Join(bin, "sqlite3")); e != nil {
			t.Fatal(e)
		}
	}
	if row.SecretMode != "missing" {
		if e = os.Symlink(testExecutable, filepath.Join(bin, "secret-tool")); e != nil {
			t.Fatal(e)
		}
	}
	root := filepath.Join(os.Getenv("XDG_CONFIG_HOME"), "google-chrome")
	switch strings.ToLower(strings.TrimSpace(row.Browser)) {
	case "edge":
		root = filepath.Join(os.Getenv("XDG_CONFIG_HOME"), "microsoft-edge")
	case "firefox":
		root = filepath.Join(os.Getenv("XDG_CONFIG_HOME"), "mozilla", "firefox")
	}
	if row.XDGEmpty {
		root = strings.Replace(root, os.Getenv("XDG_CONFIG_HOME"), filepath.Join(home, ".config"), 1)
		if e = os.Setenv("XDG_CONFIG_HOME", ""); e != nil {
			t.Fatal(e)
		}
	}
	for _, profile := range row.Profiles {
		directory := filepath.Join(root, profile.Path)
		if strings.HasPrefix(profile.Path, "$HOME/") {
			directory = filepath.Join(home, strings.TrimPrefix(profile.Path, "$HOME/"))
		}
		if e = os.MkdirAll(directory, 0700); e != nil {
			t.Fatal(e)
		}
		filename := "Cookies"
		if strings.EqualFold(strings.TrimSpace(row.Browser), "firefox") {
			filename = "cookies.sqlite"
		}
		path := filepath.Join(directory, filename)
		switch profile.Mode {
		case "missing":
			continue
		case "directory":
			if e = os.MkdirAll(path, 0700); e != nil {
				t.Fatal(e)
			}
			continue
		case "invalid":
			if e = os.WriteFile(path, []byte("owned-invalid-database"), 0600); e != nil {
				t.Fatal(e)
			}
			continue
		}
		db, e := sql.Open("sqlite", path)
		if e != nil {
			t.Fatal(e)
		}
		if profile.Mode != "missing-table" {
			statement := `CREATE TABLE cookies(host_key TEXT,name TEXT,value TEXT,encrypted_value BLOB)`
			if filename == "cookies.sqlite" {
				statement = `CREATE TABLE moz_cookies(host TEXT,name TEXT,value TEXT)`
			}
			if _, e = db.Exec(statement); e != nil {
				t.Fatal(e)
			}
			for _, cookie := range profile.Cookies {
				value, e := hex.DecodeString(cookie.ValueHex)
				if e != nil {
					t.Fatal(e)
				}
				encrypted, e := hex.DecodeString(cookie.EncryptedHex)
				if e != nil {
					t.Fatal(e)
				}
				if filename == "cookies.sqlite" {
					_, e = db.Exec(`INSERT INTO moz_cookies VALUES(?,?,?)`, cookie.Host, cookie.Name, string(value))
				} else {
					_, e = db.Exec(`INSERT INTO cookies VALUES(?,?,?,?)`, cookie.Host, cookie.Name, string(value), encrypted)
				}
				if e != nil {
					t.Fatal(e)
				}
			}
		} else {
			if _, e = db.Exec(`CREATE TABLE owned_unrelated(value TEXT)`); e != nil {
				t.Fatal(e)
			}
		}
		if e = db.Close(); e != nil {
			t.Fatal(e)
		}
		if e = os.Chmod(path, 0600); e != nil {
			t.Fatal(e)
		}
	}
	if row.FirefoxINI != nil {
		if e = os.MkdirAll(root, 0700); e != nil {
			t.Fatal(e)
		}
		body := strings.ReplaceAll(*row.FirefoxINI, "$HOME", home)
		if e = os.WriteFile(filepath.Join(root, "profiles.ini"), []byte(body), 0600); e != nil {
			t.Fatal(e)
		}
	}
	for _, file := range row.Files {
		path := filepath.Join(home, file.Path)
		if strings.HasPrefix(file.Path, "$ROOT/") {
			path = filepath.Join(root, strings.TrimPrefix(file.Path, "$ROOT/"))
		}
		if e = os.MkdirAll(filepath.Dir(path), 0700); e != nil {
			t.Fatal(e)
		}
		if file.Mode == "directory" {
			if e = os.MkdirAll(path, 0700); e != nil {
				t.Fatal(e)
			}
			continue
		}
		body, e := hex.DecodeString(file.BytesHex)
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(path, body, 0600); e != nil {
			t.Fatal(e)
		}
	}
	if e = os.Setenv("MIGRATION_BROWSER_SECRET_MODE", row.SecretMode); e != nil {
		t.Fatal(e)
	}
	if e = os.Setenv("MIGRATION_BROWSER_SECRET_HEX", row.SecretHex); e != nil {
		t.Fatal(e)
	}
}

func migrationBrowserImportSafari(cookies []migrationBrowserImportCookie) []byte {
	records := [][]byte{}
	for _, cookie := range cookies {
		value, e := hex.DecodeString(cookie.ValueHex)
		if e != nil {
			panic(e)
		}
		record := []byte{0, 0, 0, 7}
		for _, field := range [][]byte{[]byte(cookie.Host), []byte(cookie.Name), []byte("/"), value} {
			if len(field) > 255 {
				panic("oversized owned Safari field")
			}
			record = append(record, byte(len(field)))
			record = append(record, field...)
		}
		record = append(record, make([]byte, 12)...)
		record = append(record, 5, 0)
		binary.BigEndian.PutUint16(record[:2], uint16(len(record)))
		records = append(records, record)
	}
	pageStart := 16 + 4*len(records)
	page := make([]byte, pageStart)
	binary.BigEndian.PutUint32(page[:4], 16)
	binary.BigEndian.PutUint32(page[4:8], uint32(len(records)))
	binary.BigEndian.PutUint32(page[8:12], uint32(pageStart))
	offset := 0
	for i, record := range records {
		binary.BigEndian.PutUint32(page[16+i*4:20+i*4], uint32(offset))
		page = append(page, record...)
		offset += len(record)
	}
	body := []byte{'c', 'o', 'o', 'k', 0, 0, 0, 1, 0, 0, 0, 0}
	binary.BigEndian.PutUint32(body[8:12], uint32(len(page)))
	return append(body, page...)
}
func migrationBrowserImportGCM(value, key []byte, prefix string) string {
	block, e := aes.NewCipher(key)
	if e != nil {
		panic(e)
	}
	gcm, e := cipher.NewGCM(block)
	if e != nil {
		panic(e)
	}
	nonce := []byte("ownednonce12")
	body := append([]byte(prefix), nonce...)
	body = append(body, gcm.Seal(nil, nonce, value, nil)...)
	return hex.EncodeToString(body)
}
func migrationBrowserImportCBC(value, key []byte, prefix string) string {
	block, e := aes.NewCipher(key)
	if e != nil {
		panic(e)
	}
	padding := aes.BlockSize - len(value)%aes.BlockSize
	plain := append(append([]byte{}, value...), bytes.Repeat([]byte{byte(padding)}, padding)...)
	encrypted := make([]byte, len(plain))
	cipher.NewCBCEncrypter(block, bytes.Repeat([]byte{' '}, aes.BlockSize)).CryptBlocks(encrypted, plain)
	return hex.EncodeToString(append([]byte(prefix), encrypted...))
}

func migrationBrowserImportCases() []migrationBrowserImportCase {
	rows := []migrationBrowserImportCase{}
	cookie := func(value string) migrationBrowserImportCookie {
		return migrationBrowserImportCookie{Host: ".fanbox.cc", Name: "FANBOXSESSID", ValueHex: hex.EncodeToString([]byte(value))}
	}
	raw := func(browser string) migrationAuthFanboxStep {
		return migrationAuthFanboxStep{Args: []string{"fanbox", "auth", "import", "--from-browser", browser, "--json"}, InputHex: hex.EncodeToString([]byte("unread-owned-input")), InputError: true}
	}
	makeRow := func(name, browser string, cookies []migrationBrowserImportCookie) migrationBrowserImportCase {
		row := migrationBrowserImportCase{Name: name, Browser: browser, Profiles: []migrationBrowserImportProfile{}, Files: []migrationBrowserImportFile{}, SecretMode: "missing", Seed: []int64{}, Steps: []migrationAuthFanboxStep{raw(browser)}, Observations: []migrationBrowserImportObservation{}}
		switch browser {
		case "safari":
			row.Files = append(row.Files, migrationBrowserImportFile{Path: "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies", BytesHex: hex.EncodeToString(migrationBrowserImportSafari(cookies))})
		case "firefox":
			ini := "[Profile0]\nName=Owned Firefox\nPath=owned.default\nIsRelative=1\n"
			row.FirefoxINI = &ini
			row.Profiles = append(row.Profiles, migrationBrowserImportProfile{ID: "owned.default", Path: "owned.default", Cookies: cookies})
		default:
			row.Profiles = append(row.Profiles, migrationBrowserImportProfile{ID: "Default", Path: "Default", Cookies: cookies})
		}
		return row
	}
	add := func(row migrationBrowserImportCase) *migrationBrowserImportCase {
		body, e := json.Marshal(row)
		if e != nil {
			panic(e)
		}
		var clone migrationBrowserImportCase
		if e = json.Unmarshal(body, &clone); e != nil {
			panic(e)
		}
		rows = append(rows, clone)
		return &rows[len(rows)-1]
	}
	for _, browser := range []string{"chrome", "edge", "firefox", "safari"} {
		good := cookie("owned-native-session")
		ignored := cookie("ignored-private-cookie")
		ignored.Host = "evil.fanbox.cc"
		ignoredName := cookie("ignored-private-cookie")
		ignoredName.Name = "OTHER"
		noDot := good
		noDot.Host = "fanbox.cc"
		add(makeRow(browser+"/exact-query-success", browser, []migrationBrowserImportCookie{ignored, ignoredName, noDot}))
		row := makeRow(browser+"/not-installed", browser, nil)
		row.Profiles = []migrationBrowserImportProfile{}
		row.Files = []migrationBrowserImportFile{}
		row.FirefoxINI = nil
		add(row)
		add(makeRow(browser+"/zero-cookie", browser, []migrationBrowserImportCookie{ignored, ignoredName}))
		add(makeRow(browser+"/multiple-cookies", browser, []migrationBrowserImportCookie{good, noDot}))
		add(makeRow(browser+"/empty-session", browser, []migrationBrowserImportCookie{cookie("")}))
		add(makeRow(browser+"/matched-invalid-utf8", browser, []migrationBrowserImportCookie{cookie(string([]byte{'x', 0xff, 'y'}))}))
	}
	row := makeRow("chrome/normalized-name-xdg-default", "chrome", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Browser = " ChRoMe "
	row.Steps[0] = raw(row.Browser)
	row.XDGEmpty = true
	add(row)
	row = makeRow("chrome/multiple-profiles-sorted", "chrome", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Profiles = append([]migrationBrowserImportProfile{{ID: "z Profile", Path: "z Profile", Cookies: []migrationBrowserImportCookie{cookie("owned-secondary-session")}}}, row.Profiles...)
	add(row)
	row.Name = "chrome/select-profile-space"
	row.Steps[0].Args = append(row.Steps[0].Args, "--profile", "z Profile")
	add(row)
	row.Name = "chrome/unknown-profile-redacted"
	row.Steps[0] = raw("chrome")
	row.Steps[0].Args = append(row.Steps[0].Args, "--profile", "/owned-private-profile-canary")
	add(row)
	row = makeRow("chrome/hidden-directory-cookie-directory-skipped", "chrome", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Profiles = append(row.Profiles, migrationBrowserImportProfile{ID: ".hidden", Path: ".hidden", Cookies: []migrationBrowserImportCookie{cookie("ignored-private-cookie")}}, migrationBrowserImportProfile{ID: "Directory", Path: "Directory", Mode: "directory"}, migrationBrowserImportProfile{ID: "NetworkOnly", Path: "NetworkOnly", Mode: "missing"})
	row.Files = append(row.Files, migrationBrowserImportFile{Path: "$ROOT/NetworkOnly/Network/Cookies", BytesHex: hex.EncodeToString([]byte("ignored-private-database"))})
	add(row)
	for _, browser := range []string{"chrome", "edge", "firefox"} {
		for _, mode := range []string{"invalid", "missing-table"} {
			row = makeRow(browser+"/database-"+mode, browser, []migrationBrowserImportCookie{cookie("owned-native-session")})
			row.Profiles[0].Mode = mode
			add(row)
		}
		row = makeRow(browser+"/sqlite-unavailable", browser, []migrationBrowserImportCookie{cookie("owned-native-session")})
		row.SQLiteUnavailable = true
		add(row)
	}
	row = makeRow("firefox/arbitrary-section-relative-default", "firefox", []migrationBrowserImportCookie{cookie("owned-native-session")})
	ini := " ignored = line\n[Unrelated]\n Name = Owned arbitrary section\n Path = owned.default\n"
	row.FirefoxINI = &ini
	add(row)
	row = makeRow("firefox/absolute-isrelative-non-one", "firefox", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Profiles[0].Path = "$HOME/absolute/owned.default"
	ini = "[Profile0]\nIsRelative=true\nPath=$HOME/absolute/owned.default\n"
	row.FirefoxINI = &ini
	add(row)
	row = makeRow("firefox/duplicate-id-multiple-profiles", "firefox", []migrationBrowserImportCookie{cookie("owned-native-session")})
	ini = "[Profile0]\nPath=owned.default\n[Anything]\nPath=owned.default\n"
	row.FirefoxINI = &ini
	add(row)
	row.Name = "firefox/duplicate-id-explicit-first"
	row.Steps[0].Args = append(row.Steps[0].Args, "--profile", "owned.default")
	add(row)
	row = makeRow("firefox/cookie-directory-discovered-read-fails", "firefox", nil)
	row.Profiles[0].Mode = "directory"
	add(row)
	row = makeRow("firefox/invalid-profile-skipped", "firefox", nil)
	ini = "[Profile0]\nPath=.hidden\n[Profile1]\nPath=missing\n"
	row.FirefoxINI = &ini
	add(row)
	row = makeRow("safari/legacy-fallback", "safari", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Files[0].Path = "Library/Cookies/Cookies.binarycookies"
	add(row)
	row = makeRow("safari/container-precedes-legacy", "safari", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Files = append(row.Files, migrationBrowserImportFile{Path: "Library/Cookies/Cookies.binarycookies", BytesHex: hex.EncodeToString(migrationBrowserImportSafari([]migrationBrowserImportCookie{cookie("ignored-legacy-session")}))})
	add(row)
	row = makeRow("safari/container-directory-falls-back", "safari", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Files[0].Path = "Library/Cookies/Cookies.binarycookies"
	row.Files = append(row.Files, migrationBrowserImportFile{Path: "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies", Mode: "directory"})
	add(row)
	row = makeRow("safari/invalid-format", "safari", nil)
	row.Files[0].BytesHex = hex.EncodeToString([]byte("not-cook"))
	add(row)
	row = makeRow("safari/unmatched-invalid-utf8-ignored", "safari", []migrationBrowserImportCookie{cookie("owned-native-session")})
	bad := cookie(string([]byte{0xff}))
	bad.Host = "other.fanbox.cc"
	row.Files[0].BytesHex = hex.EncodeToString(migrationBrowserImportSafari([]migrationBrowserImportCookie{bad, cookie("owned-native-session")}))
	add(row)
	row = makeRow("safari/unknown-profile", "safari", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Steps[0].Args = append(row.Steps[0].Args, "--profile", "default")
	add(row)
	key := []byte("owned-aes-key-16")
	state := func(key []byte) migrationBrowserImportFile {
		body, _ := json.Marshal(map[string]any{"os_crypt": map[string]any{"encrypted_key": base64.StdEncoding.EncodeToString(key)}})
		return migrationBrowserImportFile{Path: "$ROOT/Local State", BytesHex: hex.EncodeToString(body)}
	}
	encryptedRow := func(name, browser string, value []byte) migrationBrowserImportCase {
		c := cookie("plaintext-private-canary")
		c.EncryptedHex = migrationBrowserImportGCM(value, key, "v10")
		r := makeRow(name, browser, []migrationBrowserImportCookie{c})
		r.Files = append(r.Files, state(key))
		r.SecretMode = "value"
		r.SecretHex = hex.EncodeToString([]byte("owned-password\r\n"))
		return r
	}
	for _, browser := range []string{"chrome", "edge"} {
		row = encryptedRow(browser+"/gcm-encrypted-precedes-plaintext", browser, []byte("owned-decrypted-session"))
		add(row)
	}
	row = encryptedRow("chrome/cbc-local-state-key", "chrome", []byte("unused"))
	row.Profiles[0].Cookies[0].EncryptedHex = migrationBrowserImportCBC([]byte("owned-cbc-session"), key, "v11")
	add(row)
	digest := sha256.Sum256([]byte(".fanbox.cc"))
	row = encryptedRow("chrome/host-digest-stripped", "chrome", append(digest[:], []byte("owned-digest-session")...))
	add(row)
	row = encryptedRow("chrome/host-digest-mismatch-preserved", "chrome", append(bytes.Repeat([]byte{'d'}, 32), []byte("owned-mismatch-session")...))
	add(row)
	row = encryptedRow("chrome/decrypted-invalid-utf8", "chrome", []byte{'x', 0xff, 'y'})
	add(row)
	row = encryptedRow("chrome/decrypted-nul", "chrome", []byte{'x', 0, 'y'})
	add(row)
	row = makeRow("chrome/plaintext-nul-shell-projection", "chrome", []migrationBrowserImportCookie{cookie(string([]byte{'x', 0, 'y'}))})
	add(row)
	for _, mode := range []string{"missing", "error", "empty"} {
		row = encryptedRow("chrome/secret-"+mode+"-precedes-malformed-state", "chrome", []byte("owned-decrypted-session"))
		row.SecretMode = mode
		row.Files[0].BytesHex = hex.EncodeToString([]byte("{"))
		add(row)
	}
	row = encryptedRow("chrome/secret-raw-whitespace-password-accepted", "chrome", []byte("owned-decrypted-session"))
	row.SecretHex = hex.EncodeToString([]byte(" \t \r\n"))
	add(row)
	for _, mode := range []string{"malformed-json", "invalid-base64", "dpapi-unknown", "bad-key-size", "malformed-ciphertext"} {
		row = encryptedRow("chrome/"+mode, "chrome", []byte("owned-decrypted-session"))
		switch mode {
		case "malformed-json":
			row.Files[0].BytesHex = hex.EncodeToString([]byte("{"))
		case "invalid-base64":
			row.Files[0].BytesHex = hex.EncodeToString([]byte(`{"os_crypt":{"encrypted_key":"!!"}}`))
		case "dpapi-unknown":
			row.Files[0] = state([]byte("DPAPIowned"))
		case "bad-key-size":
			row.Files[0] = state([]byte("bad"))
		case "malformed-ciphertext":
			row.Profiles[0].Cookies[0].EncryptedHex = hex.EncodeToString([]byte("v10bad"))
		}
		add(row)
	}
	row = makeRow("chrome/plaintext-avoids-secret-and-local-state", "chrome", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Files = append(row.Files, migrationBrowserImportFile{Path: "$ROOT/Local State", BytesHex: hex.EncodeToString([]byte("{"))})
	add(row)
	row = encryptedRow("chrome/unknown-version-precedes-secret", "chrome", []byte("unused"))
	row.Profiles[0].Cookies[0].EncryptedHex = hex.EncodeToString([]byte("v20owned-newer-format"))
	row.SecretMode = "missing"
	add(row)
	for _, mode := range []string{"factory", "proxy", "options", "identity"} {
		row = encryptedRow("chrome/raw-byte-order-"+mode, "chrome", []byte{'x', 0xff, 0, 'y'})
		switch mode {
		case "factory":
			row.Steps[0].FactoryError = true
		case "proxy":
			row.Steps[0].Args = append(row.Steps[0].Args, "--proxy=invalid", "--no-proxy")
		case "options":
			row.Steps[0].OptionsError = true
		case "identity":
			row.Steps[0].Identity = "unauthorized"
		}
		add(row)
	}
	row = makeRow("chrome/browser-error-precedes-factory-proxy", "chrome", nil)
	row.Profiles = []migrationBrowserImportProfile{}
	row.Steps[0].FactoryError = true
	row.Steps[0].Args = append(row.Steps[0].Args, "--proxy=invalid", "--no-proxy")
	add(row)
	row = encryptedRow("chrome/pre-canceled-discovery-no-command", "chrome", []byte("owned-decrypted-session"))
	row.Steps[0].Canceled = true
	add(row)
	row = makeRow("firefox/identity-failure-no-persist", "firefox", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Steps[0].Identity = "unauthorized"
	add(row)

	for _, vector := range []struct{ name, key, format string }{
		{"sha1000-gcm", "b539f6f7d953d148a3b24f84ac6196f28c49492bac0c62850548dd3ba7a643eb", "gcm"},
		{"saltysalt1003-cbc", "1578b9208a8261016ca8a653ad4a80e5", "cbc"},
		{"peanuts1-cbc", "e329e3e21634d46bc3e148c2093f988e", "cbc"},
	} {
		vectorKey, e := hex.DecodeString(vector.key)
		if e != nil {
			panic(e)
		}
		row = encryptedRow("chrome/native-password-"+vector.name, "chrome", []byte("unused"))
		row.Files = []migrationBrowserImportFile{}
		if vector.format == "gcm" {
			row.Profiles[0].Cookies[0].EncryptedHex = migrationBrowserImportGCM([]byte("owned-password-derived-session"), vectorKey, "v10")
		} else {
			row.Profiles[0].Cookies[0].EncryptedHex = migrationBrowserImportCBC([]byte("owned-password-derived-session"), vectorKey, "v11")
		}
		add(row)
	}
	wrappingKey, _ := hex.DecodeString("b539f6f7d953d148a3b24f84ac6196f28c49492bac0c62850548dd3ba7a643eb")
	wrappedHex := migrationBrowserImportGCM(key, wrappingKey, "v10")
	wrapped, _ := hex.DecodeString(wrappedHex)
	row = encryptedRow("chrome/native-password-unwraps-local-state", "chrome", []byte("owned-wrapped-state-session"))
	row.Files[0] = state(wrapped)
	add(row)
	row = encryptedRow("chrome/each-encrypted-row-acquires-password", "chrome", []byte("owned-decrypted-session"))
	row.Profiles[0].Cookies = append(row.Profiles[0].Cookies, row.Profiles[0].Cookies[0])
	add(row)
	row = makeRow("chrome/reimport-default-and-list", "chrome", []migrationBrowserImportCookie{cookie("owned-native-session")})
	row.Seed = []int64{1}
	config := "# retained owned comment\n[fanbox.auth]\ndefault_user_id = 1\n"
	row.ConfigBefore = &config
	first := row.Steps[0]
	second := first
	second.UserID = 42
	second.DisplayName = "Owned updated identity"
	second.Args = append(second.Args, "--default")
	list := migrationAuthFanboxStep{Args: []string{"fanbox", "auth", "list", "--json"}}
	row.Steps = []migrationAuthFanboxStep{first, second, list}
	add(row)
	return rows
}

func TestMigrationFanboxBrowserImport(t *testing.T) {
	sources, deps, depSources, stdlib, reused := migrationAuthFanboxSources(t)
	for _, p := range []string{"crates/pixiv-cli/tests/fixtures/fanbox-auth.json", "crates/pixiv-sdk/tests/fixtures/fanbox-opaque-credentials.json"} {
		body, e := os.ReadFile(filepath.Join("../..", p))
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			t.Fatal(e)
		}
		reused[p] = fmt.Sprintf("%x", sha256.Sum256(body))
	}
	support := map[string]string{}
	for _, p := range []string{"internal/cli/migration_fanbox_auth_test.go", "internal/cli/migration_fanbox_help_routing_test.go"} {
		body, e := os.ReadFile(filepath.Join("../..", p))
		if e != nil {
			t.Fatal(e)
		}
		support[p] = fmt.Sprintf("%x", sha256.Sum256(body))
	}
	sqlitePath := os.Getenv("MIGRATION_BROWSER_SQLITE_SOURCE")
	if sqlitePath == "" {
		t.Fatal("MIGRATION_BROWSER_SQLITE_SOURCE must identify verified official shell")
	}
	sqliteBytes, e := os.ReadFile(sqlitePath)
	if e != nil {
		t.Fatal(e)
	}
	if fmt.Sprintf("%x", sha256.Sum256(sqliteBytes)) != migrationBrowserImportSQLiteSHA256 {
		t.Fatal("SQLite executable identity changed")
	}
	version, e := exec.Command(sqlitePath, "-version").Output()
	if e != nil {
		t.Fatal(e)
	}
	fixture := migrationBrowserImportFixture{Reference: migrationAuthFanboxReference, Environment: runtime.GOOS + "/" + runtime.GOARCH, GoVersion: runtime.Version(), Sources: sources, Dependencies: deps, DependencySources: depSources, StdlibSources: stdlib, ReusedFixtures: reused, SupportSources: support, SQLiteTool: map[string]string{"version": strings.TrimSpace(string(version)), "sha256": migrationBrowserImportSQLiteSHA256}, IsolationScope: []string{
		"Each actual root/RunContext child owns HOME, USERPROFILE, all XDG/APPDATA/LOCALAPPDATA/TMPDIR and PATH; no real browser profile, cookie, account, credentials or keyring is read.",
		"Process-wide seccomp TSYNC denies socket, socketpair, connect and x32 syscalls; inherited by permitted child exec. Exec is deliberately not denied and no executable-allowlist sandbox claim is made.",
		"Owned PATH exposes only owned tests-only sqlite3/secret-tool wrapper symlinks to the disposable Go test executable. sqlite3 records fixed argv and map-semantic parameters then execs an owned byte-identical official SQLite3.54.0 binary; secret-tool records actual production lookup argv and emits synthetic fixture bytes only.",
		"No browsercookies.Register override, fixture read hook, encryption-key override or password-provider override is used. The observing reader forwards the genuine SystemBrowserProvider and records its raw session bytes without rewriting them.",
	}, Limitations: []string{
		"Linux/amd64 actual production native profile discovery, constrained official sqlite3 CSV query, Linux secret command/key acquisition, linked Go crypto, Safari source parser, SystemBrowserProvider, AccountService, SDK CurrentUser validation and real saved SQLite/config/output are connected. The ordinary SDK HTTP dependency receives only injected synthetic responses; network is denied.",
		"Secret Service availability/access cases use an owned synthetic secret-tool executable, not live DBus/libsecret or native desktop secret storage. Darwin Keychain, Windows DPAPI, native Safari/macOS execution and OS ACL/permissions remain outside this fixture.",
		"SQLite parameter map iteration is nondeterministic; actual wrapper validates and logs parameter-map semantics separately from stable non-parameter argv. Database paths and discovery paths replace only the exact owned home prefix with $HOME.",
		"Safari files follow the frozen source's interleaved BE page/record grammar, not a claim of full contemporary Safari binarycookies support. Chromium newer/app-bound versions remain frozen-source unsupported.",
		"Published fanbox-auth, saved-account and raw-byte SDK fixtures are reused unchanged for broader auth identity/persistence/output behavior. Real wall-clock generated database timestamps use the published interval assertions and symbolic interval projection only.",
		"All434 frozen production/module paths are unchanged; published fixture preservation is additionally checked in browser-import evidence. No Rust production/Cargo change or native platform parity is claimed by this Go capture.",
	}, Cases: migrationBrowserImportCases()}
	for i := range fixture.Cases {
		row := fixture.Cases[i]
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			body, e := json.Marshal(row)
			if e != nil {
				t.Fatal(e)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxBrowserImportChild$", "-test.count=1", "-test.timeout=18s")
			child.Dir = home
			child.WaitDelay = time.Second
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "XDG_CONFIG_HOME=" + filepath.Join(home, "xdg-config"), "XDG_DATA_HOME=" + filepath.Join(home, "xdg-data"), "XDG_CACHE_HOME=" + filepath.Join(home, "xdg-cache"), "XDG_STATE_HOME=" + filepath.Join(home, "xdg-state"), "XDG_RUNTIME_DIR=" + filepath.Join(home, "xdg-runtime"), "APPDATA=" + filepath.Join(home, "appdata"), "LOCALAPPDATA=" + filepath.Join(home, "local-appdata"), "TMPDIR=" + home, "PATH=" + filepath.Join(home, "bin"), "LANG=C", "LC_ALL=C", "TZ=UTC", "GOMAXPROCS=2", "GORACE=atexit_sleep_ms=0", "MIGRATION_BROWSER_NATIVE_HELPER=1", "MIGRATION_BROWSER_SQLITE_SOURCE=" + sqlitePath, "MIGRATION_FANBOX_BROWSER_IMPORT_CHILD=" + string(body)}
			if output, e := child.CombinedOutput(); e != nil {
				t.Fatalf("connected browser import child: %v\n%s", e, output)
			}
			result, e := os.ReadFile(filepath.Join(home, "result.json"))
			if e != nil {
				t.Fatal(e)
			}
			if e = json.Unmarshal(result, &fixture.Cases[i]); e != nil {
				t.Fatal(e)
			}
		})
	}
	migrationBrowserImportAssertCoverage(t, fixture.Cases)
	if t.Failed() {
		return
	}
	body, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	body = append(body, '\n')
	path := "../../crates/pixiv-cli/tests/fixtures/fanbox-browser-import.json"
	if *captureMigrationFanboxBrowserImport {
		if e = os.WriteFile(path, body, 0644); e != nil {
			t.Fatal(e)
		}
		return
	}
	want, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(body, want) {
		var old migrationBrowserImportFixture
		if e = json.Unmarshal(want, &old); e != nil {
			t.Fatal(e)
		}
		for i, row := range fixture.Cases {
			if i >= len(old.Cases) || !reflect.DeepEqual(row, old.Cases[i]) {
				t.Errorf("connected native observation changed: %s", row.Name)
			}
		}
		t.Fatal("connected saved-browser fixture bytes differ")
	}
}

func migrationBrowserImportAssertCoverage(t *testing.T, rows []migrationBrowserImportCase) {
	t.Helper()
	indexed := map[string]migrationBrowserImportObservation{}
	for _, row := range rows {
		if len(row.Observations) != len(row.Steps) {
			t.Errorf("missing connected observations: %s", row.Name)
			continue
		}
		indexed[row.Name] = row.Observations[0]
		for _, obs := range row.Observations {
			if !obs.SocketDenied || obs.ExecDenied || obs.StdinReads != 0 {
				t.Errorf("native fixture isolation/input scope changed: %s", row.Name)
			}
		}
	}
	require := func(name string) migrationBrowserImportObservation {
		obs, ok := indexed[name]
		if !ok {
			t.Fatalf("missing required native case %s", name)
		}
		return obs
	}
	for _, browser := range []string{"chrome", "edge", "firefox", "safari"} {
		obs := require(browser + "/exact-query-success")
		if obs.Exit != 0 || obs.NativeSessionError != "" || len(obs.Requests) != 1 || len(obs.After.Rows) != 1 {
			t.Errorf("genuine connected provider failed: %s", browser)
		}
	}
	for _, name := range []string{"chrome/gcm-encrypted-precedes-plaintext", "edge/gcm-encrypted-precedes-plaintext", "chrome/cbc-local-state-key", "chrome/native-password-sha1000-gcm", "chrome/native-password-saltysalt1003-cbc", "chrome/native-password-peanuts1-cbc", "chrome/native-password-unwraps-local-state"} {
		obs := require(name)
		if obs.Exit != 0 || len(obs.Commands) != 2 || obs.Commands[0].Program != "sqlite3" || obs.Commands[1].Program != "secret-tool" {
			t.Errorf("native crypto/key acquisition not exercised: %s", name)
		}
	}
	for _, item := range []struct{ name, hex string }{{"chrome/matched-invalid-utf8", "78ff79"}, {"edge/matched-invalid-utf8", "78ff79"}, {"chrome/decrypted-invalid-utf8", "78ff79"}, {"chrome/decrypted-nul", "780079"}, {"chrome/raw-byte-order-factory", "78ff0079"}, {"chrome/raw-byte-order-proxy", "78ff0079"}, {"chrome/raw-byte-order-options", "78ff0079"}, {"chrome/raw-byte-order-identity", "78ff0079"}} {
		obs := require(item.name)
		if obs.NativeSessionHex != item.hex || obs.NativeSessionError != "" || obs.Exit != 1 || len(obs.Requests) != 0 {
			t.Errorf("native raw-byte boundary rewritten or skipped: %s", item.name)
		}
	}
	for _, browser := range []string{"firefox", "safari"} {
		obs := require(browser + "/matched-invalid-utf8")
		if obs.NativeSessionHex != "" || obs.NativeSessionError != system.ErrEncryptedCookieUnsupported.Error() || strings.Contains(strings.Join(obs.Trace, "/"), "account.factory") {
			t.Errorf("provider-specific invalid UTF8 rejection missing: %s", browser)
		}
	}
	obs := require("chrome/plaintext-nul-shell-projection")
	if obs.Exit != 0 || obs.NativeSessionHex != "78" || len(obs.After.Rows) != 1 || obs.After.Rows[0]["session_hex"] != "78" {
		t.Error("official shell NUL text projection was not observed")
	}
	obs = require("chrome/each-encrypted-row-acquires-password")
	if len(obs.Commands) != 3 || obs.NativeSessionError != "browser profile contains multiple FANBOXSESSID cookies" {
		t.Error("encrypted rows did not separately acquire passwords")
	}
	obs = require("chrome/pre-canceled-discovery-no-command")
	if obs.NativeSessionError != "context canceled" || len(obs.Commands) != 0 {
		t.Error("pre-canceled native discovery executed child commands")
	}
}
