//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"database/sql"
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
	"regexp"
	"strings"
	"syscall"
	"testing"
	"time"

	fanboxcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox"
	fanboxpost "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox/post"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	fanboxapp "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox"
	fanboxaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
	"github.com/spf13/cobra"
)

var captureFanboxContentReads = flag.Bool("migration-capture-fanbox-content-reads", false, "capture genuine saved FANBOX content CLI reads")

const fanboxContentReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type fanboxContentReply struct {
	Body           string `json:"body"`
	Status         int    `json:"status,omitempty"`
	TransportError string `json:"transport_error,omitempty"`
	ReadError      string `json:"read_error,omitempty"`
	CloseError     string `json:"close_error,omitempty"`
}
type fanboxContentInput struct {
	Boundary     string               `json:"boundary"`
	Args         []string             `json:"args"`
	Config       string               `json:"config"`
	Saved        string               `json:"saved"`
	DBFailure    string               `json:"db_failure,omitempty"`
	Stdin        string               `json:"stdin,omitempty"`
	StdinError   bool                 `json:"stdin_error,omitempty"`
	Replies      []fanboxContentReply `json:"replies"`
	Writer       string               `json:"writer,omitempty"`
	WriteLimit   int                  `json:"write_limit,omitempty"`
	CloseError   bool                 `json:"close_error,omitempty"`
	OptionsError bool                 `json:"options_error,omitempty"`
	Cancel       string               `json:"cancel,omitempty"`
	BadTemp      bool                 `json:"bad_temp,omitempty"`
	OutputPipe   bool                 `json:"output_pipe,omitempty"`
	Repeat       int                  `json:"repeat,omitempty"`
}
type fanboxContentRequest struct {
	Method       string      `json:"method"`
	URL          string      `json:"url"`
	Header       http.Header `json:"headers"`
	ContextError string      `json:"context_error"`
}
type fanboxContentObservation struct {
	Exits          []int                  `json:"exits"`
	Stdout         string                 `json:"stdout"`
	Stderr         string                 `json:"stderr"`
	Errors         []string               `json:"errors"`
	Reasons        []string               `json:"reasons"`
	Trace          []string               `json:"trace"`
	Requests       []fanboxContentRequest `json:"requests"`
	Options        []map[string]any       `json:"options"`
	ProxyOverrides []*string              `json:"proxy_overrides"`
	StdinReads     int                    `json:"stdin_reads"`
	Writes         []int                  `json:"writes"`
	BodyCloses     int                    `json:"body_closes"`
	LeaseCloses    int                    `json:"lease_closes"`
	UniqueClients  int                    `json:"unique_clients"`
	OutputIsTTY    bool                   `json:"output_is_tty"`
	AutoNDJSON     bool                   `json:"auto_ndjson"`
	IdleCloses     int                    `json:"idle_closes"`
	DBBefore       string                 `json:"db_before"`
	DBAfter        string                 `json:"db_after"`
	DBRowsBefore   []string               `json:"db_rows_before"`
	DBRowsAfter    []string               `json:"db_rows_after"`
	ConfigAfter    string                 `json:"config_after"`
	Temps          []string               `json:"remaining_temps"`
	SocketDenied   bool                   `json:"socket_denied"`
	ExecDenied     bool                   `json:"exec_denied"`
}
type fanboxContentCase struct {
	Name        string                   `json:"name"`
	Input       fanboxContentInput       `json:"input"`
	Observation fanboxContentObservation `json:"observation"`
}
type fanboxContentReuse struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
	Cases  int    `json:"cases"`
}
type fanboxContentFixture struct {
	Reference          string               `json:"reference"`
	Base               string               `json:"published_base"`
	Environment        string               `json:"environment"`
	Sources            map[string]string    `json:"sources"`
	FrozenGoProduction map[string]string    `json:"frozen_go_production"`
	Reused             []fanboxContentReuse `json:"reused"`
	Limitations        []string             `json:"limitations"`
	Cases              []fanboxContentCase  `json:"cases"`
}

type fanboxContentReader struct {
	input       *strings.Reader
	observation *fanboxContentObservation
	fail        bool
}

func (r *fanboxContentReader) Read(p []byte) (int, error) {
	r.observation.StdinReads++
	if r.fail {
		return 0, errors.New("owned stdin failure")
	}
	return r.input.Read(p)
}

type fanboxContentWriter struct {
	observation *fanboxContentObservation
	buffer      bytes.Buffer
	remaining   int
	mode        string
}

type fanboxContentPipeWriter struct {
	writer *fanboxContentWriter
	pipe   *os.File
}

func (w fanboxContentPipeWriter) Fd() uintptr { return w.pipe.Fd() }
func (w fanboxContentPipeWriter) Write(p []byte) (int, error) {
	w.writer.observation.Writes = append(w.writer.observation.Writes, len(p))
	return w.pipe.Write(p)
}

func (w *fanboxContentWriter) Write(p []byte) (int, error) {
	w.observation.Writes = append(w.observation.Writes, len(p))
	if w.mode == "" {
		return w.buffer.Write(p)
	}
	if len(p) <= w.remaining {
		w.remaining -= len(p)
		return w.buffer.Write(p)
	}
	p = p[:w.remaining]
	w.remaining = 0
	n, _ := w.buffer.Write(p)
	switch w.mode {
	case "pipe":
		return n, syscall.EPIPE
	case "short":
		return n, nil
	default:
		return n, errors.New("owned writer failure")
	}
}

type fanboxContentTransport struct {
	observation *fanboxContentObservation
	input       fanboxContentInput
	index       int
	cancel      context.CancelFunc
}

func (r *fanboxContentTransport) CloseIdleConnections() {
	r.observation.IdleCloses++
	r.observation.Trace = append(r.observation.Trace, "transport.idle-close")
}
func (r *fanboxContentTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	ctxErr := ""
	if req.Context().Err() != nil {
		ctxErr = req.Context().Err().Error()
	}
	r.observation.Requests = append(r.observation.Requests, fanboxContentRequest{Method: req.Method, URL: req.URL.String(), Header: req.Header.Clone(), ContextError: ctxErr})
	r.observation.Trace = append(r.observation.Trace, "request:"+req.URL.String())
	cookie := req.Header.Get("Cookie")
	if cookie != "FANBOXSESSID=owned-session-42" && cookie != "FANBOXSESSID=owned-session-7" {
		return nil, errors.New("unexpected selected saved session")
	}
	var reply fanboxContentReply
	if req.URL.Host == "www.fanbox.cc" && req.URL.Path == "/" {
		id := 42
		if cookie == "FANBOXSESSID=owned-session-7" {
			id = 7
		}
		reply.Body = fmt.Sprintf(`<html><head><meta name="metadata" content='{"context":{"user":{"userId":%d,"name":"owned user"}}}'></head></html>`, id)
	} else {
		if req.URL.Host != "api.fanbox.cc" {
			return nil, errors.New("unexpected owned content destination")
		}
		if r.index >= len(r.input.Replies) {
			return nil, errors.New("owned response sequence exhausted")
		}
		reply = r.input.Replies[r.index]
		r.index++
	}
	if r.input.Cancel == "transport" {
		r.cancel()
		return nil, req.Context().Err()
	}
	if reply.TransportError != "" {
		return nil, errors.New(reply.TransportError)
	}
	status := reply.Status
	if status == 0 {
		status = 200
	}
	return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"application/json"}}, Body: &fanboxContentBody{Reader: strings.NewReader(reply.Body), reply: reply, observation: r.observation}}, nil
}

type fanboxContentBody struct {
	*strings.Reader
	reply       fanboxContentReply
	observation *fanboxContentObservation
	failed      bool
}

func (b *fanboxContentBody) Read(p []byte) (int, error) {
	if b.reply.ReadError != "" && !b.failed {
		b.failed = true
		return 0, errors.New(b.reply.ReadError)
	}
	return b.Reader.Read(p)
}
func (b *fanboxContentBody) Close() error {
	b.observation.BodyCloses++
	b.observation.Trace = append(b.observation.Trace, "body.close")
	if b.reply.CloseError != "" {
		return errors.New(b.reply.CloseError)
	}
	return nil
}

type fanboxContentOpener struct {
	accounts    *fanboxaccount.Service
	observation *fanboxContentObservation
	clients     map[*fanboxsdk.Client]struct{}
}

func (p fanboxContentOpener) OpenClientWithProxy(ctx context.Context, proxy *string) (*fanboxsdk.Client, error) {
	if proxy == nil {
		p.observation.ProxyOverrides = append(p.observation.ProxyOverrides, nil)
	} else {
		copy := *proxy
		p.observation.ProxyOverrides = append(p.observation.ProxyOverrides, &copy)
	}
	p.observation.Trace = append(p.observation.Trace, "account.open")
	client, err := p.accounts.OpenClientWithProxy(ctx, proxy)
	if client != nil {
		p.clients[client] = struct{}{}
		p.observation.UniqueClients = len(p.clients)
	}
	return client, err
}

func fanboxContentErrorText(text, home string) string {
	text = strings.ReplaceAll(text, home, "<HOME>")
	return regexp.MustCompile(`pixiv-cli-fanbox-json-[0-9]+\.tmp`).ReplaceAllString(text, "pixiv-cli-fanbox-json-<RANDOM>.tmp")
}

func fanboxContentSHA(t *testing.T, path string) string {
	t.Helper()
	b, e := os.ReadFile(path)
	if os.IsNotExist(e) {
		return "absent"
	}
	if e != nil {
		t.Fatal(e)
	}
	return fmt.Sprintf("%x", sha256.Sum256(b))
}
func fanboxContentDBRows(t *testing.T, path string) []string {
	t.Helper()
	db, err := sql.Open("sqlite", "file:"+filepath.ToSlash(path)+"?mode=ro")
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	result := []string{}
	for _, query := range []string{
		"SELECT user_id,sort_order,display_name,creator_id,hex(session_id),credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY user_id",
		"SELECT user_id,sort_order,username,hex(refresh_token),credential_revision,schedulable,created_at,updated_at FROM pixiv_account ORDER BY user_id",
		"SELECT version,name,checksum,applied_at FROM schema_migration ORDER BY version",
	} {
		rows, err := db.Query(query)
		if err != nil {
			result = append(result, err.Error())
			continue
		}
		columns, err := rows.Columns()
		if err != nil {
			t.Fatal(err)
		}
		for rows.Next() {
			values := make([]any, len(columns))
			pointers := make([]any, len(columns))
			for i := range values {
				pointers[i] = &values[i]
			}
			if err := rows.Scan(pointers...); err != nil {
				t.Fatal(err)
			}
			body, err := json.Marshal(values)
			if err != nil {
				t.Fatal(err)
			}
			result = append(result, string(body))
		}
		if err := rows.Err(); err != nil {
			t.Fatal(err)
		}
		if err := rows.Close(); err != nil {
			t.Fatal(err)
		}
	}
	return result
}

func fanboxContentSeed(t *testing.T, home string, input fanboxContentInput) {
	t.Helper()
	dir := filepath.Join(home, ".pixiv-cli")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.toml"), []byte(input.Config), 0600); err != nil {
		t.Fatal(err)
	}
	if input.DBFailure == "corrupt" {
		if err := os.WriteFile(database.DatabasePath(dir), []byte("owned non-SQLite database"), 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	db, err := database.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.DB().Exec("UPDATE schema_migration SET applied_at=10"); err != nil {
		t.Fatal(err)
	}
	if _, err := db.DB().Exec(`INSERT INTO pixiv_account(user_id,sort_order,username,refresh_token,credential_revision,created_at,updated_at) VALUES(99,1,'pixiv canary',x'6e657665722d757365',1,11,22)`); err != nil {
		t.Fatal(err)
	}
	if input.Saved != "none" {
		session := "owned-session-42"
		if input.Saved == "empty" {
			session = ""
		}
		if input.Saved == "invalid" {
			session = "bad\r\nsecret-session"
		}
		for _, item := range []struct {
			id, order int
			session   string
		}{{42, 9, session}, {7, 2, "owned-session-7"}} {
			if _, err := db.DB().Exec(`INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)`, item.id, item.order, "owned account", fmt.Sprint(item.id), []byte(item.session), 3, 10, 11, 22); err != nil {
				t.Fatal(err)
			}
		}
	}
	if input.DBFailure == "missing-table" {
		if _, err := db.DB().Exec("DROP TABLE fanbox_account"); err != nil {
			t.Fatal(err)
		}
	}
}

func TestMigrationFanboxContentReadsChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_FANBOX_CONTENT_READS_CHILD")
	if encoded == "" {
		t.Skip("owned FANBOX content helper")
	}
	var row fanboxContentCase
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || os.Getenv("TMPDIR") != filepath.Join(home, "temp") {
		t.Fatal("content child does not own home/temp")
	}
	if err := os.MkdirAll(os.Getenv("TMPDIR"), 0700); err != nil {
		t.Fatal(err)
	}
	fanboxContentSeed(t, home, row.Input)
	dbPath := database.DatabasePath(filepath.Join(home, ".pixiv-cli"))
	observation := fanboxContentObservation{Exits: []int{}, Errors: []string{}, Reasons: []string{}, Trace: []string{}, Requests: []fanboxContentRequest{}, Options: []map[string]any{}, ProxyOverrides: []*string{}, Writes: []int{}, Temps: []string{}}
	observation.DBBefore = fanboxContentSHA(t, dbPath)
	observation.DBRowsBefore = fanboxContentDBRows(t, dbPath)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	transport := &fanboxContentTransport{observation: &observation, input: row.Input, cancel: cancel}
	reader := &fanboxContentReader{input: strings.NewReader(row.Input.Stdin), observation: &observation, fail: row.Input.StdinError}
	writer := &fanboxContentWriter{observation: &observation, remaining: row.Input.WriteLimit, mode: row.Input.Writer}
	var diagnostics bytes.Buffer
	clients := map[*fanboxsdk.Client]struct{}{}
	var output io.Writer = writer
	var pipeRead, pipeWrite *os.File
	var pipeDone chan []byte
	if row.Input.OutputPipe {
		var err error
		pipeRead, pipeWrite, err = os.Pipe()
		if err != nil {
			t.Fatal(err)
		}
		defer pipeRead.Close()
		defer pipeWrite.Close()
		output = fanboxContentPipeWriter{writer: writer, pipe: pipeWrite}
		pipeDone = make(chan []byte, 1)
		go func() {
			b, err := io.ReadAll(pipeRead)
			if err != nil {
				pipeDone <- nil
				return
			}
			pipeDone <- b
		}()
	}
	observation.OutputIsTTY = outputIsTTY(output)
	if row.Input.BadTemp {
		if err := os.Remove(os.Getenv("TMPDIR")); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(os.Getenv("TMPDIR"), []byte("owned temp blocker"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	oldRuntime := loadCLIRuntimeConfig
	loadCLIRuntimeConfig = func() (settings.RuntimeConfig, error) {
		observation.Trace = append(observation.Trace, "runtime.read")
		return oldRuntime()
	}
	forbidden := func(name string) error {
		observation.Trace = append(observation.Trace, "forbidden:"+name)
		return errors.New("forbidden owned " + name)
	}
	cleanupPendingWindowsUpdate = func() error {
		observation.Trace = append(observation.Trace, "startup.cleanup")
		if row.Input.Boundary == "root-startup" {
			return errors.New("owned startup stop before external effect")
		}
		return nil
	}
	automaticPersistentHandlerSupported = func() bool { observation.Trace = append(observation.Trace, "startup.supported"); return false }
	ensureURLSchemeRelay = func(context.Context) error { return forbidden("native-handler") }
	newCLIAutomaticUpdateChecker = func(string) (*update.AutomaticUpdateChecker, error) { return nil, forbidden("automatic-update") }
	newCLIFanboxAccountService = func(app) (*fanboxaccount.Service, error) { return nil, forbidden("auth-mutation") }
	fanboxBrowserSessionReader = migrationFanboxHelpBrowser{calls: &observation.Trace}
	newCLIFanboxService = func(a app) (*fanboxapp.Facade, error) {
		observation.Trace = append(observation.Trace, "service.open")
		if strings.HasPrefix(row.Input.Boundary, "root") {
			return nil, errors.New("owned stop at FANBOX service composition")
		}
		accounts, err := a.newFanboxAccountService()
		if err != nil {
			return nil, err
		}
		original := accounts.LoadOptionsFunc
		accounts.LoadOptionsFunc = func() (fanboxsdk.Options, error) {
			observation.Trace = append(observation.Trace, "options.load")
			if row.Input.OptionsError {
				return fanboxsdk.Options{}, errors.New("owned options failure")
			}
			options, err := original()
			if err != nil {
				return options, err
			}
			var solver any
			if options.FlareSolverr != nil {
				solver = map[string]any{"url": options.FlareSolverr.URL, "proxy_url": options.FlareSolverr.ProxyURL}
			}
			observation.Options = append(observation.Options, map[string]any{"proxy_url": options.ProxyURL, "user_agent": options.UserAgent, "solver": solver})
			options.HTTPClient = &http.Client{Transport: transport}
			return options, nil
		}
		return fanboxapp.NewFacadeWithCloseClient(fanboxContentOpener{accounts: accounts, observation: &observation, clients: clients}, func(client *fanboxsdk.Client) error {
			observation.LeaseCloses++
			observation.Trace = append(observation.Trace, "lease.close")
			client.CloseIdleConnections()
			if row.Input.CloseError {
				return errors.New("owned lease close failure")
			}
			return nil
		}), nil
	}
	observation.SocketDenied, observation.ExecDenied = migrationFanboxHelpDenyExternal(t)
	if row.Input.Cancel == "before" {
		cancel()
	}
	repeat := row.Input.Repeat
	if repeat == 0 {
		repeat = 1
	}
	for i := 0; i < repeat; i++ {
		if strings.HasPrefix(row.Input.Boundary, "root") {
			exit := RunContext(ctx, append([]string{"pixiv"}, row.Input.Args...), reader, output, &diagnostics)
			observation.Exits = append(observation.Exits, exit)
			observation.Errors = append(observation.Errors, "")
			observation.Reasons = append(observation.Reasons, "")
			continue
		}
		a := app{in: reader, out: output, errOut: &diagnostics, closeState: &closeState{}}
		data := a.fanboxDataDeps()
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.SetFlagErrorFunc(func(_ *cobra.Command, err error) error { return normalizeFlagError(err) })
		root.AddCommand(fanboxcommands.New(data, fanboxcommands.CommandSet{Posts: fanboxpost.Commands(data)}))
		root.SetIn(reader)
		root.SetOut(output)
		root.SetErr(&diagnostics)
		root.SetArgs(row.Input.Args)
		root.SetContext(ctx)
		target, _, findErr := root.Find(row.Input.Args)
		if findErr != nil || target == nil {
			target = root
		}
		err := root.Execute()
		err = errors.Join(err, a.closeResources())
		secondClose := a.closeResources()
		if !reflect.DeepEqual(secondClose, a.closeResources()) {
			t.Fatal("execution cleanup is not once-owned")
		}
		text := ""
		if err != nil {
			text = fanboxContentErrorText(err.Error(), home)
		}
		observation.Errors = append(observation.Errors, text)
		observation.Reasons = append(observation.Reasons, string(sdk.ReasonOf(err)))
		observation.AutoNDJSON = commandAutoWritesNDJSON(target, output)
		exit := a.exitWithNDJSONScope(err, commandWritesNDJSON(target) || commandAutoWritesNDJSON(target, output), commandWritesNDJSON(target) || commandExplicitJSON(target))
		observation.Exits = append(observation.Exits, exit)
		pipeline.Clear(root)
	}
	observation.Stdout = writer.buffer.String()
	if row.Input.OutputPipe {
		if err := pipeWrite.Close(); err != nil {
			t.Fatal(err)
		}
		observation.Stdout = string(<-pipeDone)
	}
	observation.Stderr = fanboxContentErrorText(diagnostics.String(), home)
	observation.DBAfter = fanboxContentSHA(t, dbPath)
	observation.DBRowsAfter = fanboxContentDBRows(t, dbPath)
	config, err := os.ReadFile(filepath.Join(home, ".pixiv-cli", "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	observation.ConfigAfter = string(config)
	if !bytes.Equal(config, []byte(row.Input.Config)) {
		t.Fatal("content CLI changed saved config")
	}
	if !reflect.DeepEqual(observation.DBRowsBefore, observation.DBRowsAfter) {
		t.Fatal("content CLI changed saved account/schema rows")
	}
	if !row.Input.BadTemp {
		entries, err := os.ReadDir(os.Getenv("TMPDIR"))
		if err != nil {
			t.Fatal(err)
		}
		for _, entry := range entries {
			observation.Temps = append(observation.Temps, entry.Name())
		}
		if len(observation.Temps) != 0 {
			t.Fatal("content CLI left temporary files")
		}
	}
	if strings.Contains(observation.Stdout+observation.Stderr, "owned-session") || strings.Contains(observation.Stdout+observation.Stderr, "secret-session") {
		t.Fatal("content output exposed saved session")
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

func fanboxContentSources(t *testing.T) (map[string]string, map[string]string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	listing, err := exec.CommandContext(ctx, "git", "ls-tree", "-r", "--full-tree", "--name-only", fanboxContentReference).Output()
	if err != nil {
		t.Fatal(err)
	}
	all := map[string]string{}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if (strings.HasSuffix(path, ".go") && !strings.HasSuffix(path, "_test.go")) || path == "go.mod" || path == "go.sum" {
			frozen, err := exec.CommandContext(ctx, "git", "show", fanboxContentReference+":"+path).Output()
			if err != nil {
				t.Fatal(err)
			}
			current, err := os.ReadFile(filepath.Join("../..", path))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(frozen, current) {
				t.Fatalf("frozen Go production changed: %s", path)
			}
			all[path] = fmt.Sprintf("%x", sha256.Sum256(current))
		}
	}
	if len(all) != 434 {
		t.Fatalf("frozen production manifest has %d paths", len(all))
	}
	owners := map[string]string{}
	for path, hash := range all {
		if strings.HasPrefix(path, "sdk/") || strings.HasPrefix(path, "internal/cli/") || strings.HasPrefix(path, "internal/services/fanbox/") || strings.HasPrefix(path, "internal/storage/database/") || strings.HasPrefix(path, "internal/config/") || strings.HasPrefix(path, "internal/shared/lifecycle/") || path == "go.mod" || path == "go.sum" {
			owners[path] = hash
		}
	}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if strings.HasPrefix(path, "internal/storage/database/migrations/") && strings.HasSuffix(path, ".sql") {
			frozen, err := exec.CommandContext(ctx, "git", "show", fanboxContentReference+":"+path).Output()
			if err != nil {
				t.Fatal(err)
			}
			current, err := os.ReadFile(filepath.Join("../..", path))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(frozen, current) {
				t.Fatalf("frozen SQLite migration changed: %s", path)
			}
			owners[path] = fmt.Sprintf("%x", sha256.Sum256(current))
		}
	}
	return owners, all
}

func fanboxContentRows() []fanboxContentCase {
	rows := []fanboxContentCase{}
	add := func(name string, args []string, replies ...string) *fanboxContentCase {
		input := fanboxContentInput{Boundary: "composition", Args: args, Config: "[fanbox.auth]\ndefault_user_id=42\n", Saved: "explicit", Replies: []fanboxContentReply{}}
		for _, body := range replies {
			input.Replies = append(input.Replies, fanboxContentReply{Body: body})
		}
		rows = append(rows, fanboxContentCase{Name: name, Input: input})
		return &rows[len(rows)-1]
	}
	post := func(id string) string {
		return `{"id":"` + id + `","title":"色と quote \"title\"","publishedDatetime":"2024-01-02T03:04:05.987654321+09:00","creatorId":"creator-one","feeRequired":500,"isRestricted":true,"isPinned":true,"commentCount":8,"body":{"text":"body text\nsecond line","images":[{"id":"image-one","extension":"png","originalUrl":"https://downloads.fanbox.cc/owned/image.png"}]}}`
	}
	barePost := `{"id":"0","title":"zero optional fields","publishedDatetime":"2024-01-02T03:04:05Z"}`
	info := `{"body":{"post":` + post("123") + `}}`
	list := `{"body":{"posts":[` + post("123") + `,` + barePost + `]}}`
	tags := `{"body":[{"tag":" blue ","url":""},{"tag":"色","url":"https://www.fanbox.cc/@creator-one/tags/%E8%89%B2"}]}`
	creators := `{"body":[{"creatorId":"creator-one"},{"creatorId":"色"}]}`
	for _, leaf := range []string{"post", "posts", "home", "supporting", "creators", "tags"} {
		args := []string{"fanbox", leaf}
		reply := list
		switch leaf {
		case "post":
			args = append(args, "123")
			reply = info
		case "posts":
			args = append(args, "creator-one")
		case "creators":
			reply = creators
		case "tags":
			args = append(args, "creator-one")
			reply = tags
		}
		for _, mode := range []string{"text", "json", "ndjson"} {
			a := append([]string{}, args...)
			if mode != "text" {
				a = append(a, "--"+mode)
			}
			add(leaf+"-"+mode, a, reply)
		}
	}
	add("post-zero-optional-summary", []string{"fanbox", "post", "0", "--json"}, `{"body":{"post":`+barePost+`}}`)
	add("post-body-null-summary", []string{"fanbox", "post", "123", "--ndjson"}, `{"body":{"post":{"id":"123","publishedDatetime":"2024-01-02T03:04:05Z","body":null}}}`)
	add("creators-following", []string{"fanbox", "creators", "--kind", "following", "--json"}, `{"body":{"creators":[{"creatorId":"following"}]}}`)
	for _, item := range []struct{ name, source string }{{"numeric", "123"}, {"numeric-leading-zero", "00123"}, {"post-url", "https://www.fanbox.cc/@creator-one/posts/123"}, {"creator-url", "https://www.fanbox.cc/@creator-one"}, {"creator-subdomain-path", "https://creator-one.fanbox.cc/posts/123"}, {"tag-url", "https://www.fanbox.cc/@creator-one/posts/tag/blue%20sky"}, {"unicode-numeric-is-creator", "１２３"}, {"space-numeric-is-creator", " 123 "}} {
		body := list
		if item.name == "numeric" || item.name == "numeric-leading-zero" || item.name == "post-url" {
			body = info
		}
		add("posts-source-"+item.name, []string{"fanbox", "posts", item.source, "--json"}, body)
	}
	for _, item := range []struct{ name, source string }{{"http-url-rejected", "http://www.fanbox.cc/@creator-one"}, {"foreign-host-rejected", "https://evil.invalid/@creator-one"}, {"unknown-path-rejected", "https://www.fanbox.cc/unknown/path"}, {"uppercase-scheme-is-creator", "HTTPS://www.fanbox.cc/@creator-one"}, {"whitespace-creator-rejected", "   "}, {"empty-creator-rejected", ""}} {
		add("posts-source-"+item.name, []string{"fanbox", "posts", item.source, "--json"}, list)
	}

	for _, leaf := range []string{"post", "posts", "tags"} {
		body := info
		stdin := "123\n"
		if leaf == "posts" {
			body = list
			stdin = "creator-one\n"
		}
		if leaf == "tags" {
			body = tags
			stdin = "creator-one\n"
		}
		row := add(leaf+"-stdin", []string{"fanbox", leaf, "--json"}, body)
		row.Input.Stdin = stdin
	}
	add("post-explicit-never-reads-stdin", []string{"fanbox", "post", "123"}, info).Input.StdinError = true
	add("home-never-reads-stdin", []string{"fanbox", "home"}, list).Input.StdinError = true
	add("post-stdin-error", []string{"fanbox", "post", "--json"}).Input.StdinError = true
	add("posts-stdin-empty", []string{"fanbox", "posts"})
	for _, item := range []struct {
		name string
		args []string
	}{
		{"negative-limit-before-page", []string{"home", "--limit=-1", "--page=0"}},
		{"explicit-zero-page", []string{"home", "--page=0"}},
		{"page-requires-positive-limit", []string{"home", "--page=1"}},
		{"logical-offset-overflow", []string{"home", "--limit=2", "--page=9223372036854775807"}},
		{"json-false-still-conflicts-ndjson", []string{"home", "--json=false", "--ndjson"}},
		{"json-true-conflicts-ndjson", []string{"post", "123", "--json", "--ndjson"}},
		{"kind-before-list-plan", []string{"creators", "--kind=bad", "--limit=-1"}},
		{"single-post-rejects-limit", []string{"post", "123", "--limit=1"}},
		{"tags-rejects-page", []string{"tags", "creator-one", "--page=1"}},
		{"home-rejects-positionals", []string{"home", "extra"}},
		{"post-rejects-extra-positional", []string{"post", "123", "extra"}},
		{"proxy-presence-conflicts-no-proxy-false", []string{"home", "--proxy=", "--no-proxy=false"}},
		{"limit-not-an-integer", []string{"home", "--limit=oops"}},
		{"limit-int-overflow", []string{"home", "--limit=9223372036854775808"}},
	} {
		add("flags-"+item.name, append([]string{"fanbox"}, item.args...), list)
	}

	next := "https://api.fanbox.cc/owned-continuation?z=2&x=preserved%20space"
	page1 := `{"body":{"posts":[` + post("1") + `,` + post("2") + `],"nextUrl":"` + next + `"}}`
	page2 := `{"body":{"posts":[` + post("3") + `,` + post("4") + `]}}`
	empty1 := `{"body":{"posts":[],"nextUrl":"` + next + `"}}`
	cycle := `{"body":{"posts":[` + post("3") + `],"nextUrl":"` + next + `"}}`
	for _, item := range []struct {
		name          string
		flags         []string
		first, second string
	}{{"omitted-one-batch", nil, page1, page2}, {"explicit-zero-all", []string{"--limit=0"}, page1, page2}, {"positive-truncate", []string{"--limit=1"}, page1, page2}, {"positive-span-pages", []string{"--limit=3"}, page1, page2}, {"logical-page-skip", []string{"--limit=2", "--page=2"}, page1, page2}, {"logical-page-empty", []string{"--limit=2", "--page=3"}, page1, page2}, {"empty-continuing-first", nil, empty1, page2}, {"empty-all", []string{"--limit=0"}, `{"body":{"posts":[]}}`, page2}} {
		add("pagination-"+item.name, append([]string{"fanbox", "posts", "creator-one", "--json"}, item.flags...), item.first, item.second)
	}
	for _, mode := range []string{"json", "ndjson", "text"} {
		args := []string{"fanbox", "posts", "creator-one", "--limit=0"}
		if mode != "text" {
			args = append(args, "--"+mode)
		}
		add("pagination-cycle-"+mode, args, page1, cycle)
		row := add("pagination-prefix-failure-"+mode, args, page1, `{"body":{"posts":[{"title":"missing id"}]}}`)
		_ = row
	}
	add("pagination-unsafe-unconsumed-next", []string{"fanbox", "posts", "creator-one", "--json"}, `{"body":{"posts":[`+post("1")+`],"nextUrl":"https://evil.invalid/page"}}`)
	for _, leaf := range []string{"home", "supporting", "creators"} {
		first, second := page1, page2
		if leaf == "creators" {
			first = `{"body":{"plans":[{"creatorId":"one"}],"nextUrl":"` + next + `"}}`
			second = `{"body":{"plans":[{"creatorId":"two"}]}}`
		}
		add(leaf+"-identity-cursor-two-pages", []string{"fanbox", leaf, "--limit=0", "--ndjson"}, first, second)
	}
	add("tags-empty", []string{"fanbox", "tags", "creator-one", "--json"}, `{"body":[]}`)
	add("home-empty-json", []string{"fanbox", "home", "--json"}, `{"body":{"posts":[]}}`)
	add("home-items-fallback", []string{"fanbox", "home", "--json"}, `{"body":{"items":[`+post("123")+`]}}`)
	add("supporting-empty-posts-priority", []string{"fanbox", "supporting", "--json"}, `{"body":{"posts":[],"items":[`+post("123")+`]}}`)
	for _, item := range []struct {
		name, config string
		args         []string
	}{{"auto-first-sort-order", "", nil}, {"explicit-default", "[fanbox.auth]\ndefault_user_id=42\n", nil}, {"missing-default", "[fanbox.auth]\ndefault_user_id=999\n", nil}, {"invalid-default", "[fanbox.auth]\ndefault_user_id=0\n", nil}, {"global-proxy", "https_proxy='http://global.invalid:8080'\n", nil}, {"service-empty-proxy", "https_proxy='http://global.invalid:8080'\n[fanbox.network]\nproxy_url=''\n", nil}, {"service-proxy", "https_proxy='http://global.invalid:8080'\n[fanbox.network]\nproxy_url='https://service.invalid:8081'\n", nil}, {"command-empty-proxy", "[fanbox.network]\nproxy_url='ftp://invalid.invalid'\n", []string{"--proxy="}}, {"command-proxy", "[fanbox.network]\nproxy_url='http://service.invalid'\n", []string{"--proxy=https://command.invalid:8082"}}, {"command-no-proxy", "[fanbox.network]\nproxy_url='http://service.invalid'\n", []string{"--no-proxy"}}, {"no-proxy-false-preserves", "[fanbox.network]\nproxy_url='http://service.invalid'\n", []string{"--no-proxy=false"}}, {"invalid-proxy", "[fanbox.network]\nproxy_url='ftp://invalid.invalid'\n", nil}, {"agent-solver-independent", "[fanbox.network]\nuser_agent='owned-UA/1'\n[fanbox.flaresolverr]\nurl='http://solver.invalid:8191/'\nproxy_url='http://solver-proxy.invalid:8181'\n", []string{"--no-proxy"}}, {"invalid-agent", "[fanbox.network]\nuser_agent='bad\u0001agent'\n", nil}, {"malformed-config", "[unfinished\n", nil}, {"invalid-pool-config", "[account_pool]\nenabled='bad'\n", nil}} {
		row := add("saved-"+item.name, append([]string{"fanbox", "post", "123", "--json"}, item.args...), info)
		row.Input.Config = item.config
	}
	for _, saved := range []string{"none", "empty", "invalid"} {
		row := add("saved-"+saved+"-no-pixiv-fallback", []string{"fanbox", "post", "123", "--json"}, info)
		row.Input.Saved = saved
		if saved == "none" {
			row.Input.Config = "[account_pool]\nenabled=true\nstrategy='round_robin'\n"
		}
	}
	for _, failure := range []string{"corrupt", "missing-table"} {
		row := add("database-"+failure+"-before-options", []string{"fanbox", "post", "123", "--json"}, info)
		row.Input.DBFailure = failure
		row.Input.OptionsError = true
	}
	add("options-failure-after-selection", []string{"fanbox", "post", "123", "--json"}, info).Input.OptionsError = true
	row := add("saved-selection-before-options", []string{"fanbox", "post", "123", "--json"}, info)
	row.Input.Saved = "none"
	row.Input.OptionsError = true
	for _, mode := range []string{"text", "json", "ndjson"} {
		args := []string{"fanbox", "post", "123"}
		if mode != "text" {
			args = append(args, "--"+mode)
		}
		add("lease-close-after-"+mode, args, info).Input.CloseError = true
	}
	row = add("fetch-and-close-join", []string{"fanbox", "post", "123", "--json"}, `{"body":{"post":{"title":"missing"}}}`)
	row.Input.CloseError = true
	for _, leaf := range []string{"post", "posts", "tags", "creators"} {
		args := []string{"fanbox", leaf}
		body := list
		switch leaf {
		case "post":
			args = append(args, "123")
			body = info
		case "posts":
			args = append(args, "creator-one")
		case "tags":
			args = append(args, "creator-one")
			body = tags
		case "creators":
			body = creators
		}
		for _, mode := range []string{"text", "json", "ndjson"} {
			a := append([]string{}, args...)
			if mode != "text" {
				a = append(a, "--"+mode)
			}
			for _, writer := range []string{"error", "pipe", "short"} {
				row := add("writer-"+leaf+"-"+mode+"-"+writer, a, body)
				row.Input.Writer = writer
				row.Input.WriteLimit = 8
			}
		}
	}
	row = add("ndjson-epipe-joined-close", []string{"fanbox", "post", "123", "--ndjson"}, info)
	row.Input.Writer = "pipe"
	row.Input.CloseError = true
	row = add("json-temp-create-failure", []string{"fanbox", "posts", "creator-one", "--json"}, list)
	row.Input.BadTemp = true
	for _, kind := range []string{"transport", "read", "close", "decode-close", "unauthorized", "forbidden", "malformed"} {
		row := add("sdk-failure-"+kind, []string{"fanbox", "post", "123", "--json"}, info)
		switch kind {
		case "transport":
			row.Input.Replies[0].TransportError = "owned transport failure"
		case "read":
			row.Input.Replies[0].ReadError = "owned body read failure"
		case "close":
			row.Input.Replies[0].CloseError = "owned body close failure"
		case "decode-close":
			row.Input.Replies[0].Body = "{"
			row.Input.Replies[0].CloseError = "owned body close failure"
		case "unauthorized":
			row.Input.Replies[0].Status = 401
		case "forbidden":
			row.Input.Replies[0].Status = 403
		case "malformed":
			row.Input.Replies[0].Body = "{"
		}
	}
	for _, cancel := range []string{"before", "transport"} {
		row := add("cancel-"+cancel, []string{"fanbox", "home", "--json"}, list)
		row.Input.Cancel = cancel
	}
	row = add("independent-saved-commands", []string{"fanbox", "post", "123", "--ndjson"}, info, info)
	row.Input.Repeat = 2
	for _, item := range []struct {
		name, boundary, config string
		args                   []string
	}{{"root-startup-before-malformed-config", "root-startup", "[unfinished\n", []string{"fanbox", "home", "--json"}}, {"root-config-before-service", "root-config", "[unfinished\n", []string{"fanbox", "home", "--json"}}, {"root-safe-stop-at-composition", "root-stop", "", []string{"fanbox", "home", "--json"}}, {"root-flags-before-startup", "root-startup", "[unfinished\n", []string{"fanbox", "home", "--unknown"}}, {"root-stdin-error-before-startup", "root-startup", "[unfinished\n", []string{"fanbox", "post", "--json"}}} {
		row := add(item.name, item.args)
		row.Input.Boundary = item.boundary
		row.Input.Config = item.config
		if strings.Contains(item.name, "stdin") {
			row.Input.StdinError = true
		}
	}
	add("pagination-unsafe-consumed-next", []string{"fanbox", "posts", "creator-one", "--limit=0", "--json"}, `{"body":{"posts":[`+post("1")+`],"nextUrl":"https://evil.invalid/page"}}`)
	add("post-missing-publish-time", []string{"fanbox", "post", "123", "--json"}, `{"body":{"post":{"id":"123"}}}`)
	add("tags-invalid-name", []string{"fanbox", "tags", "creator-one", "--json"}, `{"body":[{"tag":"   "}]}`)
	add("tagged-two-pages", []string{"fanbox", "posts", "https://www.fanbox.cc/@creator-one/posts/tag/blue%20sky", "--limit=0", "--ndjson"}, page1, page2)
	add("saved-config-json-does-not-select-json", []string{"fanbox", "post", "123"}, info).Input.Config = "output_json=true\n"
	for _, leaf := range []string{"post", "posts", "home", "supporting", "creators", "tags"} {
		args := []string{"fanbox", leaf}
		body := list
		switch leaf {
		case "post":
			args = append(args, "123")
			body = info
		case "posts":
			args = append(args, "creator-one")
		case "tags":
			args = append(args, "creator-one")
			body = tags
		case "creators":
			body = creators
		}
		add("pipe-retains-text-"+leaf, args, body).Input.OutputPipe = true
	}
	add("flags-explicit-json-false-selects-text", []string{"fanbox", "home", "--json=false"}, list)
	add("flags-explicit-ndjson-false-selects-text", []string{"fanbox", "post", "123", "--ndjson=false"}, info)
	add("flags-page-one-positive-limit", []string{"fanbox", "home", "--limit=2", "--page=1", "--json"}, page1, page2)
	return rows
}

func TestMigrationFanboxContentReads(t *testing.T) {
	sources, all := fanboxContentSources(t)
	fixture := fanboxContentFixture{Reference: fanboxContentReference, Base: "7078b729cc4dd48ee2b28f8eedcb758bacbc3a0f", Environment: "linux/amd64; TZ=Asia/Tokyo; owned SQLite/config and in-memory net/http transport", Sources: sources, FrozenGoProduction: all, Cases: fanboxContentRows(), Reused: []fanboxContentReuse{}, Limitations: []string{
		"Successful reads execute genuine CLI leaf presenters and root fanboxDataDeps/newFanboxAccountService composition, selected saved SQLite account/default/config, real Facade and public SDK. Only real HTTP transport and existing close policy are injected; no returned DTO is mocked.",
		"Successful reads stop at the existing safe command/composition boundary. Actual successful cmd/pixiv subprocess startup, native handler/update effects and native networking remain unproved; bounded cli.Run failure children expose ordering with safe dependency canaries.",
		"Process-wide socket/connect/exec denial is installed only after owned synthetic DB/config setup. It is reused unchanged from FANBOX help22; no browser, auth mutation, download, live media, persistent credentials, native registration, trust or update effect occurs.",
		"Synthetic session literals appear only in fixture inputs/requests, never command output. Owned HOME and randomized CreateTemp basename suffix are replaced by explicit <HOME>/<RANDOM> in path-bearing errors/diagnostics only; presenter output and requests remain byte-exact.",
		"Saved-account321 and help22 are referenced by unchanged byte hashes. This content capture does not replace their separate repository/service/lease or help/parser coverage.",
		"Options observations occur before the command override because the actual account owner loads config before applying it; actual override pointers and SDK requests/errors separately prove presence and constructor precedence.",
		"database.Open genuinely writes PRAGMA user_version on every open. Raw SQLite hashes preserve this physical write; account/schema/pool rows are checked unchanged. Only synthetic schema applied_at seeds are fixed before the before-snapshot.",
		"SDK endpoint/cursor/wire/resource and MCP sibling captures own deeper contracts. Go nil/interface/panic/native compressed media/platform and forbidden multiplex/HEAD/upload probes are outside this CLI slice.",
	}}
	for _, path := range []string{"crates/pixiv-app/tests/fixtures/fanbox-saved-accounts.json", "crates/pixiv-cli/tests/fixtures/fanbox-help-routing.json"} {
		body, err := os.ReadFile(filepath.Join("../..", path))
		if err != nil {
			t.Fatal(err)
		}
		var data map[string]json.RawMessage
		if err := json.Unmarshal(body, &data); err != nil {
			t.Fatal(err)
		}
		count := 0
		if cases, ok := data["cases"]; ok {
			var rows []json.RawMessage
			if err := json.Unmarshal(cases, &rows); err != nil {
				t.Fatal(err)
			}
			count = len(rows)
		}
		for _, key := range []string{"repository", "service", "leases"} {
			var section struct {
				Cases []json.RawMessage `json:"cases"`
			}
			if value, ok := data[key]; ok {
				if err := json.Unmarshal(value, &section); err != nil {
					t.Fatal(err)
				}
				count += len(section.Cases)
			}
		}
		fixture.Reused = append(fixture.Reused, fanboxContentReuse{Path: path, SHA256: fmt.Sprintf("%x", sha256.Sum256(body)), Cases: count})
	}
	for i, row := range fixture.Cases {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxContentReadsChild$", "-test.count=1", "-test.timeout=8s")
			child.WaitDelay = time.Second
			child.Dir = home
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "TMPDIR=" + filepath.Join(home, "temp"), "PATH=", "TZ=Asia/Tokyo", "LANG=C", "LC_ALL=C", "GOMAXPROCS=2", "GORACE=atexit_sleep_ms=0", "MIGRATION_FANBOX_CONTENT_READS_CHILD=" + string(body)}
			if output, err := child.CombinedOutput(); err != nil {
				t.Fatalf("owned content child: %v\n%s", err, output)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &fixture.Cases[i]); err != nil {
				t.Fatal(err)
			}
			if i < 18 || strings.HasPrefix(row.Name, "pipe-retains-text-") {
				actual := fixture.Cases[i].Observation
				if !reflect.DeepEqual(actual.Exits, []int{0}) || actual.Stdout == "" || len(actual.Requests) != 1 {
					t.Fatalf("presenter seed did not establish genuine success: %+v", actual)
				}
				if actual.AutoNDJSON || actual.OutputIsTTY {
					t.Fatal("FANBOX content unexpectedly selected automatic NDJSON or terminal output")
				}
			}

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
	path := "../../crates/pixiv-cli/tests/fixtures/fanbox-content-reads.json"
	if *captureFanboxContentReads {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, body) {
		t.Fatal("genuine saved FANBOX content CLI differs from frozen Go capture")
	}
}
