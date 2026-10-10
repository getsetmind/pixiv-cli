//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"runtime/debug"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies"
	fanboxauth "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox/auth"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	secret "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/secret"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var captureMigrationFanboxAuth = flag.Bool("migration-capture-fanbox-auth", false, "capture frozen Go FANBOX auth command contracts")

const migrationAuthFanboxReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type migrationAuthFanboxKey struct{}

type migrationAuthFanboxStep struct {
	Args                []string `json:"args"`
	InputHex            string   `json:"input_hex"`
	InputError          bool     `json:"input_error,omitempty"`
	CanPrompt           bool     `json:"can_prompt,omitempty"`
	PromptValue         string   `json:"prompt_value,omitempty"`
	PromptError         bool     `json:"prompt_error,omitempty"`
	Confirm             bool     `json:"confirm,omitempty"`
	BrowserValue        string   `json:"browser_value,omitempty"`
	BrowserError        bool     `json:"browser_error,omitempty"`
	SystemBrowserMode   string   `json:"system_browser_mode,omitempty"`
	FactoryError        bool     `json:"factory_error,omitempty"`
	FactoryNil          bool     `json:"factory_nil,omitempty"`
	StartupError        bool     `json:"startup_error,omitempty"`
	StartupRelay        bool     `json:"startup_relay,omitempty"`
	CloseError          bool     `json:"close_error,omitempty"`
	Writer              string   `json:"writer,omitempty"`
	StderrWriter        string   `json:"stderr_writer,omitempty"`
	RepositoryFailure   string   `json:"repository_failure,omitempty"`
	DefaultFailure      string   `json:"default_failure,omitempty"`
	DefaultFailNth      int      `json:"default_fail_nth,omitempty"`
	FileFailure         string   `json:"file_failure,omitempty"`
	Identity            string   `json:"identity,omitempty"`
	UserID              int64    `json:"user_id,omitempty"`
	DisplayName         string   `json:"display_name,omitempty"`
	CreatorID           string   `json:"creator_id,omitempty"`
	EmptyIdentityFields bool     `json:"empty_identity_fields,omitempty"`
	Canceled            bool     `json:"canceled,omitempty"`
	CancelOnRequest     bool     `json:"cancel_on_request,omitempty"`
	InjectSession       bool     `json:"inject_session,omitempty"`
	OptionsError        bool     `json:"options_error,omitempty"`
	Release             bool     `json:"release,omitempty"`
	UpdateMode          string   `json:"update_mode,omitempty"`
}
type migrationAuthFanboxCase struct {
	Name         string                           `json:"name"`
	ConfigBefore *string                          `json:"config_before"`
	Seed         []int64                          `json:"seed"`
	Steps        []migrationAuthFanboxStep        `json:"steps"`
	Observations []migrationAuthFanboxObservation `json:"observations"`
}
type migrationAuthFanboxWrite struct {
	Bytes string `json:"bytes"`
	N     int    `json:"n"`
	Error string `json:"error"`
}
type migrationAuthFanboxState struct {
	Config        *string           `json:"config"`
	Database      bool              `json:"database"`
	UserVersion   int               `json:"user_version"`
	ApplicationID int               `json:"application_id"`
	Rows          []map[string]any  `json:"rows"`
	Modes         map[string]string `json:"modes"`
}
type migrationAuthFanboxObservation struct {
	Exit         int                        `json:"exit"`
	Stdout       string                     `json:"stdout"`
	Stderr       string                     `json:"stderr"`
	OutputWrites []migrationAuthFanboxWrite `json:"output_writes"`
	ErrorWrites  []migrationAuthFanboxWrite `json:"error_writes"`
	Trace        []string                   `json:"trace"`
	Requests     []map[string]any           `json:"requests"`
	StdinReads   int                        `json:"stdin_reads"`
	StdinBytes   int                        `json:"stdin_bytes"`
	Before       migrationAuthFanboxState   `json:"before"`
	After        migrationAuthFanboxState   `json:"after"`
	SocketDenied bool                       `json:"socket_denied"`
	ExecDenied   bool                       `json:"exec_denied"`
}
type migrationAuthFanboxFixture struct {
	Reference         string                    `json:"reference"`
	Environment       string                    `json:"environment"`
	GoVersion         string                    `json:"go_version"`
	Sources           map[string]string         `json:"sources"`
	Dependencies      map[string]string         `json:"dependencies"`
	DependencySources map[string]string         `json:"dependency_sources"`
	StdlibSources     map[string]string         `json:"stdlib_sources"`
	ReusedFixtures    map[string]string         `json:"reused_fixtures"`
	Limitations       []string                  `json:"limitations"`
	Cases             []migrationAuthFanboxCase `json:"cases"`
}

type migrationAuthFanboxReader struct {
	value *bytes.Reader
	step  migrationAuthFanboxStep
	obs   *migrationAuthFanboxObservation
}

func (r *migrationAuthFanboxReader) Read(p []byte) (int, error) {
	r.obs.StdinReads++
	r.obs.Trace = append(r.obs.Trace, "stdin.read")
	if r.step.InputError {
		return 0, errors.New("owned fixture stdin failure")
	}
	n, e := r.value.Read(p)
	r.obs.StdinBytes += n
	return n, e
}

type migrationAuthFanboxWriter struct {
	mode, name string
	obs        *migrationAuthFanboxObservation
	bytes      bytes.Buffer
	writes     []migrationAuthFanboxWrite
}

func (w *migrationAuthFanboxWriter) Write(p []byte) (int, error) {
	n := len(p)
	var err error
	switch w.mode {
	case "short":
		n = 0
	case "error":
		n = 0
		err = errors.New("owned fixture writer failure")
	case "epipe":
		n = 0
		err = syscall.EPIPE
	case "partial-error":
		n = len(p) / 2
		err = errors.New("owned fixture writer failure")
	}
	if n > 0 {
		_, _ = w.bytes.Write(p[:n])
	}
	message := ""
	if err != nil {
		message = err.Error()
	}
	w.writes = append(w.writes, migrationAuthFanboxWrite{Bytes: string(p), N: n, Error: message})
	w.obs.Trace = append(w.obs.Trace, w.name+".write")
	return n, err
}
func migrationAuthFanboxContext(ctx context.Context) string {
	if ctx == nil {
		return "nil"
	}
	v := fmt.Sprint(ctx.Value(migrationAuthFanboxKey{}))
	if ctx.Err() != nil {
		v += "/" + ctx.Err().Error()
	}
	return v
}

type migrationAuthFanboxRepository struct {
	db   *database.DB
	obs  *migrationAuthFanboxObservation
	fail string
}

func (p *migrationAuthFanboxRepository) before(ctx context.Context, op string) error {
	p.obs.Trace = append(p.obs.Trace, "repository."+op+"/context="+migrationAuthFanboxContext(ctx))
	if p.fail == op {
		return errors.New("owned fixture repository " + op + " failure")
	}
	return nil
}
func (p *migrationAuthFanboxRepository) SaveFanboxCredential(ctx context.Context, a account.Account) error {
	if e := p.before(ctx, "save"); e != nil {
		return e
	}
	return p.db.SaveFanboxCredential(ctx, a)
}
func (p *migrationAuthFanboxRepository) RotateFanboxSession(ctx context.Context, id, rev int64, b []byte, at int64) error {
	if e := p.before(ctx, "rotate"); e != nil {
		return e
	}
	return p.db.RotateFanboxSession(ctx, id, rev, b, at)
}
func (p *migrationAuthFanboxRepository) ListFanbox(ctx context.Context) ([]account.Account, error) {
	if e := p.before(ctx, "list"); e != nil {
		return nil, e
	}
	return p.db.ListFanbox(ctx)
}
func (p *migrationAuthFanboxRepository) GetFanbox(ctx context.Context, id int64) (account.Account, error) {
	if e := p.before(ctx, "get"); e != nil {
		return account.Account{}, e
	}
	return p.db.GetFanbox(ctx, id)
}
func (p *migrationAuthFanboxRepository) RemoveFanbox(ctx context.Context, id int64) error {
	if e := p.before(ctx, "remove"); e != nil {
		return e
	}
	return p.db.RemoveFanbox(ctx, id)
}

type migrationAuthFanboxFiles struct {
	path string
	fail string
	obs  *migrationAuthFanboxObservation
}

func (f *migrationAuthFanboxFiles) Path() (string, error) {
	f.obs.Trace = append(f.obs.Trace, "config.path")
	if f.fail == "path" {
		return "", errors.New("owned fixture config path failure")
	}
	return f.path, nil
}
func (f *migrationAuthFanboxFiles) ReadFile(p string) ([]byte, error) {
	f.obs.Trace = append(f.obs.Trace, "config.read")
	if f.fail == "read" {
		return nil, errors.New("owned fixture config read failure")
	}
	return os.ReadFile(p)
}
func (f *migrationAuthFanboxFiles) WritePrivateFile(p string, b []byte) error {
	f.obs.Trace = append(f.obs.Trace, "config.write")
	if f.fail == "write" {
		return errors.New("owned fixture config write failure")
	}
	return secret.WritePrivateFile(p, b, 0600)
}
func (f *migrationAuthFanboxFiles) EnsurePrivateFile(string, []byte) error {
	f.obs.Trace = append(f.obs.Trace, "FORBIDDEN.config.ensure")
	return errors.New("auth must not ensure config")
}

type migrationAuthFanboxDefaults struct {
	store settings.Store
	step  migrationAuthFanboxStep
	obs   *migrationAuthFanboxObservation
	reads int
}

func (d *migrationAuthFanboxDefaults) ReadFanboxDefaultUserID() (int64, bool, error) {
	d.reads++
	d.obs.Trace = append(d.obs.Trace, "defaults.read")
	if d.step.DefaultFailure == "read" && (d.step.DefaultFailNth == 0 || d.step.DefaultFailNth == d.reads) {
		return 0, false, errors.New("owned fixture default read failure")
	}
	return d.store.ReadFanboxDefaultUserID()
}
func (d *migrationAuthFanboxDefaults) SetFanboxDefaultUserID(id int64) error {
	d.obs.Trace = append(d.obs.Trace, fmt.Sprintf("defaults.set/%d", id))
	if d.step.DefaultFailure == "set" {
		return errors.New("owned fixture default set failure")
	}
	return d.store.SetFanboxDefaultUserID(id)
}
func (d *migrationAuthFanboxDefaults) ClearFanboxDefaultUserID() error {
	d.obs.Trace = append(d.obs.Trace, "defaults.clear")
	if d.step.DefaultFailure == "clear" {
		return errors.New("owned fixture default clear failure")
	}
	return d.store.ClearFanboxDefaultUserID()
}

type migrationAuthFanboxBody struct {
	io.Reader
	obs  *migrationAuthFanboxObservation
	mode string
}

func (b *migrationAuthFanboxBody) Close() error {
	b.obs.Trace = append(b.obs.Trace, "identity.body.close")
	if b.mode == "close" {
		return errors.New("owned fixture identity close failure")
	}
	return nil
}

type migrationAuthFanboxIdentityRead struct{}

func (migrationAuthFanboxIdentityRead) Read([]byte) (int, error) {
	return 0, errors.New("owned fixture identity read failure")
}

type migrationAuthFanboxTransport struct {
	step   migrationAuthFanboxStep
	obs    *migrationAuthFanboxObservation
	cancel context.CancelFunc
}

func (p *migrationAuthFanboxTransport) CloseIdleConnections() {
	p.obs.Trace = append(p.obs.Trace, "identity.idle.close")
}
func (p *migrationAuthFanboxTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	p.obs.Trace = append(p.obs.Trace, "identity.request")
	p.obs.Requests = append(p.obs.Requests, map[string]any{"method": r.Method, "url": r.URL.String(), "cookie_hex": hex.EncodeToString([]byte(r.Header.Get("Cookie"))), "user_agent": r.Header.Get("User-Agent"), "accept": r.Header.Get("Accept"), "context": migrationAuthFanboxContext(r.Context())})
	if p.step.CancelOnRequest {
		p.cancel()
	}
	if p.step.Identity == "transport" {
		return nil, errors.New("owned fixture transport failure")
	}
	if r.Context().Err() != nil {
		return nil, r.Context().Err()
	}
	id := p.step.UserID
	if id == 0 {
		id = 42
	}
	name := p.step.DisplayName
	creator := p.step.CreatorID
	if name == "" && !p.step.EmptyIdentityFields {
		name = "  verified fixture  "
	}
	if creator == "" && !p.step.EmptyIdentityFields {
		creator = "fixture-creator"
	}
	metadata, _ := json.Marshal(map[string]any{"context": map[string]any{"user": map[string]any{"userId": id, "name": name, "creatorId": creator}}})
	if p.step.Identity == "zero" {
		metadata = []byte(`{"context":{"user":{"userId":0}}}`)
	}
	if p.step.Identity == "malformed" {
		metadata = []byte("{")
	}
	content := `<html><head><meta name="metadata" content='` + string(metadata) + `'></head></html>`
	if p.step.Identity == "missing" {
		content = "<html></html>"
	}
	reader := io.Reader(strings.NewReader(content))
	if p.step.Identity == "read" {
		reader = migrationAuthFanboxIdentityRead{}
	}
	status := 200
	if p.step.Identity == "unauthorized" {
		status = 401
	}
	if p.step.Identity == "forbidden" {
		status = 403
	}
	return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"text/html"}}, Body: &migrationAuthFanboxBody{Reader: reader, obs: p.obs, mode: p.step.Identity}}, nil
}

type migrationAuthFanboxBrowser struct {
	step migrationAuthFanboxStep
	obs  *migrationAuthFanboxObservation
}

func (b migrationAuthFanboxBrowser) ReadSession(ctx context.Context, browser, profile string) (string, error) {
	b.obs.Trace = append(b.obs.Trace, "browser.read/browser="+browser+"/profile="+profile+"/context="+migrationAuthFanboxContext(ctx))
	if b.step.BrowserError {
		return "", errors.New("owned fixture browser failure")
	}
	return b.step.BrowserValue, nil
}

type migrationAuthFanboxNativePort struct {
	mode string
	obs  *migrationAuthFanboxObservation
}

func (p *migrationAuthFanboxNativePort) Name() string { return "fixture-fanbox" }
func (p *migrationAuthFanboxNativePort) DiscoverProfiles(ctx context.Context) ([]browsercookies.Profile, error) {
	p.obs.Trace = append(p.obs.Trace, "provider.discover/context="+migrationAuthFanboxContext(ctx))
	if p.mode == "discover" || p.mode == "discover-close" {
		return nil, errors.New("owned fixture discovery failure")
	}
	if p.mode == "none" {
		return []browsercookies.Profile{}, nil
	}
	profiles := []browsercookies.Profile{{ID: "default", Path: "/owned-private-path-canary"}}
	if p.mode == "multiple" {
		profiles = append(profiles, browsercookies.Profile{ID: "secondary", Path: "/owned-private-path-canary-2"})
	}
	return profiles, nil
}
func (p *migrationAuthFanboxNativePort) Read(ctx context.Context, q browsercookies.CookieQuery, id string) ([]browsercookies.Secret, error) {
	p.obs.Trace = append(p.obs.Trace, "provider.read/host="+q.Host+"/name="+q.Name+"/profile="+id+"/context="+migrationAuthFanboxContext(ctx))
	if p.mode == "read" {
		return nil, errors.New("owned fixture cookie read failure")
	}
	if p.mode == "zero" {
		return []browsercookies.Secret{}, nil
	}
	s := []browsercookies.Secret{browsercookies.NewSecret("synthetic-browser-secret")}
	if p.mode == "cookies" {
		s = append(s, browsercookies.NewSecret("synthetic-browser-secret-2"))
	}
	return s, nil
}
func (p *migrationAuthFanboxNativePort) Close() error {
	p.obs.Trace = append(p.obs.Trace, "provider.close")
	if p.mode == "close" || p.mode == "discover-close" {
		return errors.New("owned fixture provider close failure")
	}
	return nil
}

func migrationAuthFanboxStateAt(t *testing.T, home string, start int64, created map[int64]int64) migrationAuthFanboxState {
	t.Helper()
	state := migrationAuthFanboxState{Rows: []map[string]any{}, Modes: map[string]string{}}
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if b, e := os.ReadFile(path); e == nil {
		s := string(b)
		state.Config = &s
	} else if !errors.Is(e, os.ErrNotExist) {
		t.Fatal(e)
	}
	for _, p := range []string{directory, path, database.DatabasePath(directory)} {
		if i, e := os.Stat(p); e == nil {
			r, _ := filepath.Rel(home, p)
			state.Modes[filepath.ToSlash(r)] = i.Mode().String()
		}
	}
	dbpath := database.DatabasePath(directory)
	if _, e := os.Stat(dbpath); errors.Is(e, os.ErrNotExist) {
		return state
	}
	state.Database = true
	db, e := sql.Open("sqlite", "file:"+filepath.ToSlash(dbpath)+"?mode=ro")
	if e != nil {
		t.Fatal(e)
	}
	defer db.Close()
	if e = db.QueryRow("PRAGMA user_version").Scan(&state.UserVersion); e != nil {
		t.Fatal(e)
	}
	if e = db.QueryRow("PRAGMA application_id").Scan(&state.ApplicationID); e != nil {
		t.Fatal(e)
	}
	rows, e := db.Query(`SELECT user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY sort_order,user_id`)
	if e != nil {
		t.Fatal(e)
	}
	defer rows.Close()
	for rows.Next() {
		var id, order, rev, valid, create, updated int64
		var name, creator string
		var session []byte
		if e = rows.Scan(&id, &order, &name, &creator, &session, &rev, &valid, &create, &updated); e != nil {
			t.Fatal(e)
		}
		if valid != 1700000000 && (valid < start || valid > time.Now().Unix()) {
			t.Fatalf("validation timestamp out of observed operation interval: %d", valid)
		}
		if create < start || create > time.Now().Unix() || updated < create || updated > time.Now().Unix() {
			t.Fatal("stored timestamps outside real operation interval")
		}
		same := true
		if old, ok := created[id]; ok {
			same = create == old
		} else {
			created[id] = create
		}
		if !same {
			t.Fatal("upsert changed creation timestamp")
		}
		state.Rows = append(state.Rows, map[string]any{"user_id": id, "sort_order": order, "display_name": name, "creator_id": creator, "session_hex": hex.EncodeToString(session), "credential_revision": rev, "validated_at": "positive-observed-unix-or-seed", "created_at": "within-observed-unix-interval", "updated_at": "within-observed-unix-interval", "creation_preserved": same})
	}
	if e = rows.Err(); e != nil {
		t.Fatal(e)
	}
	return state
}

func TestMigrationFanboxAuthChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_FANBOX_AUTH_CHILD")
	if encoded == "" {
		t.Skip("owned FANBOX auth child")
	}
	var row migrationAuthFanboxCase
	if e := json.Unmarshal([]byte(encoded), &row); e != nil {
		t.Fatal(e)
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || filepath.Dir(os.Getenv("XDG_CONFIG_HOME")) != home {
		t.Fatal("child must own home/XDG")
	}
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
	socketDenied, execDenied := migrationFanboxHelpDenyExternal(t)
	var nativeStep migrationAuthFanboxStep
	var nativeObs *migrationAuthFanboxObservation
	browsercookies.Register("fixture-fanbox", func() (browsercookies.Provider, error) {
		nativeObs.Trace = append(nativeObs.Trace, "provider.factory")
		return &migrationAuthFanboxNativePort{mode: nativeStep.SystemBrowserMode, obs: nativeObs}, nil
	})
	row.Observations = []migrationAuthFanboxObservation{}
	for _, step := range row.Steps {
		obs := migrationAuthFanboxObservation{OutputWrites: []migrationAuthFanboxWrite{}, ErrorWrites: []migrationAuthFanboxWrite{}, Trace: []string{}, Requests: []map[string]any{}, SocketDenied: socketDenied, ExecDenied: execDenied}
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
		fanboxBrowserSessionReader = migrationAuthFanboxBrowser{step: step, obs: &obs}
		if step.SystemBrowserMode != "" {
			nativeStep = step
			nativeObs = &obs
			fanboxBrowserSessionReader = fanboxauth.SystemBrowserProvider{}
		}
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
		for _, canary := range []string{home, "/owned-private-path-canary", "synthetic-browser-secret", "synthetic-seed-", "opaque-secret-canary"} {
			if strings.Contains(obs.Stdout+obs.Stderr, canary) {
				t.Fatal("private fixture path/secret leaked into exact CLI output")
			}
		}
		row.Observations = append(row.Observations, obs)
	}
	b, e := json.Marshal(row)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(filepath.Join(home, "result.json"), b, 0600); e != nil {
		t.Fatal(e)
	}
}

func migrationAuthFanboxSources(t *testing.T) (map[string]string, map[string]string, map[string]string, map[string]string, map[string]string) {
	t.Helper()
	sources := map[string]string{}
	deps := map[string]string{}
	depSources := map[string]string{}
	stdlib := map[string]string{}
	reused := map[string]string{}
	dirs := []string{"internal/cli", "internal/config", "internal/storage/database", "internal/storage/file/secret", "internal/services/fanbox", "internal/browsercookies", "sdk/fanbox", "sdk"}
	for _, dir := range dirs {
		e := filepath.WalkDir(filepath.Join("../..", dir), func(p string, d os.DirEntry, e error) error {
			if e != nil {
				return e
			}
			if d.IsDir() {
				return nil
			}
			if !strings.HasSuffix(p, ".go") || strings.HasSuffix(p, "_test.go") {
				return nil
			}
			rel, e := filepath.Rel("../..", p)
			if e != nil {
				return e
			}
			rel = filepath.ToSlash(rel)
			b, e := os.ReadFile(p)
			if e != nil {
				return e
			}
			frozen, e := exec.Command("git", "show", migrationAuthFanboxReference+":"+rel).Output()
			if e != nil {
				return e
			}
			if !bytes.Equal(b, frozen) {
				return fmt.Errorf("frozen production changed: %s", rel)
			}
			sources[rel] = fmt.Sprintf("%x", sha256.Sum256(b))
			return nil
		})
		if e != nil {
			t.Fatal(e)
		}
	}
	for _, p := range []string{"go.mod", "go.sum", "internal/shared/buildinfo/buildinfo.go", "internal/shared/network/client.go", "internal/storage/database/migrations/0001_initial.sql", "internal/storage/database/migrations/0002_fanbox_creator_id_not_null.sql"} {
		b, e := os.ReadFile(filepath.Join("../..", p))
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			t.Fatal(e)
		}
		f, e := exec.Command("git", "show", migrationAuthFanboxReference+":"+p).Output()
		if e != nil || !bytes.Equal(b, f) {
			t.Fatalf("frozen source %s: %v", p, e)
		}
		sources[p] = fmt.Sprintf("%x", sha256.Sum256(b))
	}
	info, ok := debug.ReadBuildInfo()
	if !ok {
		t.Fatal("build dependency metadata unavailable")
	}
	for _, d := range info.Deps {
		deps[d.Path] = d.Version + " " + d.Sum
	}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("GOMODCACHE must come from authorized toolchain environment")
	}
	for _, module := range []string{"github.com/spf13/cobra@v1.10.1", "github.com/spf13/pflag@v1.0.9", "github.com/creachadair/tomledit@v0.0.29", "modernc.org/sqlite@v1.40.1"} {
		root := filepath.Join(cache, module)
		if _, e := os.Stat(root); e != nil {
			if strings.HasPrefix(module, "modernc.org/sqlite") {
				matches, _ := filepath.Glob(filepath.Join(cache, "modernc.org/sqlite@*"))
				if len(matches) == 1 {
					root = matches[0]
					module = filepath.Base(root)
				}
			}
		}
		e := filepath.WalkDir(root, func(p string, d os.DirEntry, e error) error {
			if e != nil {
				return e
			}
			if d.IsDir() {
				return nil
			}
			if !strings.HasSuffix(p, ".go") || strings.HasSuffix(p, "_test.go") {
				return nil
			}
			b, e := os.ReadFile(p)
			if e != nil {
				return e
			}
			rel, _ := filepath.Rel(root, p)
			depSources[module+"/"+filepath.ToSlash(rel)] = fmt.Sprintf("%x", sha256.Sum256(b))
			return nil
		})
		if e != nil {
			t.Fatal(e)
		}
	}
	for _, dir := range []string{"context", "io", "encoding/json", "database/sql", "net/http", "strconv", "os"} {
		root := filepath.Join(runtime.GOROOT(), "src", dir)
		entries, e := os.ReadDir(root)
		if e != nil {
			t.Fatal(e)
		}
		for _, entry := range entries {
			if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") || strings.HasSuffix(entry.Name(), "_test.go") {
				continue
			}
			b, e := os.ReadFile(filepath.Join(root, entry.Name()))
			if e != nil {
				t.Fatal(e)
			}
			stdlib[dir+"/"+entry.Name()] = fmt.Sprintf("%x", sha256.Sum256(b))
		}
	}
	for _, p := range []string{"crates/pixiv-app/tests/fixtures/fanbox-saved-accounts.json", "crates/pixiv-cli/tests/fixtures/fanbox-help-routing.json"} {
		b, e := os.ReadFile(filepath.Join("../..", p))
		if e != nil {
			t.Fatal(e)
		}
		reused[p] = fmt.Sprintf("%x", sha256.Sum256(b))
	}
	return sources, deps, depSources, stdlib, reused
}

func TestMigrationFanboxAuth(t *testing.T) {
	sources, deps, depSources, stdlib, reused := migrationAuthFanboxSources(t)
	rows := migrationAuthFanboxCases()
	fixture := migrationAuthFanboxFixture{Reference: migrationAuthFanboxReference, Environment: runtime.GOOS + "/" + runtime.GOARCH, GoVersion: runtime.Version(), Sources: sources, Dependencies: deps, DependencySources: depSources, StdlibSources: stdlib, ReusedFixtures: reused, Limitations: []string{
		"Actual RunContext constructs the production root, fanbox group and auth.New; the account factory injects genuine Service, SQLite, sparse settings.Store and standard HTTP transport dependency ports, never a CurrentUser DTO.",
		"Disposable Go test child entrypoints are used instead of separately built cmd/pixiv executables; process-wide socket/connect/exec denial, owned HOME/USERPROFILE/XDG and synthetic secret values prevent real browser, authenticated network, association, registry and media activity.",
		"SystemBrowserProvider uses a registered synthetic provider for adapter discovery/query/profile/close observations. All four live native browser providers, OS keychain/Secret Service/DPAPI, OS filesystem/ACL and live cookie extraction remain unverified and outside this fixture.",
		"SQLite wall-clock timestamps are asserted within each observed operation interval, positivity and retained created_at, then represented by named interval observations; session BLOB bytes, revisions, order, config bytes, modes, requests, traces, writes, stderr and exit remain exact.",
		"Linux/amd64 Go capture only; no Rust parity, platform-native or final migration completion claim. The denied supplemental native multiplex/unfinished-HEAD/upload probe was not reconstructed or retried.",
		"Protected 434 frozen Go production/module paths and preexisting fixtures are sealed in the accompanying provenance manifest. Existing saved-account321 and help22 bytes are reused unchanged.",
	}, Cases: rows}
	for i := range fixture.Cases {
		row := fixture.Cases[i]
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			b, e := json.Marshal(row)
			if e != nil {
				t.Fatal(e)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxAuthChild$", "-test.count=1", "-test.timeout=12s")
			child.Dir = home
			child.WaitDelay = time.Second
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "XDG_CONFIG_HOME=" + filepath.Join(home, "xdg-config"), "XDG_DATA_HOME=" + filepath.Join(home, "xdg-data"), "XDG_CACHE_HOME=" + filepath.Join(home, "xdg-cache"), "XDG_STATE_HOME=" + filepath.Join(home, "xdg-state"), "XDG_RUNTIME_DIR=" + filepath.Join(home, "xdg-runtime"), "APPDATA=" + filepath.Join(home, "appdata"), "LOCALAPPDATA=" + filepath.Join(home, "local-appdata"), "TMPDIR=" + home, "PATH=", "LANG=C", "LC_ALL=C", "TZ=UTC", "GOMAXPROCS=2", "GORACE=atexit_sleep_ms=0", "MIGRATION_FANBOX_AUTH_CHILD=" + string(b)}
			if output, e := child.CombinedOutput(); e != nil {
				t.Fatalf("owned auth child: %v\n%s", e, output)
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
	if t.Failed() {
		return
	}
	b, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	b = append(b, '\n')
	p := "../../crates/pixiv-cli/tests/fixtures/fanbox-auth.json"
	if *captureMigrationFanboxAuth {
		if e = os.WriteFile(p, b, 0644); e != nil {
			t.Fatal(e)
		}
		return
	}
	want, e := os.ReadFile(p)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(b, want) {
		var old migrationAuthFanboxFixture
		if e = json.Unmarshal(want, &old); e != nil {
			t.Fatal(e)
		}
		for i, row := range fixture.Cases {
			if i >= len(old.Cases) || !reflect.DeepEqual(row, old.Cases[i]) {
				t.Errorf("frozen auth observation changed: %s", row.Name)
			}
		}
		t.Fatal("FANBOX auth fixture bytes differ")
	}
}

func migrationAuthFanboxCases() []migrationAuthFanboxCase {
	rows := []migrationAuthFanboxCase{}
	raw := func(args ...string) migrationAuthFanboxStep {
		return migrationAuthFanboxStep{Args: append([]string{"fanbox", "auth"}, args...), InputHex: hex.EncodeToString([]byte("opaque-secret-canary\n"))}
	}
	add := func(name string, step migrationAuthFanboxStep) *migrationAuthFanboxCase {
		rows = append(rows, migrationAuthFanboxCase{Name: name, Seed: []int64{}, Steps: []migrationAuthFanboxStep{step}, Observations: []migrationAuthFanboxObservation{}})
		return &rows[len(rows)-1]
	}
	ptr := func(s string) *string { return &s }
	for _, leaf := range []string{"import", "list", "status", "use", "remove"} {
		add("help/"+leaf, raw(leaf, "--help"))
	}
	add("route/auth-bare", raw())
	add("route/auth-extra", raw("extra"))
	add("route/unknown-child", raw("unknown"))
	add("route/help-bypasses-args", raw("remove", "1", "2", "--help"))
	for _, args := range [][]string{{"import", "cookie"}, {"list", "1"}, {"status", "1", "2"}, {"use", "1", "2"}, {"remove"}, {"remove", "1", "2"}, {"list", "--unknown", "--json"}, {"use", "--ndjson"}, {"import", "--profile"}, {"import", "--from-browser"}, {"list", "--proxy"}, {"remove", "--yes=invalid"}, {"use", "--auto=invalid"}, {"list", "--json=invalid"}, {"status", "--no-proxy=invalid"}} {
		name := "parse/" + strings.Join(args, "_")
		s := raw(args...)
		s.InputHex = ""
		add(name, s)
	}
	for _, args := range [][]string{{"--json", "list"}, {"list", "--json=true"}, {"list", "--json=false"}, {"list", "--json", "false"}, {"use", "--auto=true", "--json"}, {"use", "--auto=false"}, {"remove", "1", "--yes=false"}, {"remove", "1", "--yes=true"}, {"status", "--", "+0001"}, {"status", "--", "--help"}, {"--proxy", "auth", "list", "--help"}, {"list", "--no-proxy=false"}} {
		s := raw(args...)
		s.InputHex = ""
		add("flags/"+strings.Join(args, "_"), s)
	}
	s := raw("list")
	s.Args = []string{"fanbox", "--proxy=auth", "auth", "list", "--help"}
	add("route/proxy-value-before-auth", s)
	s = raw("use", "--auto")
	s.Args = []string{"fanbox", "auth", "--auto", "use"}
	s.InputHex = ""
	add("route/local-bool-before-leaf", s)
	for _, id := range []string{"+0001", " 1 ", "0", "-1", "0x1", "9223372036854775808", "1\n2"} {
		for _, leaf := range []string{"status", "use", "remove"} {
			s := raw(leaf, "--", id)
			s.InputHex = ""
			row := add("uid/"+leaf+"/"+hex.EncodeToString([]byte(id)), s)
			row.Seed = []int64{1}
		}
	}
	for _, input := range []string{"opaque-secret-canary", "opaque-secret-canary\n", "opaque-secret-canary\r\n", "opaque-secret-canary\n\n", "opaque-secret-canary\r", "opaque\nsecret", "", "\n", " \t\n", "  opaque-secret-canary  \n", "FANBOXSESSID=opaque-secret-canary; other=x\n", string([]byte{'x', 0xff, 'y', '\n'})} {
		s := raw("import", "--json")
		s.InputHex = hex.EncodeToString([]byte(input))
		add("stdin/import/"+s.InputHex, s)
	}
	s = raw("import")
	s.InputError = true
	add("stdin/import-error", s)
	for _, leaf := range []string{"status", "use", "remove"} {
		s := raw(leaf)
		s.InputError = true
		add("stdin/"+leaf+"-error", s)
		s = raw(leaf)
		s.InputHex = hex.EncodeToString([]byte("+0001\r\n"))
		row := add("stdin/"+leaf+"-uid", s)
		row.Seed = []int64{1}
	}
	s = raw("use", "--auto")
	s.InputHex = hex.EncodeToString([]byte("1\n"))
	s.FactoryError = true
	add("order/auto-consumes-piped-uid-before-factory", s)
	s = raw("status", "1")
	s.InputError = true
	row := add("stdin/explicit-uid-does-not-read", s)
	row.Seed = []int64{1}
	for _, mode := range []string{"success", "empty", "whitespace", "multiline", "error"} {
		s := raw("import", "--json")
		s.CanPrompt = true
		s.PromptValue = "opaque-secret-canary"
		switch mode {
		case "empty":
			s.PromptValue = ""
		case "whitespace":
			s.PromptValue = " \t "
		case "multiline":
			s.PromptValue = "opaque\nsecret"
		case "error":
			s.PromptError = true
		}
		add("prompt/import/"+mode, s)
	}
	for _, mode := range []string{"success", "error", "empty"} {
		s := raw("import", "--from-browser", " ChRoMe ", "--profile", "safe-profile", "--json")
		s.CanPrompt = true
		s.InputError = true
		s.BrowserValue = "synthetic-browser-secret"
		if mode == "error" {
			s.BrowserError = true
		}
		if mode == "empty" {
			s.BrowserValue = ""
		}
		add("browser/dispatch/"+mode, s)
	}
	s = raw("import", "--profile", "ignored-profile")
	add("browser/profile-alone-raw", s)
	s = raw("import", "--from-browser=", "--profile", "ignored-profile")
	add("browser/empty-browser-raw", s)
	for _, mode := range []string{"success", "none", "multiple", "zero", "cookies", "discover", "read", "close", "discover-close"} {
		s := raw("import", "--from-browser", " FiXtUrE-FaNbOx ", "--json")
		s.SystemBrowserMode = mode
		add("browser/system-adapter/"+mode, s)
	}
	for _, profile := range []string{"secondary", "/owned-private-path-canary"} {
		s := raw("import", "--from-browser", "fixture-fanbox", "--profile", profile)
		s.SystemBrowserMode = "multiple"
		add("browser/system-profile/"+hex.EncodeToString([]byte(profile)), s)
	}
	s = raw("import", "--from-browser", "unsupported", "--json")
	s.SystemBrowserMode = "success"
	add("browser/system-unknown", s)
	for _, leaf := range []string{"list", "status", "import", "use", "remove"} {
		args := []string{leaf}
		if leaf == "use" || leaf == "remove" {
			args = append(args, "1")
		}
		args = append(args, "--proxy=bad", "--no-proxy=false", "--json")
		s := raw(args...)
		if leaf != "import" {
			s.InputHex = ""
		}
		row := add("proxy/conflict/"+leaf, s)
		row.Seed = []int64{1}
	}
	for _, leaf := range []string{"status", "use", "remove"} {
		s := raw(leaf, "invalid", "--json")
		s.FactoryError = true
		add("order/invalid-uid-factory/"+leaf, s)
	}
	s = raw("import", "--proxy=bad", "--no-proxy")
	s.FactoryError = true
	add("order/import-secret-factory-proxy", s)
	s = raw("import", "--proxy=bad", "--no-proxy")
	s.InputHex = ""
	s.FactoryError = true
	add("order/import-empty-secret-before-factory", s)
	s = raw("import", "--from-browser", "chrome", "--proxy=bad", "--no-proxy")
	s.BrowserError = true
	s.FactoryError = true
	add("order/browser-error-before-factory-proxy", s)
	s = raw("list", "--proxy=bad", "--no-proxy")
	s.FactoryError = true
	add("order/list-proxy-before-factory", s)
	for _, leaf := range []string{"import", "list", "status", "use", "remove"} {
		args := []string{leaf, "--json"}
		if leaf == "use" || leaf == "remove" {
			args = []string{leaf, "1", "--json"}
		}
		s := raw(args...)
		s.StartupError = true
		add("order/startup-before-leaf/"+leaf, s)
	}
	s = raw("remove")
	s.InputHex = ""
	s.StartupError = true
	add("order/arity-before-startup", s)
	s = raw("list")
	s.StartupRelay = true
	add("startup/relay-warning-success", s)
	s = raw("list", "--json")
	s.FactoryNil = true
	add("factory/nil-service", s)
	for _, leaf := range []string{"list", "status", "use", "remove"} {
		args := []string{leaf, "--json"}
		if leaf == "use" || leaf == "remove" {
			args = []string{leaf, "1", "--json"}
		}
		s := raw(args...)
		s.InputHex = ""
		s.Canceled = true
		row := add("context/background/"+leaf, s)
		row.Seed = []int64{1}
	}
	for _, mode := range []string{"canceled", "cancel-request"} {
		s := raw("import", "--json")
		s.Canceled = mode == "canceled"
		s.CancelOnRequest = mode == "cancel-request"
		add("context/import/"+mode, s)
	}
	for _, mode := range []string{"unauthorized", "forbidden", "malformed", "missing", "zero", "transport", "read", "close"} {
		s := raw("import", "--json")
		s.Identity = mode
		row := add("verification/"+mode, s)
		row.Seed = []int64{1}
		row.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 1\n")
	}
	for _, failure := range []string{"save", "default-read", "default-second-read", "default-set", "file-read", "file-write", "options"} {
		s := raw("import", "--json")
		switch failure {
		case "save":
			s.RepositoryFailure = "save"
		case "default-read":
			s.DefaultFailure = "read"
		case "default-second-read":
			s.DefaultFailure = "read"
			s.DefaultFailNth = 2
		case "default-set":
			s.DefaultFailure = "set"
		case "file-read":
			s.FileFailure = "read"
		case "file-write":
			s.FileFailure = "write"
		case "options":
			s.OptionsError = true
		}
		add("import/failure/"+failure, s)
	}
	s = raw("import", "--proxy=bad", "--json")
	s.InjectSession = true
	s.OptionsError = true
	add("import/injected-session-bypasses-options-proxy-validation", s)
	s = raw("import", "--json")
	s.EmptyIdentityFields = true
	add("verification/import-rejects-empty-display-name", s)
	for _, cfg := range []string{"[fanbox.auth]\ndefault_user_id = 999\n", "[fanbox.auth]\ndefault_user_id = 'bad'\n", "[unfinished\n", "[fanbox.auth]\ndefault_user_id = 1.0\n"} {
		for _, leaf := range []string{"list", "status", "import"} {
			s := raw(leaf, "--json")
			row := add("config/"+leaf+"/"+hex.EncodeToString([]byte(cfg)), s)
			row.ConfigBefore = ptr(cfg)
			row.Seed = []int64{1}
		}
	}
	s = raw("list", "--json")
	row = add("config/empty-list-skips-malformed-default", s)
	row.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 'bad'\n")
	s = raw("status", "1", "--json")
	row = add("config/stale-explicit-status-uid-succeeds", s)
	row.Seed = []int64{1}
	row.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 999\n")
	for _, mode := range []string{"cancel", "error", "accept", "yes", "yes-false"} {
		s := raw("remove", "1", "--json")
		s.CanPrompt = true
		s.Confirm = mode == "accept"
		s.PromptError = mode == "error"
		if mode == "yes" {
			s.Args = append(s.Args, "--yes")
		}
		if mode == "yes-false" {
			s.Args = append(s.Args, "--yes=false")
		}
		row := add("prompt/remove/"+mode, s)
		row.Seed = []int64{1}
	}
	s = raw("remove", "1")
	s.CanPrompt = true
	s.Confirm = true
	row = add("order/remove-prompts-before-explicit-default-guard", s)
	row.Seed = []int64{1}
	row.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 1\n")
	for _, leaf := range []string{"list", "status", "use", "remove"} {
		s := raw(leaf, "--json")
		if leaf == "use" || leaf == "remove" {
			s.Args = append(s.Args, "1")
		}
		s.InputHex = ""
		s.DefaultFailure = "read"
		row := add("failure/default-read/"+leaf, s)
		row.Seed = []int64{1}
	}
	s = raw("use", "1", "--json")
	s.DefaultFailure = "set"
	row = add("failure/use-set", s)
	row.Seed = []int64{1}
	s = raw("use", "--auto", "--json")
	s.InputHex = ""
	s.DefaultFailure = "clear"
	add("failure/use-auto-clear", s)
	for _, leaf := range []string{"list", "status", "use", "remove"} {
		s := raw(leaf, "--json")
		if leaf == "use" || leaf == "remove" {
			s.Args = append(s.Args, "1")
		}
		s.InputHex = ""
		s.RepositoryFailure = map[string]string{"list": "list", "status": "get", "use": "get", "remove": "remove"}[leaf]
		row := add("failure/repository/"+leaf, s)
		row.Seed = []int64{1}
		if leaf == "status" {
			row.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 1\n")
		}
	}
	for _, leaf := range []string{"import", "list", "status", "use", "remove"} {
		for _, format := range []string{"text", "json"} {
			for _, mode := range []string{"error", "short", "epipe"} {
				args := []string{leaf}
				if leaf == "use" || leaf == "remove" {
					args = append(args, "1")
				}
				if format == "json" {
					args = append(args, "--json")
				}
				s := raw(args...)
				s.Writer = mode
				row := add("writer/"+leaf+"/"+format+"/"+mode, s)
				row.Seed = []int64{1}
			}
		}
	}
	s = raw("import", "--json")
	s.Writer = "partial-error"
	s.CloseError = true
	add("cleanup/output-failure-joined-db-close", s)
	s = raw("import", "--json")
	s.Identity = "unauthorized"
	s.CloseError = true
	add("cleanup/verification-failure-joined-db-close", s)
	s = raw("list", "--json=false")
	s.FactoryError = true
	add("errors/json-false-machine-envelope", s)
	s = raw("list", "--json")
	s.FactoryError = true
	s.StderrWriter = "error"
	add("errors/stderr-envelope-fallback-attempt", s)
	s = raw("list")
	s.CloseError = true
	add("cleanup/success-output-before-db-close-error", s)
	for _, mode := range []string{"factory", "runtime-error", "disabled", "conflict", "false-direct"} {
		s := raw("use", "--auto")
		s.InputHex = ""
		s.Release = true
		s.UpdateMode = mode
		if mode == "conflict" {
			s.Args = append(s.Args, "--proxy=bad", "--no-proxy")
		}
		if mode == "false-direct" {
			s.Args = append(s.Args, "--no-proxy=false")
		}
		add("update/post-success/"+mode, s)
	}

	for _, value := range []string{"1", "0", "t", "f", "T", "F", "TRUE", "FALSE"} {
		s = raw("list", "--json="+value)
		s.InputHex = ""
		add("flags/bool-spelling/json="+value, s)
	}
	s = raw("import", "--json")
	s.DisplayName = "verified without creator"
	s.EmptyIdentityFields = true
	add("output/import-omits-empty-creator", s)
	s = raw("import", "--json")
	s.DisplayName = "  <tag>&\"日本語  "
	s.CreatorID = "safe-creator"
	add("output/import-json-escapes-display", s)

	for _, value := range []string{"true", "false"} {
		s = raw("import", "--default="+value, "--json")
		r := add("flags/import-default="+value, s)
		r.Seed = []int64{1}
		r.ConfigBefore = ptr("[fanbox.auth]\ndefault_user_id = 1\n")
	}
	s = raw("import", "--default", "false")
	s.InputHex = ""
	add("flags/import-default-space-is-positional", s)
	s = raw("import", "--json")
	s.Args = []string{"fanbox", "auth", "--from-browser", "chrome", "import", "--json"}
	s.BrowserValue = "synthetic-browser-secret"
	add("route/browser-value-before-import", s)
	// The workflow is one owned database/config lifecycle rather than unrelated mocked leaf responses.
	sequence := []migrationAuthFanboxStep{}
	addStep := func(s migrationAuthFanboxStep) {
		if len(s.Args) > 2 && s.Args[2] != "import" {
			s.InputHex = ""
		}
		sequence = append(sequence, s)
	}
	addStep(raw("list", "--json"))
	addStep(raw("status", "--json"))
	addStep(raw("import", "--json"))
	addStep(raw("list"))
	addStep(raw("status"))
	s = raw("import", "--json")
	s.UserID = 7
	s.DisplayName = "second fixture"
	s.EmptyIdentityFields = true
	addStep(s)
	addStep(raw("list", "--json"))
	addStep(raw("status", "7", "--json"))
	addStep(raw("use", "7", "--json"))
	addStep(raw("remove", "7", "--yes", "--json"))
	addStep(raw("use", "--auto", "--json"))
	addStep(raw("remove", "42", "--yes", "--json"))
	addStep(raw("status", "--json"))
	s = raw("import", "--default", "--json")
	s.UserID = 7
	s.DisplayName = "replacement"
	s.CreatorID = "replacement-creator"
	s.InputHex = hex.EncodeToString([]byte("synthetic-replacement\n"))
	addStep(s)
	addStep(raw("list", "--json"))
	addStep(raw("use", "--auto"))
	addStep(raw("remove", "7", "--yes"))
	addStep(raw("list", "--json"))
	addStep(raw("use", "--auto", "--json"))
	rows = append(rows, migrationAuthFanboxCase{Name: "workflow/import-list-status-use-auto-remove-reimport", Seed: []int64{}, Steps: sequence, Observations: []migrationAuthFanboxObservation{}})
	row = add("config/use-auto-empty-absent-file", raw("use", "--auto", "--json"))
	row.Steps[0].InputHex = ""
	row = add("config/preserve-comments-unknown-keys", raw("import", "--json"))
	row.ConfigBefore = ptr("# preserved leading comment\nunknown = 'keep'\n[fanbox.auth]\n# selection comment\ndefault_user_id = 999\ncustom = true\n")
	s = raw("use", "42", "--json")
	s.InputHex = ""
	row.Steps = append(row.Steps, s)
	s = raw("use", "--auto", "--json")
	s.InputHex = ""
	row.Steps = append(row.Steps, s)
	row = add("workflow/failed-reimport-preserves-existing-credential", raw("import", "--json"))
	s = raw("import", "--json")
	s.Identity = "unauthorized"
	s.InputHex = hex.EncodeToString([]byte("synthetic-rejected-new-secret\n"))
	row.Steps = append(row.Steps, s)
	s = raw("list", "--json")
	s.InputHex = ""
	row.Steps = append(row.Steps, s)
	sort.Slice(rows, func(i, j int) bool { return rows[i].Name < rows[j].Name })
	return rows
}
