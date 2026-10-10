//go:build linux && amd64

package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	urlpkg "net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	fanboxapp "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox"
	fanboxaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var captureFanboxDownload = flag.Bool("migration-capture-fanbox-download", false, "capture genuine saved FANBOX root download workflow")

const fanboxDownloadPublished = "7991c7d6c8e58ad1b4e562ebcf91e8070d0ccfb5"

type fanboxDownloadReply struct {
	fanboxContentReply
	URL           string `json:"url,omitempty"`
	ContentType   string `json:"content_type,omitempty"`
	ContentLength int64  `json:"content_length,omitempty"`
	ChunkSize     int    `json:"chunk_size,omitempty"`
	BytesAndEOF   bool   `json:"bytes_and_eof,omitempty"`
	BytesAndError bool   `json:"bytes_and_error,omitempty"`
	CancelAtRead  int    `json:"cancel_at_read,omitempty"`
}
type fanboxDownloadFile struct {
	Path     string `json:"path"`
	Kind     string `json:"kind"`
	Mode     string `json:"mode"`
	BytesHex string `json:"bytes_hex,omitempty"`
	Size     int64  `json:"size"`
	SHA256   string `json:"sha256,omitempty"`
}
type fanboxDownloadInput struct {
	Args           []string              `json:"args"`
	Config         string                `json:"config"`
	Saved          string                `json:"saved"`
	DBFailure      string                `json:"db_failure,omitempty"`
	Stdin          string                `json:"stdin,omitempty"`
	StdinError     bool                  `json:"stdin_error,omitempty"`
	Replies        []fanboxDownloadReply `json:"replies"`
	OutputRoot     string                `json:"output_root"`
	SeedFiles      []fanboxDownloadFile  `json:"seed_files"`
	Writer         string                `json:"writer,omitempty"`
	WriteLimit     int                   `json:"write_limit,omitempty"`
	CloseError     bool                  `json:"close_error,omitempty"`
	RootCloseError bool                  `json:"root_close_error,omitempty"`
	OptionsError   bool                  `json:"options_error,omitempty"`
	Factory        string                `json:"factory,omitempty"`
	Open           string                `json:"open,omitempty"`
	StartupError   bool                  `json:"startup_error,omitempty"`
	Cancel         string                `json:"cancel,omitempty"`
	Repeat         int                   `json:"repeat,omitempty"`
}
type fanboxDownloadRead struct {
	URL      string `json:"url"`
	Capacity int    `json:"capacity"`
	N        int    `json:"n"`
	Error    string `json:"error"`
}
type fanboxDownloadWrite struct {
	Bytes string `json:"bytes"`
	N     int    `json:"n"`
	Error string `json:"error"`
}
type fanboxDownloadObservation struct {
	fanboxContentObservation
	FilesBefore  []fanboxDownloadFile  `json:"files_before"`
	FilesAfter   []fanboxDownloadFile  `json:"files_after"`
	BodyReads    []fanboxDownloadRead  `json:"body_reads"`
	OutputWrites []fanboxDownloadWrite `json:"output_writes"`
	ReplyCount   int                   `json:"reply_count"`
}
type fanboxDownloadCase struct {
	Name        string                    `json:"name"`
	Input       fanboxDownloadInput       `json:"input"`
	Observation fanboxDownloadObservation `json:"observation"`
}
type fanboxDownloadFixture struct {
	Reference          string               `json:"reference"`
	Base               string               `json:"published_base"`
	Environment        string               `json:"environment"`
	Sources            map[string]string    `json:"sources"`
	FrozenGoProduction map[string]string    `json:"frozen_go_production"`
	PublishedFixtures  map[string]string    `json:"published_fixtures"`
	Limitations        []string             `json:"limitations"`
	Cases              []fanboxDownloadCase `json:"cases"`
}
type fanboxDownloadWriter struct {
	observation *fanboxDownloadObservation
	input       fanboxDownloadInput
	buffer      bytes.Buffer
	remaining   int
}

func (w *fanboxDownloadWriter) Write(p []byte) (int, error) {
	n := len(p)
	var err error
	if w.input.Writer != "" && n > w.remaining {
		n = w.remaining
		switch w.input.Writer {
		case "short":
		case "pipe":
			err = syscall.EPIPE
		default:
			err = errors.New("owned writer failure")
		}
	}
	if w.input.Writer != "" {
		w.remaining -= n
	}
	if n > 0 {
		_, _ = w.buffer.Write(p[:n])
	}
	message := ""
	if err != nil {
		message = err.Error()
	}
	w.observation.Writes = append(w.observation.Writes, len(p))
	w.observation.OutputWrites = append(w.observation.OutputWrites, fanboxDownloadWrite{Bytes: string(p), N: n, Error: message})
	w.observation.Trace = append(w.observation.Trace, "stdout.write")
	return n, err
}

type fanboxDownloadTransport struct {
	observation *fanboxDownloadObservation
	input       fanboxDownloadInput
	index       int
	cancel      context.CancelFunc
	unexpected  string
}

func (r *fanboxDownloadTransport) CloseIdleConnections() {
	r.observation.IdleCloses++
	r.observation.Trace = append(r.observation.Trace, "transport.idle-close")
}
func (r *fanboxDownloadTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	ctxErr := ""
	if req.Context().Err() != nil {
		ctxErr = req.Context().Err().Error()
	}
	rawURL := req.URL.String()
	r.observation.Requests = append(r.observation.Requests, fanboxContentRequest{Method: req.Method, URL: rawURL, Header: req.Header.Clone(), ContextError: ctxErr})
	r.observation.Trace = append(r.observation.Trace, "request:"+rawURL)
	cookie := req.Header.Get("Cookie")
	switch req.URL.Host {
	case "api.fanbox.cc", "downloads.fanbox.cc":
		if cookie != "FANBOXSESSID=owned-session-42" && cookie != "FANBOXSESSID=owned-session-7" {
			r.unexpected = "unexpected selected saved session"
			return nil, errors.New(r.unexpected)
		}
	case "i.pximg.net", "pixiv.pximg.net":
		if cookie != "" {
			r.unexpected = "unexpected credential on public media"
			return nil, errors.New(r.unexpected)
		}
	default:
		r.unexpected = "unexpected owned download destination"
		return nil, errors.New(r.unexpected)
	}
	if r.index >= len(r.input.Replies) {
		r.unexpected = "owned response sequence exhausted"
		return nil, errors.New(r.unexpected)
	}
	reply := r.input.Replies[r.index]
	r.index++
	if reply.URL != "" && reply.URL != rawURL {
		r.unexpected = fmt.Sprintf("owned expected request %s, got %s", reply.URL, rawURL)
		return nil, errors.New(r.unexpected)
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
	contentType := reply.ContentType
	if contentType == "" {
		contentType = "application/json"
	}
	header := http.Header{"Content-Type": {contentType}}
	if reply.ContentLength != 0 {
		header.Set("Content-Length", fmt.Sprint(reply.ContentLength))
	}
	return &http.Response{StatusCode: status, Header: header, ContentLength: reply.ContentLength, Body: &fanboxDownloadBody{Reader: strings.NewReader(reply.Body), reply: reply, url: rawURL, observation: r.observation, cancel: r.cancel}}, nil
}

type fanboxDownloadBody struct {
	*strings.Reader
	reply       fanboxDownloadReply
	url         string
	observation *fanboxDownloadObservation
	reads       int
	failed      bool
	cancel      context.CancelFunc
}

func (b *fanboxDownloadBody) Read(p []byte) (int, error) {
	capacity := len(p)
	b.reads++
	if b.reply.ChunkSize > 0 && len(p) > b.reply.ChunkSize {
		p = p[:b.reply.ChunkSize]
	}
	n := 0
	var err error
	if b.reply.ReadError != "" && !b.failed {
		b.failed = true
		if b.reply.BytesAndError {
			n, _ = b.Reader.Read(p)
		}
		err = errors.New(b.reply.ReadError)
	} else {
		n, err = b.Reader.Read(p)
		if n > 0 && b.reply.BytesAndEOF && b.Reader.Len() == 0 {
			err = io.EOF
		}
	}
	if b.reply.CancelAtRead == b.reads {
		b.cancel()
	}
	message := ""
	if err != nil {
		message = err.Error()
	}
	b.observation.BodyReads = append(b.observation.BodyReads, fanboxDownloadRead{URL: b.url, Capacity: capacity, N: n, Error: message})
	b.observation.Trace = append(b.observation.Trace, "body.read:"+b.url)
	return n, err
}
func (b *fanboxDownloadBody) Close() error {
	b.observation.BodyCloses++
	b.observation.Trace = append(b.observation.Trace, "body.close:"+b.url)
	if b.reply.CloseError != "" {
		return errors.New(b.reply.CloseError)
	}
	return nil
}

type fanboxDownloadOpener struct {
	accounts    *fanboxaccount.Service
	observation *fanboxDownloadObservation
	clients     map[*fanboxsdk.Client]struct{}
	mode        string
}

func (p fanboxDownloadOpener) OpenClientWithProxy(ctx context.Context, proxy *string) (*fanboxsdk.Client, error) {
	if proxy == nil {
		p.observation.ProxyOverrides = append(p.observation.ProxyOverrides, nil)
	} else {
		v := *proxy
		p.observation.ProxyOverrides = append(p.observation.ProxyOverrides, &v)
	}
	p.observation.Trace = append(p.observation.Trace, "account.open")
	switch p.mode {
	case "error":
		return nil, errors.New("owned account opener failure")
	case "nil":
		return nil, nil
	}
	client, err := p.accounts.OpenClientWithProxy(ctx, proxy)
	if client != nil {
		p.clients[client] = struct{}{}
		p.observation.UniqueClients = len(p.clients)
	}
	if err == nil && p.mode == "client-error" {
		err = errors.New("owned account opener returned client and failure")
	}
	return client, err
}
func fanboxDownloadFiles(t *testing.T, home, root string) []fanboxDownloadFile {
	t.Helper()
	files := []fanboxDownloadFile{}
	path := filepath.Join(home, root)
	if _, err := os.Stat(path); os.IsNotExist(err) {
		return files
	}
	err := filepath.Walk(path, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(home, path)
		if err != nil {
			return err
		}
		file := fanboxDownloadFile{Path: filepath.ToSlash(relative), Kind: "directory", Mode: fmt.Sprintf("%04o", info.Mode().Perm())}
		if info.Mode().IsRegular() {
			body, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			file.Kind = "file"
			file.BytesHex = hex.EncodeToString(body)
			file.Size = int64(len(body))
			file.SHA256 = fmt.Sprintf("%x", sha256.Sum256(body))
		} else if !info.IsDir() {
			return fmt.Errorf("unexpected owned output entry %s", relative)
		}
		files = append(files, file)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return files
}
func fanboxDownloadSeedFiles(t *testing.T, home string, input fanboxDownloadInput) {
	t.Helper()
	for _, file := range input.SeedFiles {
		if filepath.IsAbs(file.Path) || strings.HasPrefix(filepath.Clean(file.Path), "..") || !strings.HasPrefix(file.Path, input.OutputRoot) {
			t.Fatal("output seed is outside owned root")
		}
		path := filepath.Join(home, file.Path)
		mode, err := strconv.ParseUint(file.Mode, 8, 32)
		if err != nil {
			t.Fatal(err)
		}
		if file.Kind == "directory" {
			err = os.MkdirAll(path, os.FileMode(mode))
		} else {
			if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
				t.Fatal(err)
			}
			body, e := hex.DecodeString(file.BytesHex)
			if e != nil {
				t.Fatal(e)
			}
			err = os.WriteFile(path, body, os.FileMode(mode))
		}
		if err != nil {
			t.Fatal(err)
		}
	}
}
func TestMigrationFanboxDownloadChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_FANBOX_DOWNLOAD_CHILD")
	if encoded == "" {
		t.Skip("owned FANBOX download helper")
	}
	var row fanboxDownloadCase
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	if home == "" || home != os.Getenv("USERPROFILE") || os.Getenv("TMPDIR") != filepath.Join(home, "temp") {
		t.Fatal("download child does not own home/temp")
	}
	if filepath.IsAbs(row.Input.OutputRoot) || strings.HasPrefix(filepath.Clean(row.Input.OutputRoot), "..") {
		t.Fatal("download output is outside owned home")
	}
	syscall.Umask(0022)
	if err := os.MkdirAll(os.Getenv("TMPDIR"), 0700); err != nil {
		t.Fatal(err)
	}
	fanboxContentSeed(t, home, fanboxContentInput{Config: row.Input.Config, Saved: row.Input.Saved, DBFailure: row.Input.DBFailure})
	fanboxDownloadSeedFiles(t, home, row.Input)
	dbPath := database.DatabasePath(filepath.Join(home, ".pixiv-cli"))
	observation := fanboxDownloadObservation{fanboxContentObservation: fanboxContentObservation{Exits: []int{}, Errors: []string{}, Reasons: []string{}, Trace: []string{}, Requests: []fanboxContentRequest{}, Options: []map[string]any{}, ProxyOverrides: []*string{}, Writes: []int{}, Temps: []string{}}, FilesBefore: fanboxDownloadFiles(t, home, row.Input.OutputRoot), FilesAfter: []fanboxDownloadFile{}, BodyReads: []fanboxDownloadRead{}, OutputWrites: []fanboxDownloadWrite{}}
	observation.DBBefore = fanboxContentSHA(t, dbPath)
	observation.DBRowsBefore = fanboxContentDBRows(t, dbPath)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if row.Input.Cancel == "deadline" {
		var deadlineCancel context.CancelFunc
		ctx, deadlineCancel = context.WithDeadline(ctx, time.Unix(1, 0))
		defer deadlineCancel()
	}
	transport := &fanboxDownloadTransport{observation: &observation, input: row.Input, cancel: cancel}
	reader := &fanboxContentReader{input: strings.NewReader(row.Input.Stdin), observation: &observation.fanboxContentObservation, fail: row.Input.StdinError}
	writer := &fanboxDownloadWriter{observation: &observation, input: row.Input, remaining: row.Input.WriteLimit}
	var diagnostics bytes.Buffer
	clients := map[*fanboxsdk.Client]struct{}{}
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
		if row.Input.StartupError {
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
		if row.Input.Factory == "error" {
			return nil, errors.New("owned service factory failure")
		}
		if row.Input.Factory == "nil" {
			return nil, nil
		}
		a.closeState.add(func() error {
			observation.Trace = append(observation.Trace, "root.cleanup.before-database")
			return nil
		})
		accounts, err := a.newFanboxAccountService()
		if err != nil {
			return nil, err
		}
		a.closeState.add(func() error {
			observation.Trace = append(observation.Trace, "root.cleanup.after-database")
			if row.Input.RootCloseError {
				return errors.New("owned root cleanup failure")
			}
			return nil
		})
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
		return fanboxapp.NewFacadeWithCloseClient(fanboxDownloadOpener{accounts: accounts, observation: &observation, clients: clients, mode: row.Input.Open}, func(client *fanboxsdk.Client) error {
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
		observation.Exits = append(observation.Exits, RunContext(ctx, append([]string{"pixiv"}, row.Input.Args...), reader, writer, &diagnostics))
	}
	if transport.unexpected != "" {
		t.Fatal(transport.unexpected)
	}
	observation.OutputIsTTY = outputIsTTY(writer)
	observation.Stdout = fanboxContentErrorText(writer.buffer.String(), home)
	observation.Stderr = fanboxContentErrorText(diagnostics.String(), home)
	for i := range observation.OutputWrites {
		observation.OutputWrites[i].Bytes = fanboxContentErrorText(observation.OutputWrites[i].Bytes, home)
	}
	observation.ReplyCount = transport.index
	observation.DBAfter = fanboxContentSHA(t, dbPath)
	observation.DBRowsAfter = fanboxContentDBRows(t, dbPath)
	config, err := os.ReadFile(filepath.Join(home, ".pixiv-cli", "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	observation.ConfigAfter = string(config)
	if !bytes.Equal(config, []byte(row.Input.Config)) {
		t.Fatal("download changed saved config")
	}
	if !reflect.DeepEqual(observation.DBRowsBefore, observation.DBRowsAfter) {
		t.Fatal("download changed saved account/schema rows")
	}
	observation.FilesAfter = fanboxDownloadFiles(t, home, row.Input.OutputRoot)
	savedFiles := map[string]bool{}
	for _, file := range observation.FilesAfter {
		if file.Kind == "file" {
			savedFiles[file.Path] = true
		}
	}
	for _, write := range observation.OutputWrites {
		if strings.HasPrefix(write.Bytes, "saved: ") {
			path := filepath.ToSlash(filepath.Clean(strings.TrimSuffix(strings.TrimPrefix(write.Bytes, "saved: "), "\n")))
			if !savedFiles[path] {
				t.Fatalf("saved output has no published file in owned root: %s", path)
			}
		}
	}
	for _, file := range observation.FilesAfter {
		if strings.HasPrefix(filepath.Base(file.Path), ".atomic-write-") {
			observation.Temps = append(observation.Temps, file.Path)
		}
	}
	entries, err := os.ReadDir(os.Getenv("TMPDIR"))
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		observation.Temps = append(observation.Temps, "temp/"+entry.Name())
	}
	if len(observation.Temps) != 0 {
		t.Fatal("download left temporary files")
	}
	if strings.Contains(observation.Stdout+observation.Stderr, "owned-session") || strings.Contains(observation.Stdout+observation.Stderr, "secret-session") {
		t.Fatal("download output exposed saved session")
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

func fanboxDownloadJSON(value any) string {
	body, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	return string(body)
}
func fanboxDownloadRows() []fanboxDownloadCase {
	rows := []fanboxDownloadCase{}
	add := func(name string, sources []string, replies ...fanboxDownloadReply) *fanboxDownloadCase {
		args := append([]string{"fanbox", "download"}, sources...)
		rows = append(rows, fanboxDownloadCase{Name: name, Input: fanboxDownloadInput{Args: args, Config: "[fanbox.auth]\ndefault_user_id=42\n", Saved: "explicit", Replies: append([]fanboxDownloadReply{}, replies...), OutputRoot: "downloads", SeedFiles: []fanboxDownloadFile{}}})
		return &rows[len(rows)-1]
	}
	post := func(id, creator string, body any) map[string]any {
		return map[string]any{"id": id, "creatorId": creator, "title": "owned post", "publishedDatetime": "2024-01-02T03:04:05Z", "body": body}
	}
	image := func(id, url string) map[string]any {
		return map[string]any{"id": id, "extension": "ignored", "originalUrl": url, "thumbnailUrl": "https://i.pximg.net/owned/never-thumbnail.jpg"}
	}
	file := func(id, name, url string) map[string]any {
		return map[string]any{"id": id, "name": name, "extension": "ignored", "url": url}
	}
	info := func(p any) fanboxDownloadReply {
		return fanboxDownloadReply{fanboxContentReply: fanboxContentReply{Body: fanboxDownloadJSON(map[string]any{"body": map[string]any{"post": p}})}}
	}
	page := func(posts []any, next string) fanboxDownloadReply {
		return fanboxDownloadReply{fanboxContentReply: fanboxContentReply{Body: fanboxDownloadJSON(map[string]any{"body": map[string]any{"posts": posts, "nextUrl": next}})}}
	}
	media := func(url, body string) fanboxDownloadReply {
		parsed, err := urlpkg.Parse(url)
		if err != nil {
			panic(err)
		}
		return fanboxDownloadReply{fanboxContentReply: fanboxContentReply{Body: body}, URL: parsed.String(), ContentType: "image/png", ContentLength: 999}
	}
	imageURL := "https://downloads.fanbox.cc/owned/Image.PNG?query=ignored#fragment"
	imageRequestURL := "https://downloads.fanbox.cc/owned/Image.PNG?query=ignored#fragment"
	imageBody := map[string]any{"images": []any{image("image-one", imageURL)}}
	p := post("123", "creator-one", imageBody)
	seedInfo := info(p)
	seedPage := page([]any{p}, "")
	seedMedia := media(imageRequestURL, "owned image\x00\n")
	for _, item := range []struct {
		name, source string
		single       bool
	}{
		{"numeric", "123", true}, {"leading-zero", "00123", true}, {"creator", "creator-one", false}, {"creator-url", "https://www.fanbox.cc/@creator-one", false}, {"creator-posts-url", "https://www.fanbox.cc/@creator-one/posts", false}, {"post-url", "https://www.fanbox.cc/@creator-one/posts/123", true}, {"creator-subdomain-path", "https://creator-one.fanbox.cc/posts/123", false}, {"tag-url", "https://www.fanbox.cc/@creator-one/posts/tag/blue%20sky", false}, {"unicode-numeric-creator", "１２３", false}, {"space-numeric-creator", " 123 ", false}, {"uppercase-scheme-creator", "HTTPS://www.fanbox.cc/@creator-one", false}, {"media-subdomain-is-creator", "https://downloads.fanbox.cc/owned/Image.PNG", false},
	} {
		reply := seedPage
		if item.single {
			reply = seedInfo
		}
		add("source-"+item.name, []string{item.source}, reply, seedMedia)
	}
	for _, item := range []struct{ name, source string }{
		{"http-rejected", "http://www.fanbox.cc/@creator-one"}, {"foreign-host", "https://evil.invalid/@creator-one"}, {"unknown-path", "https://www.fanbox.cc/unknown"}, {"userinfo", "https://owned:password@www.fanbox.cc/@creator-one"}, {"legacy-host", "https://www.pixiv.net/fanbox/creator/1"}, {"tag-missing", "https://www.fanbox.cc/@creator-one/posts/tag"}, {"whitespace", "   "}, {"empty", ""},
	} {
		add("source-invalid-"+item.name, []string{item.source})
	}
	next := "https://api.fanbox.cc/owned-next?z=2&x=kept%20space"
	p2 := post("124", "creator-one", imageBody)
	p3 := post("125", "creator-two", imageBody)
	add("pagination-all-pages", []string{"creator-one"}, page([]any{p}, next), seedMedia, page([]any{p2}, ""), seedMedia)
	add("pagination-empty-next", []string{"creator-one"}, page([]any{}, next), page([]any{p}, ""), seedMedia)
	add("pagination-empty-final", []string{"creator-one"}, page([]any{}, ""))
	add("pagination-repeat-cursor-keeps-files", []string{"creator-one"}, page([]any{p}, next), seedMedia, page([]any{p2}, next), seedMedia)
	badPage := page([]any{map[string]any{"title": "missing id"}}, "")
	add("pagination-page-two-decode-failure-keeps-files", []string{"creator-one"}, page([]any{p}, next), seedMedia, badPage)
	failPage := page([]any{}, "")
	failPage.TransportError = "owned page two transport failure"
	add("pagination-page-two-transport-failure-keeps-files", []string{"creator-one"}, page([]any{p}, next), seedMedia, failPage)
	add("pagination-unsafe-next-keeps-files", []string{"creator-one"}, page([]any{p}, "https://evil.invalid/next"), seedMedia)
	add("multiple-sources-one-client", []string{"123", "creator-one", "https://www.fanbox.cc/@creator-two/posts/tag/red"}, seedInfo, seedMedia, page([]any{p, p2}, ""), seedMedia, page([]any{p3}, ""), seedMedia)
	add("multiple-sources-fail-fast", []string{"123", "http://www.fanbox.cc/@bad", "creator-never"}, seedInfo, seedMedia)
	add("multiple-sources-duplicate-same-path", []string{"123", "123"}, seedInfo, seedMedia, seedInfo)
	for _, item := range []struct {
		name string
		body any
	}{
		{"nil", nil}, {"empty", map[string]any{}}, {"text", map[string]any{"text": "text is not saved"}}, {"embed-only", map[string]any{"blocks": []any{map[string]any{"type": "article"}, map[string]any{"type": "video"}, map[string]any{"type": "unexpected"}}}}, {"empty-image-list", map[string]any{"images": []any{}}}, {"empty-file-list", map[string]any{"files": []any{}}},
	} {
		add("body-"+item.name, []string{"123"}, info(post("123", "creator-one", item.body)))
	}
	restricted := post("123", "creator-one", imageBody)
	restricted["isRestricted"] = true
	add("body-restricted-with-body-still-saves", []string{"123"}, info(restricted), seedMedia)
	restrictedNil := post("123", "..", nil)
	restrictedNil["isRestricted"] = true
	add("body-nil-skips-invalid-identity", []string{"123"}, info(restrictedNil))
	fileURL := "https://downloads.fanbox.cc/owned/archive.ZIP?name=ignored"
	fileBody := map[string]any{"files": []any{file("file-one", " report / name:*?\"<>|\\.txt ", fileURL)}}
	add("body-file-name-sanitized", []string{"123"}, info(post("123", "creator-one", fileBody)), media(fileURL, "owned archive"))
	add("body-explicit-list-duplicates-share-last-cache", []string{"123"}, info(post("123", "creator-one", map[string]any{"images": []any{image("same", imageURL), image("same", "https://downloads.fanbox.cc/owned/rotated.JPG")}})), media("https://downloads.fanbox.cc/owned/rotated.JPG", "cached last URL"), media("https://downloads.fanbox.cc/owned/rotated.JPG", "second extension path"))
	blog := map[string]any{"blocks": []any{map[string]any{"type": "file", "fileId": "f"}, map[string]any{"type": "image", "imageId": "i"}, map[string]any{"type": "unexpected", "imageId": "i", "fileId": "f"}, map[string]any{"type": "image", "imageId": "i"}}, "imageMap": map[string]any{"i": image("i", imageURL), "unreferenced": image("unused", "https://downloads.fanbox.cc/owned/never-unused.png")}, "fileMap": map[string]any{"f": file("f", "attachment.dat", fileURL)}}
	add("body-blog-mixed-images-before-files-dedup", []string{"123"}, info(post("123", "creator-one", blog)), seedMedia, media(fileURL, "blog file"))
	add("body-blog-missing-image", []string{"123"}, info(post("123", "creator-one", map[string]any{"blocks": []any{map[string]any{"type": "image", "imageId": "missing"}}})))
	add("body-blog-missing-file", []string{"123"}, info(post("123", "creator-one", map[string]any{"blocks": []any{map[string]any{"type": "file", "fileId": "missing"}}})))
	precedence := map[string]any{"images": []any{}, "files": []any{file("f", "attachment.dat", fileURL)}, "blocks": []any{map[string]any{"type": "image", "imageId": "missing"}}}
	add("body-empty-images-win-over-files-blocks", []string{"123"}, info(post("123", "creator-one", precedence)))
	precedence["images"] = []any{image("image-one", imageURL)}
	add("body-images-win-over-files-blocks", []string{"123"}, info(post("123", "creator-one", precedence)), seedMedia)
	delete(precedence, "images")
	precedence["files"] = []any{}
	add("body-empty-files-win-over-blocks", []string{"123"}, info(post("123", "creator-one", precedence)))
	for _, item := range []struct{ name, url string }{
		{"lowercase-query-fragment", imageURL}, {"no-extension", "https://downloads.fanbox.cc/owned/noextension?ext=.png"}, {"dot-only", "https://downloads.fanbox.cc/owned/name."}, {"one-byte-extension", "https://downloads.fanbox.cc/owned/name.X"}, {"max-length-extension", "https://downloads.fanbox.cc/owned/name.ABCDEFGHIJK"}, {"overlong-extension", "https://downloads.fanbox.cc/owned/name.ABCDEFGHIJKL"}, {"percent-encoded-extension", "https://downloads.fanbox.cc/owned/name%2EPNG"}, {"unicode-extension", "https://downloads.fanbox.cc/owned/name.色"},
	} {
		add("filename-id-fallback-"+item.name, []string{"123"}, info(post("123", "creator-one", map[string]any{"images": []any{image("fallback", item.url)}})), media(item.url, "fallback"))
	}
	for _, name := range []string{"", "   ", ".", "..", " CON. ", "unicode色\t.bin", "bad\x00name"} {
		label := hex.EncodeToString([]byte(name))
		if label == "" {
			label = "empty"
		}
		add("filename-explicit-"+label, []string{"123"}, info(post("123", "creator-one", map[string]any{"files": []any{file("file-one", name, fileURL)}})), media(fileURL, "named file"))
	}
	add("filename-overlong-os-error", []string{"123"}, info(post("123", "creator-one", map[string]any{"files": []any{file("file-one", strings.Repeat("n", 256), fileURL)}})), media(fileURL, "never final"))
	for _, item := range []struct{ name, id, creator string }{
		{"creator-empty", "123", ""}, {"creator-whitespace", "123", "   "}, {"creator-dot", "123", "."}, {"creator-dotdot", "123", ".."}, {"post-dot", ".", "creator-one"}, {"post-dotdot", "..", "creator-one"}, {"post-empty-wire-reject", "", "creator-one"}, {"sanitized", " ../creator:/\\*?\"<>| ", " creator:/\\*?\"<>| "}, {"unicode-trim", "\u00a0123\u00a0", "\u2003色\u2003"},
	} {
		add("identity-"+item.name, []string{"123"}, info(post(item.id, item.creator, map[string]any{"text": "empty but validates identity"})))
	}
	collidingA := post("same?", "creator/", map[string]any{"files": []any{file("a", "same?.bin", fileURL)}})
	collidingB := post("same*", "creator\\", map[string]any{"files": []any{file("b", "same*.bin", "https://downloads.fanbox.cc/owned/never-collision.bin")}})
	add("filename-cross-source-sanitized-collision", []string{"one", "two"}, page([]any{collidingA}, ""), media(fileURL, "first owns path"), page([]any{collidingB}, ""))
	for _, item := range []struct{ name, body string }{
		{"zero-ref-empty-url-wire-rejected", `{"images":[{"id":"i","originalUrl":""}]}`}, {"zero-ref-missing-id-wire-rejected", `{"images":[{"originalUrl":"https://downloads.fanbox.cc/owned/image.png"}]}`}, {"media-foreign-host-rejected", `{"images":[{"id":"i","originalUrl":"https://evil.invalid/image.png"}]}`}, {"thumbnail-foreign-rejects-unused", `{"images":[{"id":"i","originalUrl":"https://downloads.fanbox.cc/owned/image.png","thumbnailUrl":"https://evil.invalid/thumbnail.jpg"}]}`},
	} {
		var body any
		if err := json.Unmarshal([]byte(item.body), &body); err != nil {
			panic(err)
		}
		add("assets-"+item.name, []string{"123"}, info(post("123", "creator-one", body)))
	}
	for _, host := range []string{"i.pximg.net", "pixiv.pximg.net"} {
		url := "https://" + host + "/owned/public.png"
		add("assets-public-host-"+host, []string{"123"}, info(post("123", "creator-one", map[string]any{"images": []any{image("public", url)}})), media(url, "public bytes"))
	}
	invalidDate := post("123", "creator-one", imageBody)
	invalidDate["publishedDatetime"] = "invalid"
	add("post-invalid-date-before-output", []string{"123"}, info(invalidDate))
	for _, kind := range []string{"transport", "read", "bytes-and-read-error", "close-ignored", "304", "401", "403", "500", "empty", "bytes-and-eof", "cancel-during-read"} {
		reply := seedMedia
		switch kind {
		case "transport":
			reply.TransportError = "owned media transport failure"
		case "read":
			reply.ReadError = "owned media body failure"
		case "bytes-and-read-error":
			reply.ReadError = "owned media body failure"
			reply.BytesAndError = true
		case "close-ignored":
			reply.CloseError = "owned media close failure"
		case "304":
			reply.Status = 304
		case "401":
			reply.Status = 401
		case "403":
			reply.Status = 403
		case "500":
			reply.Status = 500
		case "empty":
			reply.Body = ""
		case "bytes-and-eof":
			reply.BytesAndEOF = true
		case "cancel-during-read":
			reply.ChunkSize = 3
			reply.CancelAtRead = 1
		}
		add("save-"+kind, []string{"123"}, seedInfo, reply)
	}
	cancelEOF := seedMedia
	cancelEOF.BytesAndEOF = true
	cancelEOF.CancelAtRead = 1
	add("save-cancel-at-eof-still-publishes", []string{"123"}, seedInfo, cancelEOF)
	add("pagination-cancel-after-published-asset", []string{"creator-one"}, page([]any{p}, next), cancelEOF)
	for _, kind := range []string{"overwrite", "preserve-after-read-error", "destination-directory", "creator-file-blocker", "root-file-blocker"} {
		reply := seedMedia
		row := add("filesystem-"+kind, []string{"123"}, seedInfo, reply)
		seed := fanboxDownloadFile{Path: "downloads/fanbox/creator-one/123/image-one.png", Kind: "file", Mode: "0644", BytesHex: hex.EncodeToString([]byte("old destination"))}
		switch kind {
		case "preserve-after-read-error":
			row.Input.Replies[1].ReadError = "owned later read failure"
			row.Input.Replies[1].BytesAndError = true
		case "destination-directory":
			seed.Kind = "directory"
			seed.BytesHex = ""
			seed.Mode = "0755"
		case "creator-file-blocker":
			seed.Path = "downloads/fanbox/creator-one"
		case "root-file-blocker":
			seed.Path = "downloads"
		}
		row.Input.SeedFiles = append(row.Input.SeedFiles, seed)
	}
	badMedia := seedMedia
	badMedia.ReadError = "owned second asset failure"
	badMedia.BytesAndError = true
	add("save-later-post-failure-keeps-prefix", []string{"creator-one"}, page([]any{p, p2, p3}, next), seedMedia, badMedia)
	secondURL := "https://downloads.fanbox.cc/owned/second.PNG"
	neverURL := "https://downloads.fanbox.cc/owned/never-third.PNG"
	secondFailure := media(secondURL, "partial second bytes")
	secondFailure.ReadError = "owned second asset failure"
	secondFailure.BytesAndError = true
	multipleAssets := post("123", "creator-one", map[string]any{"images": []any{image("image-one", imageURL), image("image-two", secondURL), image("image-three", neverURL)}})
	add("save-later-asset-failure-keeps-prefix", []string{"123", "creator-never"}, info(multipleAssets), seedMedia, secondFailure)
	add("pagination-whole-batch-decode-before-any-save", []string{"creator-one"}, page([]any{p, map[string]any{"title": "missing id"}}, next))
	for _, writer := range []string{"error", "short", "pipe"} {
		row := add("writer-"+writer+"-ignored", []string{"123", "124"}, seedInfo, seedMedia, info(p2), seedMedia)
		row.Input.Writer = writer
		row.Input.WriteLimit = 8
		row = add("writer-"+writer+"-zero-ignored", []string{"123"}, seedInfo, seedMedia)
		row.Input.Writer = writer
	}
	for _, item := range []struct {
		name, config string
		flags        []string
	}{
		{"default-sort-order", "", nil}, {"missing-default", "[fanbox.auth]\ndefault_user_id=999\n", nil}, {"invalid-default", "[fanbox.auth]\ndefault_user_id=0\n", nil}, {"global-proxy", "[network]\nhttps_proxy='http://global.invalid:8080'\n", nil}, {"service-empty-proxy", "[network]\nhttps_proxy='http://global.invalid'\n[fanbox.network]\nproxy_url=''\n", nil}, {"service-proxy", "[fanbox.network]\nproxy_url='https://service.invalid:8081'\n", nil}, {"command-proxy", "[fanbox.network]\nproxy_url='http://service.invalid'\n", []string{"--proxy=https://command.invalid:8082"}}, {"command-empty-proxy", "[fanbox.network]\nproxy_url='ftp://invalid.invalid'\n", []string{"--proxy="}}, {"no-proxy", "[fanbox.network]\nproxy_url='ftp://invalid.invalid'\n", []string{"--no-proxy"}}, {"no-proxy-false", "[fanbox.network]\nproxy_url='http://service.invalid'\n", []string{"--no-proxy=false"}}, {"invalid-native-proxy", "[fanbox.network]\nproxy_url='ftp://invalid.invalid'\n", nil}, {"agent-solver-independent", "[fanbox.network]\nuser_agent='owned-UA/1'\n[fanbox.flaresolverr]\nurl='http://solver.invalid:8191/'\nproxy_url='http://solver-proxy.invalid:8181'\n", []string{"--no-proxy"}}, {"invalid-agent-toml", "[fanbox.network]\nuser_agent='bad\u0001agent'\n", nil}, {"control-agent-custom-transport", "[fanbox.network]\nuser_agent=\"bad\\u0001agent\"\n", nil}, {"invalid-native-agent", "[fanbox.network]\nuser_agent=\"bad\\ragent\"\n", nil}, {"invalid-solver-url", "[fanbox.flaresolverr]\nurl='https://user:password@solver.invalid'\n", nil}, {"malformed", "[unfinished\n", nil}, {"invalid-pool", "[account_pool]\nenabled='bad'\n", nil}, {"output-json-template-ignored", "[output]\njson=true\n[download]\nfilename_template='never-template'\ndirectory_template='never-directory'\n", nil}, {"custom-download-path", "[download]\npath='./owned-output'\n", nil},
	} {
		row := add("config-"+item.name, append([]string{"123"}, item.flags...), seedInfo, seedMedia)
		row.Input.Config = item.config
		if item.name == "custom-download-path" {
			row.Input.OutputRoot = "owned-output"
		}
	}
	row := add("proxy-flags-before-leaf", []string{"123"}, seedInfo, seedMedia)
	row.Input.Args = []string{"fanbox", "--no-proxy", "download", "123"}
	row = add("proxy-conflict-even-false", []string{"123", "--proxy=", "--no-proxy=false"}, seedInfo, seedMedia)
	for _, saved := range []string{"none", "empty", "invalid"} {
		row := add("saved-"+saved+"-no-pixiv-fallback", []string{"123"}, seedInfo, seedMedia)
		row.Input.Saved = saved
		if saved == "none" {
			row.Input.Config = "[account_pool]\nenabled=true\nstrategy='round_robin'\n"
		}
	}
	for _, failure := range []string{"corrupt", "missing-table"} {
		row := add("database-"+failure, []string{"123"}, seedInfo, seedMedia)
		row.Input.DBFailure = failure
		row.Input.OptionsError = true
	}
	add("options-failure", []string{"123"}, seedInfo, seedMedia).Input.OptionsError = true
	row = add("missing-account-before-options", []string{"123"}, seedInfo, seedMedia)
	row.Input.Saved = "none"
	row.Input.Config = ""
	row.Input.OptionsError = true
	for _, kind := range []string{"error", "nil"} {
		add("factory-"+kind, []string{"123"}).Input.Factory = kind
	}
	for _, kind := range []string{"error", "nil", "client-error"} {
		add("opener-"+kind, []string{"123"}).Input.Open = kind
	}
	add("lease-close-after-success", []string{"123"}, seedInfo, seedMedia).Input.CloseError = true
	row = add("save-and-lease-close-join", []string{"123"}, seedInfo, badMedia)
	row.Input.CloseError = true
	add("root-close-after-success", []string{"123"}, seedInfo, seedMedia).Input.RootCloseError = true
	row = add("save-lease-root-close-join", []string{"123"}, seedInfo, badMedia)
	row.Input.CloseError = true
	row.Input.RootCloseError = true
	for _, kind := range []string{"before", "deadline", "transport"} {
		add("cancel-"+kind, []string{"123"}, seedInfo, seedMedia).Input.Cancel = kind
	}
	for _, flagValue := range []string{"--json", "--ndjson", "--page=1", "--limit=1", "--output=out", "--filename-template=x", "--directory-template=x", "--record=x", "--on-error=continue", "--unknown", "-x"} {
		row := add("flag-rejected-"+strings.ReplaceAll(strings.TrimLeft(flagValue, "-"), "=", "-"), []string{"123", flagValue})
		row.Input.Config = "[unfinished\n"
		row.Input.StartupError = true
		row.Input.StdinError = true
	}
	for _, item := range []struct {
		name string
		args []string
	}{{"plain", []string{"fanbox", "download", "--help"}}, {"positional", []string{"fanbox", "download", "123", "--help"}}, {"then-json", []string{"fanbox", "download", "--help", "--json"}}, {"unknown-then-help", []string{"fanbox", "download", "--unknown", "--help"}}} {
		row := add("help-"+item.name, nil)
		row.Input.Args = item.args
		row.Input.Config = "[unfinished\n"
		row.Input.StartupError = true
		row.Input.StdinError = true
	}
	for _, item := range []struct{ name, value string }{
		{"lf", "123\n"}, {"crlf", "123\r\n"}, {"no-newline", "123"}, {"one-terminal-lf-only", "123\n\n"}, {"bare-cr-preserved", "123\r"}, {"spaces-no-split", "123 creator-one\n"}, {"records-lookalike-one-text-value", "{\"id\":123}\n{\"id\":124}\n"}, {"hyphen-not-sentinel", "-\n"}, {"empty", ""},
	} {
		replies := []fanboxDownloadReply{seedInfo, seedMedia}
		if item.name != "lf" && item.name != "crlf" && item.name != "no-newline" {
			replies = []fanboxDownloadReply{seedPage, seedMedia}
		}
		row := add("stdin-"+item.name, nil, replies...)
		row.Input.Stdin = item.value
	}
	add("stdin-read-failure-before-startup", nil).Input.StdinError = true
	row = add("explicit-source-never-reads-stdin", []string{"123"}, seedInfo, seedMedia)
	row.Input.StdinError = true
	row = add("startup-before-malformed-config", []string{"123"}, seedInfo, seedMedia)
	row.Input.StartupError = true
	row.Input.Config = "[unfinished\n"
	row = add("independent-root-executions", []string{"123"}, seedInfo, seedMedia, seedInfo, seedMedia)
	row.Input.Repeat = 2
	for _, writer := range []string{"error", "short", "pipe"} {
		row := add("help-writer-"+writer+"-ignored", nil)
		row.Input.Args = []string{"fanbox", "download", "--help"}
		row.Input.Config = "[unfinished\n"
		row.Input.StartupError = true
		row.Input.StdinError = true
		row.Input.Writer = writer
		row.Input.WriteLimit = 8
	}
	for _, flagValue := range []string{"-hh", "-hx"} {
		row := add("help-short-cluster-"+strings.TrimPrefix(flagValue, "-"), []string{flagValue})
		row.Input.Config = "[unfinished\n"
		row.Input.StartupError = true
		row.Input.StdinError = true
	}
	for _, flagValue := range []string{"-h=false", "-hh=false"} {
		row := add("help-short-false-"+strings.ReplaceAll(strings.TrimPrefix(flagValue, "-"), "=", "-"), []string{"123", flagValue}, seedInfo, seedMedia)
		row.Input.StdinError = true
	}
	row = add("help-short-empty-equals", []string{"-h="})
	row.Input.Config = "[unfinished\n"
	row.Input.StartupError = true
	row.Input.StdinError = true
	return rows
}

func fanboxDownloadPublishedFixtures(t *testing.T) map[string]string {
	t.Helper()
	listing, err := exec.Command("git", "ls-tree", "-r", "--full-tree", "--name-only", fanboxDownloadPublished).Output()
	if err != nil {
		t.Fatal(err)
	}
	hashes := map[string]string{}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if !strings.Contains(path, "/tests/fixtures/") {
			continue
		}
		frozen, err := exec.Command("git", "show", fanboxDownloadPublished+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		current, err := os.ReadFile(filepath.Join("../..", path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(frozen, current) {
			t.Fatalf("published fixture changed: %s", path)
		}
		hashes[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	if len(hashes) != 95 {
		t.Fatalf("published fixture guard has %d files", len(hashes))
	}
	return hashes
}
func TestMigrationFanboxDownload(t *testing.T) {
	sources, all := fanboxContentSources(t)
	fixture := fanboxDownloadFixture{Reference: fanboxContentReference, Base: fanboxDownloadPublished, Environment: "linux/amd64; TZ=Asia/Tokyo; umask=0022; genuine root RunContext, owned saved SQLite/config/output and synthetic net/http transport", Sources: sources, FrozenGoProduction: all, PublishedFixtures: fanboxDownloadPublishedFixtures(t), Cases: fanboxDownloadRows(), Limitations: []string{
		"Every workflow row executes genuine root RunContext/newRootCommand, the actual download leaf, normal startup/config/pipeline, real saved SQLite selection/options, Facade lease, public SDK post/list/resource/save methods and atomic local files. No returned post/resource/save DTO is mocked.",
		"Existing production factory seam supplies the real account service and Facade with the public custom HTTP transport/close ports. Factory/opener fault rows inject failures only at their documented dependency boundary. Root cleanup trace canaries bracket actual database ownership; they are not a replacement database.",
		"Process-wide socket/connect/exec denial follows owned synthetic DB/config setup and is reused unchanged from help routing. Startup cleanup and platform support use safe canaries; automatic update construction, native handler, browser extraction and auth mutation are forbidden. Development-build automatic update is skipped by genuine Go policy. Actual cmd/pixiv process/native startup and signals remain separate proof obligations.",
		"Only owned relative download roots are written. File snapshots are exact relative path, bytes as hex, SHA256, size and mode including empty directories, old-final preservation, successful prefixes and temp cleanup. Umask is fixed to 0022. Symlink containment, reserved-device behavior, disk-full, sync/chmod faults and non-Linux filesystem/platform modes are unproved.",
		"Output writer rows supply error/short/EPIPE from a writer at the actual root leaf. The Go download ignores fmt.Fprintf results, proceeds to later files and exits successfully absent another error. No OS-generated SIGPIPE is claimed by these synthetic writer observations.",
		"Wire validation rejects missing/empty asset IDs and empty media locators before the CLI. Thus zero ResourceRef skip, ID-empty asset/asset-extension fallback and an arbitrary nonzero asset-kind DTO are unreachable through real endpoint replies. No fabricated DTO was used to claim root coverage of those direct helper-only branches.",
		"HTTP replies are synthetic and bounded. Credential/media policy is traversed using actual first-party refs and requests but no external media/account/browser/system trust/HKCU or denied native multiplex/unfinished HEAD/upload probe runs. SDK save sibling owns deeper public SaveResource producer contracts.",
		"Go production and module/sum bytes are guarded against the frozen 434-file manifest; all 95 published fixture files at the declared base are guarded byte-for-byte. Real database opens preserve physical SQLite hashes while saved account/schema/Pixiv canary rows and config are required unchanged.",
		"Low-level API decoder read capacity is recorded as an observation, not a new mandated cross-language implementation API; media 64-KiB producer reads, exact bytes and close/lease ownership are save-boundary evidence. Custom transports may accept header controls a native transport rejects. Native networking is not inferred from custom transport success.",
		"Inherited content-observation errors/reasons are empty because public RunContext returns only an exit code; exact root stderr carries errors. Output and path-bearing diagnostics replace only the owned randomized HOME. API/media request bytes, writer bytes on relative paths and file bytes remain exact.",
	}}
	names := map[string]bool{}
	for i, row := range fixture.Cases {
		if names[row.Name] {
			t.Fatalf("duplicate capture case %s", row.Name)
		}
		names[row.Name] = true
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxDownloadChild$", "-test.count=1", "-test.timeout=8s")
			child.WaitDelay = time.Second
			child.Dir = home
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "TMPDIR=" + filepath.Join(home, "temp"), "PATH=", "TZ=Asia/Tokyo", "LANG=C", "LC_ALL=C", "GOMAXPROCS=2", "GORACE=atexit_sleep_ms=0", "MIGRATION_FANBOX_DOWNLOAD_CHILD=" + string(body)}
			if output, err := child.CombinedOutput(); err != nil {
				t.Fatalf("owned download child: %v\n%s", err, output)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &fixture.Cases[i]); err != nil {
				t.Fatal(err)
			}
			actual := fixture.Cases[i].Observation
			if !actual.SocketDenied || !actual.ExecDenied || len(actual.Temps) != 0 {
				t.Fatal("download isolation/temp guard failed")
			}
			if strings.HasPrefix(row.Name, "source-") && !strings.HasPrefix(row.Name, "source-invalid-") {
				files := 0
				for _, file := range actual.FilesAfter {
					if file.Kind == "file" {
						files++
					}
				}
				if !reflect.DeepEqual(actual.Exits, []int{0}) || files != 1 || actual.Stdout == "" || len(actual.Requests) != 2 || actual.LeaseCloses != 1 || actual.UniqueClients != 1 {
					t.Fatalf("source seed did not establish genuine root save: %+v", actual)
				}
			}
			if strings.HasPrefix(row.Name, "flag-rejected-") && (!reflect.DeepEqual(actual.Exits, []int{2}) || len(actual.Trace) != 0) {
				t.Fatal("unknown flag did not stop before hooks")
			}
			if strings.HasPrefix(row.Name, "writer-") && !reflect.DeepEqual(actual.Exits, []int{0}) {
				t.Fatal("ignored output failure became command failure")
			}
			if strings.HasPrefix(row.Name, "help-writer-") {
				if !reflect.DeepEqual(actual.Exits, []int{0}) || actual.Stdout != "Download" || actual.Stderr != "" || len(actual.OutputWrites) != 3 || actual.StdinReads != 0 || len(actual.Requests) != 0 || !reflect.DeepEqual(actual.Trace, []string{"stdout.write", "stdout.write", "stdout.write"}) {
					t.Fatalf("help did not preserve three ignored writes before hooks: %+v", actual)
				}
				if !reflect.DeepEqual(actual.Writes, []int{44, 1, 254}) {
					t.Fatal("help emission write boundaries changed")
				}
			}
		})
	}
	if t.Failed() {
		return
	}
	for i := range fixture.Cases {
		sort.Strings(fixture.Cases[i].Observation.Temps)
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := "../../crates/pixiv-cli/tests/fixtures/fanbox-download.json"
	if *captureFanboxDownload {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile("/tmp/pixiv-fanbox-download-cli-go.json", body, 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, body) {
		t.Fatal("genuine saved FANBOX root download differs from frozen Go capture")
	}
}
