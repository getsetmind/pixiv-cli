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
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	pixivaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	pixivpool "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
	reverseassembly "github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch/assembly"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/record"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var captureReverseSearchCLI = flag.Bool("migration-capture-reverse-search-cli", false, "capture frozen connected reverse-search CLI contracts")

const reverseCLIReference = "4b4426487ef18bed276706daec385e0d0a6979f9"
const reverseCLIPublished = "99246d8bfb8712caa0903426953423850836cefe"
const reverseCLISource = "https://owned-source.invalid/image.png?synthetic-source-secret"
const reverseCLIPayload = "owned synthetic reverse-search payload\n"

type reverseCLIInput struct {
	Args           []string                                                  `json:"args"`
	Stdin          string                                                    `json:"stdin"`
	StdinFailure   bool                                                      `json:"stdin_failure,omitempty"`
	Config         *string                                                   `json:"config_before"`
	Environment    map[string]string                                         `json:"environment_overrides"`
	SavedAccounts  []int64                                                   `json:"saved_accounts"`
	SDKSearchBody  string                                                    `json:"sdk_search_body,omitempty"`
	Replies        map[reversesearch.Provider]reversesearch.ProviderResponse `json:"provider_replies"`
	Failures       map[string]string                                         `json:"failures"`
	Cancel         string                                                    `json:"cancel,omitempty"`
	SourceStatus   int                                                       `json:"source_status,omitempty"`
	Writer         string                                                    `json:"writer,omitempty"`
	WarningWriter  string                                                    `json:"warning_writer,omitempty"`
	WriteLimit     int                                                       `json:"write_limit,omitempty"`
	OutputPipe     bool                                                      `json:"output_pipe,omitempty"`
	StartupFailure bool                                                      `json:"startup_failure,omitempty"`
	ConsumeRecords bool                                                      `json:"consume_records,omitempty"`
}

type reverseCLIPayloadObservation struct {
	Kind   reversesearch.SourceKind `json:"kind"`
	SHA256 string                   `json:"sha256"`
	Size   int64                    `json:"size"`
	Reads  []string                 `json:"reads_hex"`
}

type reverseCLIProviderObservation struct {
	Preflights int                            `json:"preflights"`
	Searches   int                            `json:"searches"`
	Uploads    int                            `json:"uploads"`
	Payloads   []reverseCLIPayloadObservation `json:"payloads"`
	Closes     int                            `json:"closes"`
}

type reverseCLIHTTPRequest struct {
	Method string              `json:"method"`
	URL    string              `json:"url"`
	Header http.Header         `json:"headers"`
	Form   map[string][]string `json:"form,omitempty"`
}

type reverseCLIAccountState struct {
	ID          int64  `json:"id"`
	Revision    int64  `json:"revision"`
	Refresh     string `json:"refresh"`
	Selected    bool   `json:"selected"`
	Schedulable bool   `json:"schedulable"`
}

type reverseCLIObservation struct {
	Exit                int                                       `json:"exit"`
	Stdout              string                                    `json:"stdout"`
	Stderr              string                                    `json:"stderr"`
	OutputWrites        []int                                     `json:"output_writes"`
	ErrorWrites         []int                                     `json:"error_writes"`
	StdinReads          int                                       `json:"stdin_reads"`
	StdinBytes          int                                       `json:"stdin_bytes"`
	StartupCalls        int                                       `json:"startup_calls"`
	HandlerChecks       int                                       `json:"handler_checks"`
	AutomaticChecks     int                                       `json:"automatic_checks"`
	Constructors        []reverseassembly.Options                 `json:"constructors"`
	Requests            []reversesearch.Request                   `json:"requests"`
	SearchResponses     []reversesearch.Response                  `json:"search_responses"`
	SearchErrors        []string                                  `json:"search_errors"`
	SourceRequests      []reverseCLIHTTPRequest                   `json:"source_requests"`
	SourceBodyReads     int                                       `json:"source_body_reads"`
	SourceBodyCloses    int                                       `json:"source_body_closes"`
	Providers           map[string]*reverseCLIProviderObservation `json:"providers"`
	CloseOrder          []string                                  `json:"close_order"`
	SearcherCloses      int                                       `json:"searcher_closes"`
	SDKConstructors     int                                       `json:"sdk_constructors"`
	SDKRequests         []reverseCLIHTTPRequest                   `json:"sdk_requests"`
	SDKAccounts         []pixivdeps.Request                       `json:"sdk_accounts"`
	SDKOptions          []map[string]any                          `json:"sdk_options"`
	SDKCloses           int                                       `json:"sdk_closes"`
	SDKBodyCloses       int                                       `json:"sdk_body_closes"`
	AccountsBefore      []reverseCLIAccountState                  `json:"accounts_before"`
	AccountsAfter       []reverseCLIAccountState                  `json:"accounts_after"`
	ConfigAfter         *string                                   `json:"config_after"`
	Database            bool                                      `json:"database"`
	SourceFileUnchanged bool                                      `json:"source_file_unchanged"`
	SnapshotFiles       []string                                  `json:"snapshot_files"`
	SnapshotReopenError string                                    `json:"snapshot_reopen_error"`
	PipelineRecords     []json.RawMessage                         `json:"pipeline_records,omitempty"`
	PipelineError       string                                    `json:"pipeline_error,omitempty"`
	PipelineStderr      string                                    `json:"pipeline_stderr,omitempty"`
}

type reverseCLICase struct {
	Name        string                `json:"name"`
	Input       reverseCLIInput       `json:"input"`
	Observation reverseCLIObservation `json:"observation"`
}

type reverseCLIFixture struct {
	Reference         string            `json:"reference"`
	Published         string            `json:"published_base"`
	Toolchain         string            `json:"toolchain"`
	Environment       string            `json:"environment"`
	FrozenGo          map[string]string `json:"frozen_go_production"`
	PublishedFixtures map[string]string `json:"published_fixtures"`
	Sources           map[string]string `json:"sources"`
	Limitations       []string          `json:"limitations"`
	Cases             []reverseCLICase  `json:"cases"`
}

type reverseCLIReader struct {
	reader      *strings.Reader
	observation *reverseCLIObservation
	failure     bool
}

func (r *reverseCLIReader) Read(p []byte) (int, error) {
	r.observation.StdinReads++
	if r.failure {
		return 0, errors.New("owned stdin failure")
	}
	n, err := r.reader.Read(p)
	r.observation.StdinBytes += n
	return n, err
}

type reverseCLIWriter struct {
	buffer    bytes.Buffer
	writes    *[]int
	mode      string
	remaining int
}

func (w *reverseCLIWriter) Write(p []byte) (int, error) {
	*w.writes = append(*w.writes, len(p))
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
	case "short-error":
		return n, io.ErrShortWrite
	default:
		return n, errors.New("owned writer failure")
	}
}

type reverseCLIPipeWriter struct {
	*reverseCLIWriter
	file *os.File
}

func (w reverseCLIPipeWriter) Fd() uintptr { return w.file.Fd() }

type reverseCLITransport func(*http.Request) (*http.Response, error)

func (f reverseCLITransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type reverseCLIBody struct {
	reader *strings.Reader
	reads  *int
	closes *int
	cancel context.CancelFunc
	mode   string
}

func (b *reverseCLIBody) Read(p []byte) (int, error) {
	if b.reads != nil {
		*b.reads++
	}
	if b.mode == "error" {
		return 0, errors.New("synthetic-upstream-body-secret")
	}
	n, err := b.reader.Read(p)
	if b.cancel != nil {
		b.cancel()
		b.cancel = nil
	}
	return n, err
}
func (b *reverseCLIBody) Close() error { *b.closes++; return nil }

type reverseCLIState struct {
	mu          sync.Mutex
	observation *reverseCLIObservation
	input       reverseCLIInput
	cancel      context.CancelFunc
	snapshot    *reversesearch.Snapshot
}

func (s *reverseCLIState) payload(provider string, snapshot *reversesearch.Snapshot) error {
	current := reverseCLIPayloadObservation{Kind: snapshot.Kind(), SHA256: snapshot.SHA256(), Size: snapshot.Size(), Reads: []string{}}
	for range 2 {
		r, err := snapshot.Open()
		if err != nil {
			return err
		}
		body, err := io.ReadAll(r)
		closeErr := r.Close()
		if err != nil || closeErr != nil {
			return errors.Join(err, closeErr)
		}
		current.Reads = append(current.Reads, hex.EncodeToString(body))
	}
	s.mu.Lock()
	s.snapshot = snapshot
	s.observation.Providers[provider].Payloads = append(s.observation.Providers[provider].Payloads, current)
	s.mu.Unlock()
	return nil
}

func (s *reverseCLIState) failure(key string) error {
	value := s.input.Failures[key]
	switch value {
	case "":
		return nil
	case "classified":
		return reversesearch.NewError(reversesearch.CodeSolverUnavailable, "ascii2d challenge solver is unavailable", errors.New("synthetic-api-key-secret synthetic-upstream-body-secret synthetic-csrf-secret synthetic-location-secret"))
	case "missing-key":
		return reversesearch.NewError(reversesearch.CodeMissingCredential, "SauceNAO API key is required", errors.New("synthetic-api-key-secret"))
	case "cancel":
		s.cancel()
		return context.Canceled
	default:
		return errors.New("synthetic-upstream-body-secret synthetic-csrf-secret synthetic-location-secret")
	}
}

type reverseCLISauce struct{ state *reverseCLIState }

func (p *reverseCLISauce) Preflight(context.Context) error {
	p.state.mu.Lock()
	p.state.observation.Providers["saucenao"].Preflights++
	p.state.mu.Unlock()
	return p.state.failure("saucenao-preflight")
}
func (p *reverseCLISauce) Search(_ context.Context, snapshot *reversesearch.Snapshot) (reversesearch.ProviderResponse, error) {
	p.state.mu.Lock()
	p.state.observation.Providers["saucenao"].Searches++
	p.state.mu.Unlock()
	if err := p.state.payload("saucenao", snapshot); err != nil {
		return reversesearch.ProviderResponse{}, err
	}
	return p.state.input.Replies[reversesearch.ProviderSauceNAO], p.state.failure("saucenao-search")
}
func (p *reverseCLISauce) Close() error {
	p.state.mu.Lock()
	p.state.observation.Providers["saucenao"].Closes++
	p.state.observation.CloseOrder = append(p.state.observation.CloseOrder, "saucenao")
	p.state.mu.Unlock()
	if p.state.input.Failures["saucenao-close"] != "" {
		return errors.New("owned SauceNAO close failure")
	}
	return nil
}

type reverseCLIASCII struct{ state *reverseCLIState }

func (p *reverseCLIASCII) Preflight(context.Context) error {
	p.state.mu.Lock()
	p.state.observation.Providers["ascii2d"].Preflights++
	p.state.mu.Unlock()
	return p.state.failure("ascii2d-preflight")
}
func (p *reverseCLIASCII) Upload(_ context.Context, snapshot *reversesearch.Snapshot) (reversesearch.ASCII2DSession, error) {
	p.state.mu.Lock()
	p.state.observation.Providers["ascii2d"].Uploads++
	p.state.mu.Unlock()
	if err := p.state.payload("ascii2d", snapshot); err != nil {
		return nil, err
	}
	if err := p.state.failure("ascii2d-upload"); err != nil {
		return nil, err
	}
	return p, nil
}
func (p *reverseCLIASCII) Search(_ context.Context, provider reversesearch.Provider) (reversesearch.ProviderResponse, error) {
	p.state.mu.Lock()
	p.state.observation.Providers[string(provider)].Searches++
	p.state.mu.Unlock()
	return p.state.input.Replies[provider], p.state.failure(string(provider) + "-search")
}
func (p *reverseCLIASCII) Close() error {
	p.state.mu.Lock()
	p.state.observation.Providers["ascii2d"].Closes++
	p.state.observation.CloseOrder = append(p.state.observation.CloseOrder, "ascii2d")
	p.state.mu.Unlock()
	if p.state.input.Failures["ascii2d-close"] != "" {
		return errors.New("owned ascii2d close failure")
	}
	return nil
}

type reverseCLISearcher struct {
	*reversesearch.Facade
	state *reverseCLIState
}

func (s *reverseCLISearcher) Search(ctx context.Context, request reversesearch.Request) (reversesearch.Response, error) {
	s.state.observation.Requests = append(s.state.observation.Requests, request)
	response, err := s.Facade.Search(ctx, request)
	s.state.observation.SearchResponses = append(s.state.observation.SearchResponses, response)
	message := ""
	if err != nil {
		message = err.Error()
	}
	s.state.observation.SearchErrors = append(s.state.observation.SearchErrors, message)
	return response, err
}
func (s *reverseCLISearcher) Close() error {
	s.state.observation.SearcherCloses++
	return s.Facade.Close()
}

func reverseCLIAccounts(t *testing.T, home string) []reverseCLIAccountState {
	t.Helper()
	rows := []reverseCLIAccountState{}
	if _, err := os.Stat(filepath.Join(home, ".pixiv-cli", "pixiv-cli.db")); os.IsNotExist(err) {
		return rows
	} else if err != nil {
		t.Fatal(err)
	}
	db, err := database.Open(filepath.Join(home, ".pixiv-cli"))
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
	}()
	accounts, err := db.ListPixiv(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	for _, account := range accounts {
		rows = append(rows, reverseCLIAccountState{account.UserID, account.CredentialRevision, string(account.RefreshTokenCopy()), account.PoolLastSelected, account.Schedulable})
	}
	return rows
}

func reverseCLIObserve(t *testing.T, input reverseCLIInput) reverseCLIObservation {
	t.Helper()
	home := t.TempDir()
	t.Chdir(home)
	for _, key := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "SAUCENAO_API_KEY", "PIXIV_ACCESS_TOKEN", "PIXIV_REFRESH_TOKEN"} {
		t.Setenv(key, "")
		if err := os.Unsetenv(key); err != nil {
			t.Fatal(err)
		}
	}
	for key, value := range map[string]string{"HOME": home, "USERPROFILE": home, "PIXIV_REQUEST_INTERVAL": "0", "TZ": "UTC", "XDG_CONFIG_HOME": filepath.Join(home, "xdg-config"), "XDG_CACHE_HOME": filepath.Join(home, "xdg-cache"), "TMPDIR": filepath.Join(home, "snapshots")} {
		t.Setenv(key, value)
	}
	for key, value := range input.Environment {
		t.Setenv(key, value)
	}
	configPath := filepath.Join(home, ".pixiv-cli", "config.toml")
	if input.Config != nil {
		if err := os.MkdirAll(filepath.Dir(configPath), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(configPath, []byte(*input.Config), 0600); err != nil {
			t.Fatal(err)
		}
	}
	for _, file := range []string{"image.png", "owned image.png", "ftp:owned.png"} {
		if err := os.WriteFile(file, []byte(reverseCLIPayload), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Mkdir("directory", 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir("snapshots", 0700); err != nil {
		t.Fatal(err)
	}
	if len(input.SavedAccounts) != 0 {
		db, err := database.Open(filepath.Dir(configPath))
		if err != nil {
			t.Fatal(err)
		}
		for _, id := range input.SavedAccounts {
			if err := db.SavePixivCredential(context.Background(), pixivaccount.New(id, "owned account", []byte(fmt.Sprintf("synthetic-refresh-secret-%d", id)))); err != nil {
				t.Fatal(err)
			}
		}
		if err := db.SetAllPixivSchedulable(context.Background(), true); err != nil {
			t.Fatal(err)
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
	}
	o := reverseCLIObservation{OutputWrites: []int{}, ErrorWrites: []int{}, Constructors: []reverseassembly.Options{}, Requests: []reversesearch.Request{}, SearchResponses: []reversesearch.Response{}, SearchErrors: []string{}, SourceRequests: []reverseCLIHTTPRequest{}, Providers: map[string]*reverseCLIProviderObservation{}, CloseOrder: []string{}, SDKRequests: []reverseCLIHTTPRequest{}, SDKAccounts: []pixivdeps.Request{}, SDKOptions: []map[string]any{}, SnapshotFiles: []string{}, AccountsBefore: reverseCLIAccounts(t, home)}
	for _, name := range []string{"saucenao", "ascii2d", "ascii2d-color", "ascii2d-bovw"} {
		o.Providers[name] = &reverseCLIProviderObservation{Payloads: []reverseCLIPayloadObservation{}}
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if input.Cancel == "before" {
		cancel()
	}
	state := &reverseCLIState{observation: &o, input: input, cancel: cancel}
	oldReverse, oldSDK := newCLIReverseSearch, newCLIPixivSDKPorts
	oldCleanup, oldSupported, oldAutomatic := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, newCLIAutomaticUpdateChecker
	t.Cleanup(func() {
		newCLIReverseSearch, newCLIPixivSDKPorts = oldReverse, oldSDK
		cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, newCLIAutomaticUpdateChecker = oldCleanup, oldSupported, oldAutomatic
	})
	cleanupPendingWindowsUpdate = func() error {
		o.StartupCalls++
		if input.StartupFailure {
			return errors.New("owned startup failure")
		}
		return nil
	}
	automaticPersistentHandlerSupported = func() bool { o.HandlerChecks++; return false }
	newCLIAutomaticUpdateChecker = func(string) (*update.AutomaticUpdateChecker, error) {
		o.AutomaticChecks++
		return nil, errors.New("owned automatic-check boundary")
	}
	newCLIReverseSearch = func(options reverseassembly.Options) (reversesearch.Searcher, error) {
		o.Constructors = append(o.Constructors, options)
		if input.Failures["constructor"] != "" {
			return nil, reversesearch.NewError(reversesearch.CodeProviderNotConfigured, "reverse search HTTP client is not configured", errors.New("synthetic-api-key-secret"))
		}
		transport := reverseCLITransport(func(request *http.Request) (*http.Response, error) {
			o.SourceRequests = append(o.SourceRequests, reverseCLIHTTPRequest{Method: request.Method, URL: request.URL.String(), Header: request.Header.Clone()})
			if request.Method != http.MethodGet || request.URL.Hostname() != "owned-source.invalid" {
				return nil, errors.New("owned source transport rejected an unexpected destination")
			}
			if input.Failures["source-transport"] != "" {
				return nil, errors.New("synthetic-source-secret synthetic-upstream-body-secret")
			}
			status := input.SourceStatus
			if status == 0 {
				status = 200
			}
			body := &reverseCLIBody{reader: strings.NewReader(reverseCLIPayload), reads: &o.SourceBodyReads, closes: &o.SourceBodyCloses}
			if input.Cancel == "source" {
				body.cancel = cancel
			}
			if input.Failures["source-read"] != "" {
				body.mode = "error"
			}
			return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"image/png"}}, Body: body, Request: request}, nil
		})
		facade := reversesearch.NewFacade(reversesearch.Dependencies{Sources: reversesearch.NewSourceLoader(reversesearch.SourceLoaderOptions{TempDir: filepath.Join(home, "snapshots"), HTTPClient: &http.Client{Transport: transport}}), Payloads: reversesearch.NewAggregator(reversesearch.AggregatorDependencies{SauceNAO: &reverseCLISauce{state}, ASCII2D: &reverseCLIASCII{state}})})
		return &reverseCLISearcher{facade, state}, nil
	}
	newCLIPixivSDKPorts = func(a app) (pixivSDKPorts, error) {
		o.SDKConstructors++
		if len(input.SavedAccounts) == 0 {
			return oldSDK(a)
		}
		db, err := a.openAuthDatabase()
		if err != nil {
			return pixivSDKPorts{}, err
		}
		transport := reverseCLITransport(func(request *http.Request) (*http.Response, error) {
			observed := reverseCLIHTTPRequest{Method: request.Method, URL: request.URL.String(), Header: request.Header.Clone()}
			payload := ""
			switch request.URL.Host {
			case "oauth.secure.pixiv.net":
				if request.Method != "POST" || request.URL.Path != "/auth/token" {
					return nil, errors.New("owned SDK OAuth destination mismatch")
				}
				if err := request.ParseForm(); err != nil {
					return nil, err
				}
				observed.Form = request.Form
				id, err := strconv.ParseInt(strings.TrimPrefix(request.Form.Get("refresh_token"), "synthetic-refresh-secret-"), 10, 64)
				if err != nil {
					return nil, err
				}
				payload = fmt.Sprintf(`{"access_token":"synthetic-access-secret-%d","refresh_token":"synthetic-rotated-secret-%d","expires_in":3600,"user":{"id":%d,"name":"owned account"}}`, id, id, id)
			case "app-api.pixiv.net":
				if request.Method != "GET" || request.URL.Path != "/v1/search/illust" {
					return nil, errors.New("owned SDK search destination mismatch")
				}
				payload = input.SDKSearchBody
			default:
				return nil, errors.New("owned SDK transport rejected an unexpected destination")
			}
			o.SDKRequests = append(o.SDKRequests, observed)
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: &reverseCLIBody{reader: strings.NewReader(payload), closes: &o.SDKBodyCloses}, Request: request}, nil
		})
		facade := pixivapp.New(pixivapp.Dependencies{Accounts: pixivaccount.NewService(db, settings.DefaultStore()), Gate: pixivpool.NewGate(), LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
			runtime, err := a.runtimeConfig()
			return pixivapp.PoolConfig{Enabled: runtime.AccountPool.Enabled, Strategy: string(runtime.AccountPool.Strategy)}, err
		}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
			return pixivpool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
		}, CloseClient: func(client *pixiv.Client) error { o.SDKCloses++; client.CloseIdleConnections(); return nil }})
		return pixivSDKPorts{execute: func(ctx context.Context, request pixivdeps.Request, callback func(context.Context, *pixiv.Client) (bool, error)) error {
			o.SDKAccounts = append(o.SDKAccounts, request)
			options, err := pixivOptionsFromRequest(request, a.runtimeConfig)
			if err != nil {
				return err
			}
			o.SDKOptions = append(o.SDKOptions, map[string]any{"min_interval": options.Pacing.MinInterval.String(), "http_client_present": options.HTTPClient != nil})
			options.HTTPClient = &http.Client{Transport: transport}
			return facade.Use(ctx, pixivapp.Request{UserID: request.UserID, Options: options}, callback)
		}}, nil
	}
	reader := &reverseCLIReader{reader: strings.NewReader(input.Stdin), observation: &o, failure: input.StdinFailure}
	out := &reverseCLIWriter{writes: &o.OutputWrites, mode: input.Writer, remaining: input.WriteLimit}
	errOut := &reverseCLIWriter{writes: &o.ErrorWrites, mode: input.WarningWriter}
	var writer io.Writer = out
	if input.OutputPipe {
		file, peer, err := os.Pipe()
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		defer peer.Close()
		writer = reverseCLIPipeWriter{out, peer}
	}
	o.Exit = RunContext(ctx, append([]string{"pixiv"}, input.Args...), reader, writer, errOut)
	o.Stdout, o.Stderr = out.buffer.String(), errOut.buffer.String()
	if input.ConsumeRecords {
		var diagnostics bytes.Buffer
		err := pipeline.ConsumeNDJSONRecords(context.Background(), strings.NewReader(o.Stdout), &diagnostics, "owned-read-only-consumer", false, func(_ context.Context, value record.Record) error {
			body, err := value.MarshalJSON()
			if err != nil {
				return err
			}
			o.PipelineRecords = append(o.PipelineRecords, body)
			return nil
		})
		if err != nil {
			o.PipelineError = err.Error()
		}
		o.PipelineStderr = diagnostics.String()
	}
	if state.snapshot != nil {
		r, err := state.snapshot.Open()
		if r != nil {
			r.Close()
		}
		if err != nil {
			o.SnapshotReopenError = err.Error()
		}
	}
	files, err := os.ReadDir(filepath.Join(home, "snapshots"))
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range files {
		o.SnapshotFiles = append(o.SnapshotFiles, file.Name())
	}
	config, err := os.ReadFile(configPath)
	if err == nil {
		value := string(config)
		o.ConfigAfter = &value
	} else if !os.IsNotExist(err) {
		t.Fatal(err)
	}
	_, err = os.Stat(filepath.Join(home, ".pixiv-cli", "pixiv-cli.db"))
	o.Database = err == nil
	if err != nil && !os.IsNotExist(err) {
		t.Fatal(err)
	}
	o.AccountsAfter = reverseCLIAccounts(t, home)
	o.SourceFileUnchanged = true
	for _, file := range []string{"image.png", "owned image.png", "ftp:owned.png"} {
		source, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		o.SourceFileUnchanged = o.SourceFileUnchanged && bytes.Equal(source, []byte(reverseCLIPayload))
	}
	for _, secret := range []string{"synthetic-source-secret", "synthetic-api-key-secret", "synthetic-upstream-body-secret", "synthetic-csrf-secret", "synthetic-location-secret", "synthetic-refresh-secret", "synthetic-access-secret", "synthetic-rotated-secret"} {
		if strings.Contains(o.Stdout+o.Stderr, secret) {
			t.Fatalf("CLI output leaked %s", secret)
		}
	}
	if len(o.SnapshotFiles) != 0 {
		t.Fatal("connected CLI retained source snapshots")
	}
	if o.SearcherCloses != len(o.Requests) {
		t.Fatalf("searcher closure ownership differs: closes=%d requests=%d", o.SearcherCloses, len(o.Requests))
	}
	if len(o.Requests) != 0 && o.SDKConstructors != 0 {
		t.Fatal("image route constructed Pixiv SDK/account resources")
	}
	return o
}

func reverseCLIRows() []reverseCLICase {
	rows := []reverseCLICase{}
	text := func(value string) *string { return &value }
	add := func(name string, args ...string) *reverseCLICase {
		replies := map[reversesearch.Provider]reversesearch.ProviderResponse{}
		for _, provider := range []reversesearch.Provider{reversesearch.ProviderSauceNAO, reversesearch.ProviderASCII2DColor, reversesearch.ProviderASCII2DBOVW} {
			replies[provider] = reversesearch.ProviderResponse{Provider: provider, Matches: []reversesearch.Match{
				{Rank: 3, Similarity: 82.75, IndexID: 5, IndexName: "owned external index", Title: "external\tresult", Author: "other", ExternalURLs: []string{"https://external.invalid/owned"}},
				{Rank: 1, Similarity: 97.125, IndexID: 5, IndexName: "owned Pixiv index", Title: "owned\nartwork", Author: "artist\rname", ArtworkID: 42, ExternalURLs: []string{"https://www.pixiv.net/artworks/42"}},
				{Rank: 2, Similarity: 91, IndexID: 5, IndexName: "owned user index", Title: "owned user", Author: "user", UserID: 7, ExternalURLs: []string{"https://www.pixiv.net/users/7"}},
			}, Quota: &reversesearch.Quota{ShortRemaining: 7, LongRemaining: 80, ShortLimit: 10, LongLimit: 100}}
		}
		rows = append(rows, reverseCLICase{Name: name, Input: reverseCLIInput{Args: args, Environment: map[string]string{}, SavedAccounts: []int64{}, Replies: replies, Failures: map[string]string{}}})
		return &rows[len(rows)-1]
	}
	for _, mode := range []string{"human", "json", "ndjson", "auto", "json-false-pipe", "ndjson-false-pipe"} {
		args := []string{"search", reverseCLISource}
		switch mode {
		case "json", "ndjson":
			args = append(args, "--"+mode)
		case "json-false-pipe":
			args = append(args, "--json=false")
		case "ndjson-false-pipe":
			args = append(args, "--ndjson=false")
		}
		row := add("default-provider/"+mode, args...)
		row.Input.OutputPipe = strings.Contains(mode, "pipe") || mode == "auto"
	}
	for _, source := range []string{"image.png", "owned image.png", "ftp:owned.png", "HTTP://owned-source.invalid/image.png", "https:", "https://", "https://user:synthetic-source-secret@owned-source.invalid/image.png"} {
		add("source/"+source, "search", source, "--json")
	}
	row := add("source/joined-arguments", "search", "owned", "image.png", "--json")
	row.Input.Stdin = "must not be read\n"
	for _, stdin := range []string{reverseCLISource + "\n", "image.png\n", " image.png \r\n", "image.png\nimage.png\n", "", "{\"id\":\"42\",\"type\":\"artwork\",\"url\":\"https://www.pixiv.net/artworks/42\"}\n"} {
		add("stdin/"+strconv.Quote(stdin), "search", "--json").Input.Stdin = stdin
	}
	add("stdin/explicit-dash", "search", "-", "--json").Input.Stdin = reverseCLISource + "\n"
	add("stdin/read-failure", "search", "--json").Input.StdinFailure = true
	for _, provider := range []string{"saucenao", "ascii2d-color", "ascii2d-bovw", "all", "", "unknown", "SauceNAO"} {
		add("provider/"+provider, "search", reverseCLISource, "--provider", provider, "--json")
	}
	for _, mode := range []string{"human", "json", "ndjson"} {
		args := []string{"search", reverseCLISource, "--provider=all"}
		if mode != "human" {
			args = append(args, "--"+mode)
		}
		row := add("all/external/"+mode, args...)
		row.Input.Config = text("[reverse_search]\npixiv_only = false\n")
		row = add("partial/"+mode, args...)
		row.Input.Failures["ascii2d-color-search"] = "classified"
		row = add("all-failed/"+mode, args...)
		row.Input.Failures["saucenao-search"] = "raw"
		row.Input.Failures["ascii2d-preflight"] = "classified"
		row = add("single-failed/"+mode, append([]string{"search", reverseCLISource, "--provider=ascii2d-color"}, args[3:]...)...)
		row.Input.Failures["ascii2d-color-search"] = "classified"
		row = add("empty/"+mode, args...)
		for provider, reply := range row.Input.Replies {
			reply.Matches = nil
			reply.Quota = nil
			row.Input.Replies[provider] = reply
		}
	}
	for _, failure := range []string{"constructor", "source-transport", "source-read", "saucenao-preflight"} {
		row := add("failure/"+failure, "search", reverseCLISource, "--json")
		row.Input.Failures[failure] = "raw"
		if failure == "saucenao-preflight" {
			row.Input.Failures[failure] = "missing-key"
		}
	}
	add("source/status", "search", reverseCLISource, "--json").Input.SourceStatus = 403
	for _, stage := range []string{"before", "source", "saucenao-search", "ascii2d-color-search"} {
		row := add("cancel/"+stage, "search", reverseCLISource, "--json")
		if stage == "before" || stage == "source" {
			row.Input.Cancel = stage
		} else {
			row.Input.Args = append(row.Input.Args, "--provider=all")
			row.Input.Failures[stage] = "cancel"
		}
	}
	for _, name := range []string{"search-by", "sort", "period", "start-date", "end-date", "rating", "type", "content-type", "resolution", "aspect-ratio", "draw-tool", "ai-mode", "bookmark-min", "bookmark-max", "bookmark-strategy", "limit", "page"} {
		value := "invalid"
		if strings.HasPrefix(name, "bookmark-m") || name == "limit" || name == "page" {
			value = "0"
		}
		add("unsupported-flag/"+name, "search", reverseCLISource, "--"+name+"="+value, "--json")
	}
	for _, flags := range [][]string{{"--json", "--ndjson"}, {"--json=false", "--ndjson=false"}, {"--trending-tags"}, {"--unknown"}, {"--proxy=", "--no-proxy=false"}} {
		add("flag-conflict/"+strings.Join(flags, "/"), append([]string{"search", reverseCLISource}, flags...)...)
	}
	for _, value := range []string{"http://global-proxy.invalid:8080", ""} {
		for _, flags := range [][]string{{}, {"--proxy=http://flag-proxy.invalid:8080"}, {"--proxy="}, {"--no-proxy"}, {"--no-proxy=false"}} {
			row := add("proxy/"+value+"/"+strings.Join(flags, "/"), append([]string{"search", reverseCLISource, "--json"}, flags...)...)
			row.Input.Config = text("[network]\nhttps_proxy = 'http://global-proxy.invalid:8080'\n[reverse_search]\nprovider = 'all'\npixiv_only = false\nsaucenao_api_key = 'synthetic-api-key-secret'\n[reverse_search.network]\nproxy_url = '" + value + "'\nuser_agent = 'owned-browser-agent'\n[reverse_search.flaresolverr]\nurl = 'http://owned-solver.invalid'\nproxy_url = 'socks5://owned-solver-proxy.invalid:1080'\n")
		}
	}
	for _, config := range []string{"[network]\nhttps_proxy = 'http://global-proxy.invalid:8080'\n", "[network]\nhttps_proxy = 'http://global-proxy.invalid:8080'\n[reverse_search.network]\nproxy_url = 'http://service-proxy.invalid:8081'\n"} {
		add("proxy/config-fallback/"+strconv.Quote(config), "search", reverseCLISource, "--json").Input.Config = text(config)
	}
	for _, flags := range [][]string{{}, {"--no-proxy"}, {"--no-proxy=false"}, {"--proxy="}} {
		row := add("proxy/environment/"+strings.Join(flags, "/"), append([]string{"search", reverseCLISource, "--json"}, flags...)...)
		row.Input.Config = text("[network]\nhttps_proxy = 'http://file-proxy.invalid:8080'\n[reverse_search]\nsaucenao_api_key = 'synthetic-file-key'\n")
		row.Input.Environment = map[string]string{"https_proxy": "http://lower-proxy.invalid:8081", "HTTPS_PROXY": "http://upper-proxy.invalid:8082", "SAUCENAO_API_KEY": "synthetic-api-key-secret"}
	}
	for _, flags := range [][]string{{"--json=false"}, {"--ndjson"}} {
		add("output/config-override/"+strings.Join(flags, "/"), append([]string{"search", reverseCLISource}, flags...)...).Input.Config = text("[output]\njson = true\n")
	}
	for _, config := range []string{"[unfinished\n", "[reverse_search]\nprovider = 'unknown'\n", "[reverse_search]\npixiv_only = 'true'\n", "[reverse_search.network]\nproxy_url = 42\n", "[pixiv.auth]\ndefault_user_id = 0\n", "[output]\njson = true\n", "# preserved\r\n[unknown]\r\nkeep = true\r\n"} {
		add("config/"+strconv.Quote(config), "search", reverseCLISource).Input.Config = text(config)
	}
	for _, mode := range []string{"human", "json", "ndjson", "auto"} {
		for _, failure := range []string{"other", "pipe", "short", "short-error"} {
			args := []string{"search", reverseCLISource}
			if mode == "json" || mode == "ndjson" {
				args = append(args, "--"+mode)
			}
			row := add("writer/"+mode+"/"+failure, args...)
			row.Input.Writer = failure
			row.Input.WriteLimit = 9
			row.Input.OutputPipe = mode == "auto"
		}
		for _, failure := range []string{"other", "pipe"} {
			args := []string{"search", reverseCLISource, "--provider=all"}
			if mode == "json" || mode == "ndjson" {
				args = append(args, "--"+mode)
			}
			row := add("warning-writer/"+mode+"/"+failure, args...)
			row.Input.WarningWriter = failure
			row.Input.Failures["ascii2d-color-search"] = "classified"
			row.Input.OutputPipe = mode == "auto"
		}
		args := []string{"search", reverseCLISource}
		if mode == "json" || mode == "ndjson" {
			args = append(args, "--"+mode)
		}
		row := add("close-failure/"+mode, args...)
		row.Input.Failures["ascii2d-close"] = "error"
		row.Input.Failures["saucenao-close"] = "error"
		row.Input.OutputPipe = mode == "auto"
	}
	row = add("failure/output-precedes-provider", "search", reverseCLISource, "--json")
	row.Input.Failures["saucenao-search"] = "raw"
	row.Input.Writer = "other"
	add("startup/failure", "search", reverseCLISource, "--json").Input.StartupFailure = true
	for _, word := range []string{"keyword", "missing.png", "directory", "ftp://owned-source.invalid/image.png"} {
		add("keyword/no-account/"+word, "search", word, "--json")
		add("keyword/provider-rejected/"+word, "search", word, "--provider=all", "--json")
	}
	for _, config := range []string{"", "[pixiv.auth]\ndefault_user_id = 43\n", "[pixiv.auth]\ndefault_user_id = 99\n", "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n"} {
		row := add("keyword/saved-account/"+strconv.Quote(config), "search", "owned keyword", "--json")
		row.Input.Config = text(config)
		row.Input.SavedAccounts = []int64{42, 43}
		row.Input.SDKSearchBody = `{"illusts":[],"next_url":null}`
		row = add("image/saved-account-bypass/"+strconv.Quote(config), "search", reverseCLISource, "--json")
		row.Input.Config = text(config)
		row.Input.SavedAccounts = []int64{42, 43}
	}
	for _, mode := range []string{"ndjson", "auto"} {
		args := []string{"search", reverseCLISource, "--provider=all"}
		if mode == "ndjson" {
			args = append(args, "--ndjson")
		}
		row := add("pipeline/"+mode, args...)
		row.Input.Config = text("[reverse_search]\npixiv_only = false\n")
		row.Input.OutputPipe = mode == "auto"
		row.Input.ConsumeRecords = true
	}
	return rows
}

func reverseCLIAssertBoundary(t *testing.T, row reverseCLICase) {
	t.Helper()
	o := row.Observation
	prefix := strings.SplitN(row.Name, "/", 2)[0]
	stopBeforeConstruction := func(exit, startup int, message string) {
		if o.Exit != exit || o.StartupCalls != startup || len(o.Constructors) != 0 || len(o.Requests) != 0 || len(o.SourceRequests) != 0 || o.SDKConstructors != 0 || o.Stdout != "" || !strings.Contains(o.Stderr, message) {
			t.Fatalf("validation did not stop at its intended boundary: exit=%d startup=%d constructors=%d requests=%d sources=%d sdk=%d stderr=%s", o.Exit, o.StartupCalls, len(o.Constructors), len(o.Requests), len(o.SourceRequests), o.SDKConstructors, o.Stderr)
		}
	}
	connected := func() {
		if len(o.Constructors) != 1 || len(o.Requests) != 1 || len(o.SearchResponses) != 1 || o.SearcherCloses != 1 || o.SDKConstructors != 0 || o.Providers["saucenao"].Closes != 1 || o.Providers["ascii2d"].Closes != 1 {
			t.Fatalf("case did not complete its owned reverse-search lifecycle: %+v", o)
		}
	}
	loaded := func() {
		connected()
		if len(o.SourceRequests) != 1 || (o.Providers["saucenao"].Searches == 0 && o.Providers["ascii2d"].Uploads == 0) || o.SourceBodyCloses != 1 || o.SnapshotReopenError != "image snapshot is closed" {
			t.Fatalf("case did not reach source/provider work and clean its snapshot: %+v", o)
		}
	}
	success := false
	switch prefix {
	case "unsupported-flag":
		stopBeforeConstruction(2, 1, "--"+strings.TrimPrefix(row.Name, "unsupported-flag/")+" is not supported for image sources")
	case "flag-conflict":
		switch {
		case strings.Contains(row.Name, "--unknown"):
			stopBeforeConstruction(2, 0, "unknown option '--unknown'")
		case strings.Contains(row.Name, "--trending-tags"):
			stopBeforeConstruction(1, 0, "usage: pixiv search --trending-tags")
		case strings.Contains(row.Name, "--proxy="):
			stopBeforeConstruction(1, 1, "use either --proxy or --no-proxy, not both")
		default:
			stopBeforeConstruction(2, 1, "--ndjson cannot be used with --json")
		}
	case "provider":
		if row.Name == "provider/unknown" || row.Name == "provider/SauceNAO" {
			stopBeforeConstruction(2, 1, "provider must be one of")
		} else {
			loaded()
			success = true
		}
	case "config":
		switch *row.Input.Config {
		case "[unfinished\n":
			stopBeforeConstruction(1, 1, "toml:")
		case "[reverse_search]\nprovider = 'unknown'\n":
			stopBeforeConstruction(1, 1, "reverse_search_provider must be one of")
		case "[reverse_search.network]\nproxy_url = 42\n":
			stopBeforeConstruction(1, 1, "reverse_search.network.proxy_url must be a string")
		default:
			loaded()
			success = true
		}
	case "startup":
		stopBeforeConstruction(1, 1, "clean pending update: owned startup failure")
	case "keyword":
		if strings.Contains(row.Name, "/provider-rejected/") {
			stopBeforeConstruction(2, 1, "--provider is only supported for image sources")
			break
		}
		if len(o.Constructors) != 0 || o.SDKConstructors != 1 || len(o.SourceRequests) != 0 || !o.Database {
			t.Fatalf("keyword route did not exclusively acquire its account SDK: %+v", o)
		}
		if strings.Contains(row.Name, "/no-account/") {
			if o.Exit != 1 || len(o.SDKRequests) != 0 || !strings.Contains(o.Stderr, "no pixiv account is authenticated") {
				t.Fatalf("no-account case failed outside account selection: %+v", o)
			}
			break
		}
		if strings.Contains(*row.Input.Config, "default_user_id = 99") {
			if o.Exit != 1 || len(o.SDKRequests) != 0 || !strings.Contains(o.Stderr, "configured default pixiv account 99 is missing") {
				t.Fatalf("missing default failed outside account selection: %+v", o)
			}
			break
		}
		if o.Exit != 0 || len(o.SDKRequests) != 2 || o.SDKCloses != 1 || o.SDKBodyCloses != 2 || o.Stdout == "" {
			t.Fatalf("saved keyword route did not complete OAuth/search/output/close: %+v", o)
		}
	case "source":
		connected()
		if row.Name == "source/https:" || row.Name == "source/https://" || strings.Contains(row.Name, "user:synthetic-source-secret@") {
			if o.Exit != 1 || len(o.SourceRequests) != 0 || o.Providers["saucenao"].Searches != 0 || !strings.Contains(o.Stderr, "image source URL") {
				t.Fatalf("invalid source was not rejected before fetching: %+v", o)
			}
			break
		}
		if row.Name == "source/status" {
			if o.Exit != 1 || len(o.SourceRequests) != 1 || o.SourceBodyReads != 0 || o.SourceBodyCloses != 1 || o.Providers["saucenao"].Searches != 0 || !strings.Contains(o.Stderr, "unsuccessful HTTP status") {
				t.Fatalf("source status failure did not stop before provider access: %+v", o)
			}
			break
		}
		if o.Providers["saucenao"].Searches != 1 || o.SnapshotReopenError != "image snapshot is closed" {
			t.Fatalf("valid source did not reach its provider or close snapshot: %+v", o)
		}
		success = true
	case "stdin":
		switch {
		case row.Input.StdinFailure:
			stopBeforeConstruction(2, 0, "read stdin value: owned stdin failure")
		case row.Input.Stdin == "":
			stopBeforeConstruction(1, 0, "usage: pixiv search [options] WORD")
		case row.Input.Stdin == reverseCLISource+"\n" && row.Name != "stdin/explicit-dash":
			loaded()
			success = true
		case row.Input.Stdin == "image.png\n":
			connected()
			if o.Providers["saucenao"].Searches != 1 {
				t.Fatal("stdin file did not reach provider")
			}
			success = true
		default:
			if o.SDKConstructors != 1 || len(o.Requests) != 0 || o.Exit != 1 || !strings.Contains(o.Stderr, "no pixiv account is authenticated") {
				t.Fatalf("literal stdin keyword did not reach account selection: %+v", o)
			}
		}
	case "failure":
		switch row.Name {
		case "failure/constructor":
			if o.Exit != 1 || len(o.Constructors) != 1 || len(o.Requests) != 0 || o.SearcherCloses != 0 || len(o.SourceRequests) != 0 || !strings.Contains(o.Stderr, "reverse search HTTP client is not configured") {
				t.Fatalf("constructor failure acquired later resources: %+v", o)
			}
		case "failure/saucenao-preflight":
			connected()
			if o.Exit != 1 || len(o.SourceRequests) != 0 || o.Providers["saucenao"].Preflights != 1 || o.Providers["saucenao"].Searches != 0 || !strings.Contains(o.Stderr, "SauceNAO API key is required") {
				t.Fatalf("preflight did not reject before reading source: %+v", o)
			}
		case "failure/source-transport", "failure/source-read":
			connected()
			if o.Exit != 1 || len(o.SourceRequests) != 1 || o.Providers["saucenao"].Searches != 0 || !strings.Contains(o.Stderr, "image source") {
				t.Fatalf("source failure escaped its boundary: %+v", o)
			}
		default:
			loaded()
			if o.Exit != 1 || !strings.Contains(o.Stderr, "owned writer failure") || strings.Contains(o.Stderr, "reverse search provider failed") {
				t.Fatalf("output failure did not take precedence: %+v", o)
			}
		}
	case "cancel":
		connected()
		if o.Exit != 1 || !strings.Contains(o.Stderr, "context canceled") {
			t.Fatalf("cancellation did not remain overall failure: %+v", o)
		}
		if row.Input.Cancel == "before" && len(o.SourceRequests) != 0 {
			t.Fatal("pre-cancellation read source")
		}
		if row.Input.Cancel == "source" && (len(o.SourceRequests) != 1 || o.Providers["saucenao"].Searches != 0) {
			t.Fatal("source cancellation started provider")
		}
		if row.Input.Cancel == "" {
			loaded()
		}
	case "all-failed", "single-failed":
		loaded()
		if o.Exit != 1 || o.SearchErrors[0] == "" || len(o.SearchResponses[0].ProviderErrors) == 0 {
			t.Fatalf("provider failure did not return its safe domain response/error: %+v", o)
		}
	case "empty":
		loaded()
		if o.Exit != 0 || len(o.SearchResponses[0].Results) != 0 {
			t.Fatalf("empty provider response did not succeed: %+v", o)
		}
	case "writer", "warning-writer", "close-failure":
		loaded()
		if len(o.OutputWrites) == 0 {
			t.Fatal("writer/close failure did not reach actual output")
		}
	default:
		loaded()
		success = true
	}
	if success && (o.Exit != 0 || o.Stdout == "" || len(o.SearchResponses[0].Results) < 2 || o.SearchErrors[0] != "") {
		t.Fatalf("successful root route did not emit its genuine results: %+v", o)
	}
	if prefix == "pipeline" && (len(o.PipelineRecords) != 2 || o.PipelineError != "" || o.PipelineStderr != "") {
		t.Fatalf("actual NDJSON record pipeline did not consume both identities: %+v", o)
	}
}

func reverseCLISourceManifest(t *testing.T, repo string) (map[string]string, map[string]string, map[string]string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	frozen, published, sources := map[string]string{}, map[string]string{}, map[string]string{}
	for _, item := range []struct {
		revision    string
		destination map[string]string
		production  bool
	}{{reverseCLIReference, frozen, true}, {reverseCLIPublished, published, false}} {
		listing, err := exec.CommandContext(ctx, "git", "-C", repo, "ls-tree", "-r", "--name-only", item.revision).Output()
		if err != nil {
			t.Fatal(err)
		}
		for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
			eligible := strings.HasPrefix(path, "crates/") && strings.Contains(path, "/fixtures/")
			if item.production {
				eligible = (strings.HasSuffix(path, ".go") && !strings.HasSuffix(path, "_test.go")) || path == "go.mod" || path == "go.sum"
			}
			if !eligible {
				continue
			}
			original, err := exec.CommandContext(ctx, "git", "-C", repo, "show", item.revision+":"+path).Output()
			if err != nil {
				t.Fatal(err)
			}
			actual, err := os.ReadFile(filepath.Join(repo, path))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(actual, original) {
				t.Fatalf("protected reference changed: %s", path)
			}
			item.destination[path] = fmt.Sprintf("%x", sha256.Sum256(actual))
			if item.production && (strings.HasPrefix(path, "internal/cli/") || strings.HasPrefix(path, "internal/services/reversesearch/") || strings.HasPrefix(path, "internal/shared/record/") || strings.HasPrefix(path, "internal/config/") || strings.HasPrefix(path, "internal/services/pixiv/")) {
				sources[path] = item.destination[path]
			}
		}
	}
	if len(frozen) != 434 || len(published) != 104 {
		t.Fatalf("protected inventory differs: Go=%d fixtures=%d", len(frozen), len(published))
	}
	return frozen, published, sources
}

func TestMigrationReverseSearchCLIPreservesConnectedRootContracts(t *testing.T) {
	repo, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	frozen, published, sources := reverseCLISourceManifest(t, repo)
	fixture := reverseCLIFixture{Reference: reverseCLIReference, Published: reverseCLIPublished, Toolchain: runtime.Version(), Environment: "linux/amd64; TZ=UTC; owned home/config/SQLite/source files; constructor overrides and in-memory SDK/source HTTP endpoints", FrozenGo: frozen, PublishedFixtures: published, Sources: sources, Cases: reverseCLIRows(), Limitations: []string{
		"Each observation is captured through the actual RunContext root, startup/configuration, source routing, command owner, presentation, error envelope, and resource closure. No custom presenter or expectation projection is used.",
		"The original newCLIReverseSearch constructor port is replaced with the genuine SourceLoader, Facade, and Aggregator plus synthetic terminal provider ports. This proves connected CLI ownership and projection, not provider HTTP upload/protocol or native browser equivalence.",
		"Native ascii2d requires its dedicated Chrome 146 browser transport. Ordinary net/http source/SDK mocks do not satisfy that requirement; native transport, TLS fingerprints, browser challenges, and third-party uploads are not exercised.",
		"Only owned synthetic payloads and account credentials are used. Source and SDK transports reject unexpected destinations and never open a network socket; no user credentials, external copyrighted media, browser sessions, or host registration/settings are accessed.",
		"Keyword no-account cases retain the original SDK constructor. Saved-account keyword cases inject the existing SDK constructor port with the genuine account service, SQLite repository, Facade, Gate, Scheduler, config option resolver, public OAuth/search SDK, and an in-memory HTTP endpoint.",
		"Output pipe selection uses a real owned pipe file descriptor with a capturing writer. Pipe failures are injected EPIPE and cover the actual root exit decision; kernel SIGPIPE delivery and shell child process behavior are not exercised.",
		"Relative owned source filenames avoid path normalization. Configuration and credential environment aliases are absent unless explicitly provided by an input; PIXIV_REQUEST_INTERVAL=0 avoids pacing delays. Entire stdout/stderr, domain responses, constructor values, request fields, provider payload reads, account changes, and close state are retained byte-for-byte. Concurrent provider scheduling order is not asserted; per-provider counts and fixed provider output order are retained.",
		"Protected source inventory is exactly the 434 production Go/module paths at the frozen revision and the 104 published crate fixtures at the published base. Test additions are separate from production sources. No Rust changes, Cargo execution, commits, or pushes are performed by this capture.",
		"The denied supplemental multiplex/upload probe is not retried and is not represented as completed evidence.",
	}}
	for index := range fixture.Cases {
		row := &fixture.Cases[index]
		t.Run(row.Name, func(t *testing.T) {
			row.Observation = reverseCLIObserve(t, row.Input)
			reverseCLIAssertBoundary(t, *row)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(repo, "crates", "pixiv-cli", "tests", "fixtures", "reverse-search-cli.json")
	if *captureReverseSearchCLI {
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
		if err := os.WriteFile("/tmp/reverse-cli-replay-actual.json", data, 0600); err != nil {
			t.Fatal(err)
		}
		t.Fatal("connected reverse-search CLI differs from frozen Go reference")
	}
}
