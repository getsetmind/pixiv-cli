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
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	ugoiracmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/ugoira"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateUgoiraWorkflow = flag.Bool("migration-update-ugoira-workflow", false, "capture frozen saved ugoira workflow")

const ugoiraWorkflowReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type ugoiraWorkflowInput struct {
	Name          string          `json:"name"`
	Mode          string          `json:"mode"`
	Selection     string          `json:"selection"`
	Scenario      string          `json:"scenario"`
	Kind          string          `json:"kind"`
	MetadataIndex int             `json:"metadata_index"`
	Config        string          `json:"config"`
	Flags         []string        `json:"flags"`
	DetailBody    json.RawMessage `json:"detail_body"`
	MetadataBody  json.RawMessage `json:"metadata_body"`
}
type ugoiraWorkflowRequest struct {
	Method    string   `json:"method"`
	Host      string   `json:"host"`
	URI       string   `json:"uri"`
	Account   int64    `json:"account"`
	Revision  int64    `json:"revision"`
	Persisted bool     `json:"persisted"`
	GrantType string   `json:"grant_type,omitempty"`
	FormKeys  []string `json:"form_keys,omitempty"`
}
type ugoiraWorkflowState struct {
	ID       int64 `json:"id"`
	Revision int64 `json:"revision"`
	Rotated  bool  `json:"rotated"`
	Frozen   bool  `json:"frozen"`
	Selected bool  `json:"selected"`
}
type ugoiraWorkflowError struct {
	Message       string        `json:"message"`
	Product       string        `json:"product"`
	Operation     string        `json:"operation"`
	Reason        sdk.Reason    `json:"reason"`
	Detail        string        `json:"detail"`
	Transport     sdk.Transport `json:"transport"`
	HTTPStatus    int           `json:"http_status"`
	RetrySafe     bool          `json:"retry_safe"`
	RetryHasAfter bool          `json:"retry_has_after"`
	Canceled      bool          `json:"canceled"`
	Deadline      bool          `json:"deadline"`
	BrokenPipe    bool          `json:"broken_pipe"`
}
type ugoiraWorkflowOutcome struct {
	Stdout          string                  `json:"stdout"`
	Stderr          string                  `json:"stderr"`
	Exit            int                     `json:"exit"`
	Error           *ugoiraWorkflowError    `json:"error"`
	Requests        []ugoiraWorkflowRequest `json:"requests"`
	States          []ugoiraWorkflowState   `json:"states"`
	ConfigUnchanged bool                    `json:"config_unchanged"`
}
type ugoiraWorkflowGoObservations struct {
	PortLoads            int     `json:"port_loads"`
	Opens                []int64 `json:"opens"`
	Closes               int     `json:"closes"`
	CallbackCommitted    []bool  `json:"callback_committed"`
	Writes               int     `json:"writes"`
	AllWritesUnderLease  bool    `json:"all_writes_under_lease"`
	OutputVisibleAtClose bool    `json:"output_visible_at_close"`
	BodiesOpened         int64   `json:"bodies_opened"`
	BodiesClosed         int64   `json:"bodies_closed"`
	PendingBodyReads     int64   `json:"pending_body_reads"`
	GateReusable         bool    `json:"gate_reusable"`
	ActiveAfter          int64   `json:"active_after"`
}
type ugoiraWorkflowRow struct {
	Input          ugoiraWorkflowInput          `json:"input"`
	Outcome        ugoiraWorkflowOutcome        `json:"outcome"`
	Reuse          *ugoiraWorkflowOutcome       `json:"reuse,omitempty"`
	GoObservations ugoiraWorkflowGoObservations `json:"go_observations"`
}
type ugoiraWorkflowFixture struct {
	ReferenceCommit       string              `json:"reference_commit"`
	SourceSHA256          map[string]string   `json:"source_sha256"`
	MetadataFixtureSHA256 string              `json:"metadata_fixture_sha256"`
	Scope                 []string            `json:"scope"`
	Limitations           []string            `json:"limitations"`
	Cases                 []ugoiraWorkflowRow `json:"cases"`
}

type ugoiraWorkflowBody struct {
	io.Reader
	context context.Context
	pending bool
	started chan struct{}
	once    sync.Once
	opened  *atomic.Int64
	closed  *atomic.Int64
	reads   *atomic.Int64
}

func (b *ugoiraWorkflowBody) Read(p []byte) (int, error) {
	if b.pending {
		b.reads.Add(1)
		b.once.Do(func() { close(b.started) })
		<-b.context.Done()
		return 0, b.context.Err()
	}
	return b.Reader.Read(p)
}
func (b *ugoiraWorkflowBody) Close() error { b.closed.Add(1); return nil }

type ugoiraWorkflowWriter struct {
	out          bytes.Buffer
	failure      string
	active       *atomic.Int64
	gate         *pool.Gate
	observations *ugoiraWorkflowGoObservations
}

func (w *ugoiraWorkflowWriter) Write(p []byte) (int, error) {
	w.observations.Writes++
	if w.active.Load() != 1 {
		w.observations.AllWritesUnderLease = false
	}
	probe, cancel := context.WithTimeout(context.Background(), time.Millisecond)
	err := w.gate.Acquire(probe)
	cancel()
	if err == nil {
		w.gate.Release()
		w.observations.AllWritesUnderLease = false
	}
	switch w.failure {
	case "writer_broken":
		n, _ := w.out.Write(p[:min(8, len(p))])
		return n, syscall.EPIPE
	case "writer_denied":
		n, _ := w.out.Write(p[:min(8, len(p))])
		return n, errors.New("fixture output denied")
	case "writer_short":
		return w.out.Write(p[:min(8, len(p))])
	case "writer_retryable":
		if w.observations.Writes == 1 {
			n, _ := w.out.Write(p[:min(8, len(p))])
			return n, sdk.NewError("pixiv", "fixture_writer", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: time.Now().Add(120 * time.Second)}))
		}
	}
	return w.out.Write(p)
}

func ugoiraWorkflowSourceHashes(t *testing.T) map[string]string {
	t.Helper()
	files := []string{
		"internal/cli/commands/pixiv/ugoira/ugoira.go", "internal/cli/composition.go", "internal/cli/execution.go",
		"internal/services/pixiv/facade.go", "internal/services/pixiv/account/accounts.go", "internal/services/pixiv/account/pixiv.go",
		"internal/services/pixiv/pool/pool.go", "internal/services/pixiv/pool/replay.go", "internal/services/pixiv/pool/gate.go",
		"sdk/pixiv/ops_artwork.go", "sdk/pixiv/map_artwork.go", "sdk/pixiv/dto.go", "sdk/pixiv/errors.go",
	}
	result := map[string]string{}
	for _, file := range files {
		frozen, err := exec.Command("git", "show", ugoiraWorkflowReference+":"+file).Output()
		if err != nil {
			t.Fatal(err)
		}
		current, err := os.ReadFile(filepath.Join("..", "..", file))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(frozen, current) {
			t.Fatalf("saved ugoira source differs from frozen Go reference: %s", file)
		}
		digest := sha256.Sum256(current)
		result[file] = hex.EncodeToString(digest[:])
	}
	return result
}
func ugoiraWorkflowInputs(t *testing.T) ([]ugoiraWorkflowInput, string) {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "ugoira-metadata.json"))
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(data)
	var metadata []struct {
		Body json.RawMessage `json:"body"`
	}
	if err := json.Unmarshal(data, &metadata); err != nil {
		t.Fatal(err)
	}
	detail, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "mcp-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var details struct {
		Cases []struct {
			Body     json.RawMessage `json:"body"`
			Requests int             `json:"requests"`
			Result   struct {
				IsError bool `json:"isError"`
			} `json:"result"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(detail, &details); err != nil {
		t.Fatal(err)
	}
	var detailBody json.RawMessage
	for _, row := range details.Cases {
		if row.Requests == 1 && !row.Result.IsError {
			detailBody = row.Body
			break
		}
	}
	if detailBody == nil {
		t.Fatal("missing genuine artwork body")
	}
	var result []ugoiraWorkflowInput
	add := func(selection, scenario, kind string, index int, modes ...string) {
		for _, mode := range modes {
			var body map[string]any
			if err := json.Unmarshal(detailBody, &body); err != nil {
				t.Fatal(err)
			}
			body["illust"].(map[string]any)["type"] = kind
			encoded, err := json.Marshal(body)
			if err != nil {
				t.Fatal(err)
			}
			config := "# synthetic preserved comment\n[output]\njson = false\n"
			if selection == "default" {
				config += "[pixiv.auth]\ndefault_user_id = 43\n"
			}
			if selection == "pool" {
				config += "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n"
			}
			flags := []string{}
			if mode == "json" {
				flags = append(flags, "--json")
			}
			if mode == "configured_json" {
				config = strings.Replace(config, "json = false", "json = true", 1)
			}
			if mode == "explicit_false" {
				config = strings.Replace(config, "json = false", "json = true", 1)
				flags = append(flags, "--json=false")
			}
			result = append(result, ugoiraWorkflowInput{Name: selection + "/" + scenario + "/" + kind + "/" + mode + "/" + strconv.Itoa(index), Mode: mode, Selection: selection, Scenario: scenario, Kind: kind, MetadataIndex: index, Config: config, Flags: flags, DetailBody: encoded, MetadataBody: metadata[index].Body})
		}
	}
	for _, selection := range []string{"default", "fallback", "pool"} {
		add(selection, "success", "ugoira", 0, "human", "json")
	}
	for _, scenario := range []string{"detail_replay", "metadata_replay", "metadata_replay_kind", "metadata_replay_malformed", "detail_401", "metadata_401", "writer_retryable"} {
		add("pool", scenario, "ugoira", 0, "human", "json")
	}
	add("pool", "all_rate_limited", "ugoira", 0, "human")
	for _, scenario := range []string{"detail_401", "metadata_401", "detail_malformed", "refresh_401", "close_failure", "writer_broken", "writer_denied", "writer_short", "writer_close_failure", "detail_retry", "metadata_retry"} {
		add("default", scenario, "ugoira", 0, "human", "json")
	}
	for _, kind := range []string{"illust", "manga", "unrecognized"} {
		add("default", "success", kind, 0, "human", "json")
	}
	for _, index := range []int{5, 6, 7, 8, 9, 38} {
		add("default", "success", "ugoira", index, "json")
	}
	add("default", "success", "ugoira", 38, "human")
	for _, index := range []int{10, 37, 42} {
		add("default", "success", "ugoira", index, "human", "json")
	}
	add("default", "success", "ugoira", 0, "configured_json", "explicit_false")
	add("default", "metadata_401", "ugoira", 0, "configured_json", "explicit_false")
	for _, stage := range []string{"detail", "metadata"} {
		for _, stop := range []string{"cancel", "deadline"} {
			add("default", stage+"_"+stop, "ugoira", 0, "human", "json")
		}
	}
	return result, hex.EncodeToString(digest[:])
}

func ugoiraWorkflowCapture(t *testing.T, input ugoiraWorkflowInput) ugoiraWorkflowRow {
	t.Helper()
	row := ugoiraWorkflowRow{Input: input, GoObservations: ugoiraWorkflowGoObservations{Opens: []int64{}, CallbackCommitted: []bool{}, AllWritesUnderLease: true}}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	for _, name := range []string{"https_proxy", "HTTPS_PROXY"} {
		t.Setenv(name, "")
	}
	t.Setenv("PIXIV_REQUEST_INTERVAL", "0s")
	directory := filepath.Join(home, ".pixiv-cli")
	if err := os.MkdirAll(directory, 0700); err != nil {
		t.Fatal(err)
	}
	configPath := filepath.Join(directory, "config.toml")
	if err := os.WriteFile(configPath, []byte(input.Config), 0600); err != nil {
		t.Fatal(err)
	}
	db, err := database.Open(directory)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	background := context.Background()
	for _, id := range []int64{42, 43} {
		if err := db.SavePixivCredential(background, account.New(id, "synthetic", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
			t.Fatal(err)
		}
	}
	if err := db.SetAllPixivSchedulable(background, true); err != nil {
		t.Fatal(err)
	}
	var active, bodiesOpened, bodiesClosed, pendingReads atomic.Int64
	gate := pool.NewGate()
	output := &ugoiraWorkflowWriter{failure: input.Scenario, active: &active, gate: gate, observations: &row.GoObservations}
	requests := []ugoiraWorkflowRequest{}
	requestCounts := map[string]int{}
	pendingStarted := make(chan struct{})
	reuse := false
	transport := migrationDateTransport(func(req *http.Request) (*http.Response, error) {
		header := http.Header{"Content-Type": {"application/json"}}
		status := 200
		body := ""
		observation := ugoiraWorkflowRequest{Method: req.Method, Host: req.URL.Host, URI: req.URL.RequestURI()}
		if req.URL.Host == "oauth.secure.pixiv.net" {
			if req.Method != "POST" || req.URL.Path != "/auth/token" {
				t.Errorf("unexpected OAuth request: %s %s", req.Method, req.URL)
			}
			if err := req.ParseForm(); err != nil {
				return nil, err
			}
			token := req.Form.Get("refresh_token")
			id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
			if err != nil {
				return nil, err
			}
			stored, err := db.GetPixiv(background, id)
			if err != nil {
				return nil, err
			}
			if token != string(stored.RefreshTokenCopy()) {
				t.Error("refresh used stale credential")
			}
			observation.Account = id
			observation.Revision = stored.CredentialRevision
			observation.GrantType = req.Form.Get("grant_type")
			for key := range req.Form {
				observation.FormKeys = append(observation.FormKeys, key)
			}
			sort.Strings(observation.FormKeys)
			body = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id)
			if input.Scenario == "refresh_401" {
				status = 401
			} else {
				row.GoObservations.Opens = append(row.GoObservations.Opens, id)
				active.Add(1)
			}
		} else {
			if req.URL.Host != "app-api.pixiv.net" || req.Method != "GET" || req.URL.RawQuery != "illust_id=42" || (req.URL.Path != "/v1/illust/detail" && req.URL.Path != "/v1/ugoira/metadata") {
				t.Errorf("unexpected media/API request: %s %s", req.Method, req.URL)
				return nil, errors.New("unexpected fixture request")
			}
			token := req.Header.Get("Authorization")
			id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
			if err != nil {
				return nil, err
			}
			if token != fmt.Sprintf("Bearer fixture-access-%d", id) {
				t.Error("request did not use refreshed bearer")
			}
			stored, err := db.GetPixiv(background, id)
			if err != nil {
				return nil, err
			}
			observation.Account = id
			observation.Revision = stored.CredentialRevision
			observation.Persisted = stored.CredentialRevision >= 2 && string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id)
			if !observation.Persisted {
				t.Error("API request preceded refresh persistence")
			}
			stage := "detail"
			body = string(input.DetailBody)
			if req.URL.Path == "/v1/ugoira/metadata" {
				stage = "metadata"
				body = string(input.MetadataBody)
			}
			key := fmt.Sprintf("%d/%s", id, stage)
			requestCounts[key]++
			if !reuse {
				switch input.Scenario {
				case "detail_401", "metadata_401":
					if input.Scenario == stage+"_401" {
						status = 401
					}
				case "detail_malformed":
					if stage == "detail" {
						body = `{}`
					}
				case "detail_retry", "metadata_retry":
					if input.Scenario == stage+"_retry" && requestCounts[key] == 1 {
						status = 429
						header.Set("Retry-After", "0")
					}
				case "detail_replay", "metadata_replay", "metadata_replay_kind", "metadata_replay_malformed", "all_rate_limited":
					failingStage := "metadata"
					if input.Scenario == "detail_replay" {
						failingStage = "detail"
					}
					if stage == failingStage && (id == 42 || input.Scenario == "all_rate_limited") {
						status = 429
						if requestCounts[key] == 1 {
							header.Set("Retry-After", "0")
						} else {
							header.Set("Retry-After", "120")
						}
					}
					if id == 43 && input.Scenario == "metadata_replay_kind" && stage == "detail" {
						var changed map[string]any
						_ = json.Unmarshal([]byte(body), &changed)
						changed["illust"].(map[string]any)["type"] = "illust"
						data, _ := json.Marshal(changed)
						body = string(data)
					}
					if id == 43 && input.Scenario == "metadata_replay_malformed" && stage == "metadata" {
						body = `{}`
					}
				}
			}
		}
		requests = append(requests, observation)
		pending := !reuse && (input.Scenario == "detail_cancel" || input.Scenario == "detail_deadline") && req.URL.Path == "/v1/illust/detail" || !reuse && (input.Scenario == "metadata_cancel" || input.Scenario == "metadata_deadline") && req.URL.Path == "/v1/ugoira/metadata"
		bodiesOpened.Add(1)
		return &http.Response{StatusCode: status, Header: header, Body: &ugoiraWorkflowBody{Reader: strings.NewReader(body), context: req.Context(), pending: pending, started: pendingStarted, opened: &bodiesOpened, closed: &bodiesClosed, reads: &pendingReads}, Request: req}, nil
	})
	facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, settings.DefaultStore()), Gate: gate, LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
		runtime, err := (app{}).runtimeConfig()
		return pixivapp.PoolConfig{Enabled: runtime.AccountPool.Enabled, Strategy: string(runtime.AccountPool.Strategy)}, err
	}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
		return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
	}, CloseClient: func(client *pixiv.Client) error {
		row.GoObservations.Closes++
		active.Add(-1)
		row.GoObservations.OutputVisibleAtClose = row.GoObservations.OutputVisibleAtClose || output.out.Len() > 0
		client.CloseIdleConnections()
		if input.Scenario == "close_failure" || input.Scenario == "writer_close_failure" {
			return errors.New("fixture client close failed")
		}
		return nil
	}})
	oldPorts := newCLIPixivSDKPorts
	defer func() { newCLIPixivSDKPorts = oldPorts }()
	newCLIPixivSDKPorts = func(a app) (pixivSDKPorts, error) {
		row.GoObservations.PortLoads++
		return pixivSDKPorts{jsonOut: func(override *bool) (bool, error) {
			if override != nil {
				return *override, nil
			}
			runtime, err := a.runtimeConfig()
			return runtime.OutputJSON, err
		}, execute: func(ctx context.Context, request pixivdeps.Request, callback func(context.Context, *pixiv.Client) (bool, error)) error {
			options, err := pixivOptionsFromRequest(request, a.runtimeConfig)
			if err != nil {
				return err
			}
			options.HTTPClient.Transport = transport
			return facade.Use(ctx, pixivapp.Request{UserID: request.UserID, Options: options}, func(ctx context.Context, client *pixiv.Client) (bool, error) {
				committed, err := callback(ctx, client)
				row.GoObservations.CallbackCommitted = append(row.GoObservations.CallbackCommitted, committed)
				return committed, err
			})
		}}, nil
	}
	execute := func(ctx context.Context) ugoiraWorkflowOutcome {
		output.out.Reset()
		var diagnostics bytes.Buffer
		a := app{in: strings.NewReader(""), out: output, errOut: &diagnostics, closeState: &closeState{}}
		command := ugoiracmd.New(a.ugoiraDeps())
		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(command)
		root.SetOut(output)
		root.SetErr(&diagnostics)
		root.SetContext(ctx)
		root.SetArgs(append([]string{"ugoira", "42"}, input.Flags...))
		_, err := root.ExecuteC()
		current := ugoiraWorkflowOutcome{Stdout: output.out.String(), Requests: append([]ugoiraWorkflowRequest{}, requests...), States: []ugoiraWorkflowState{}}
		current.Exit = a.exitWithNDJSONScope(err, false, commandExplicitJSON(command))
		current.Stderr = diagnostics.String()
		if err != nil {
			current.Error = &ugoiraWorkflowError{Message: err.Error(), Canceled: errors.Is(err, context.Canceled), Deadline: errors.Is(err, context.DeadlineExceeded), BrokenPipe: errors.Is(err, syscall.EPIPE)}
			var typed *sdk.Error
			if errors.As(err, &typed) {
				current.Error.Product = typed.Product
				current.Error.Operation = typed.Operation
				current.Error.Reason = typed.Reason
				current.Error.Detail = typed.Detail
				current.Error.Transport = typed.Transport
				current.Error.HTTPStatus = typed.HTTPStatus
				current.Error.RetrySafe = typed.Retry.Safe
				current.Error.RetryHasAfter = typed.Retry.HasAfter
			}
		}
		for _, id := range []int64{42, 43} {
			stored, err := db.GetPixiv(background, id)
			if err != nil {
				t.Fatal(err)
			}
			current.States = append(current.States, ugoiraWorkflowState{ID: id, Revision: stored.CredentialRevision, Rotated: string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id), Frozen: stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), Selected: stored.PoolLastSelected})
		}
		after, err := os.ReadFile(configPath)
		if err != nil {
			t.Fatal(err)
		}
		current.ConfigUnchanged = bytes.Equal(after, []byte(input.Config))
		return current
	}
	if strings.HasSuffix(input.Scenario, "_cancel") || strings.HasSuffix(input.Scenario, "_deadline") {
		ctx, cancel := context.WithCancel(background)
		if strings.HasSuffix(input.Scenario, "_deadline") {
			cancel()
			ctx, cancel = context.WithTimeout(background, 250*time.Millisecond)
		}
		done := make(chan ugoiraWorkflowOutcome, 1)
		go func() { done <- execute(ctx) }()
		select {
		case <-pendingStarted:
		case early := <-done:
			cancel()
			t.Fatalf("pending command returned before body read: %#v", early)
		case <-time.After(5 * time.Second):
			cancel()
			t.Fatal("pending SDK body did not start")
		}
		if strings.HasSuffix(input.Scenario, "_cancel") {
			cancel()
		}
		select {
		case row.Outcome = <-done:
		case <-time.After(5 * time.Second):
			cancel()
			t.Fatal("pending SDK body did not stop")
		}
		cancel()
		expected := row.Outcome.Error
		if expected == nil || !expected.Canceled && !expected.Deadline {
			t.Fatal("pending response lost context identity")
		}
		if active.Load() != 0 || bodiesOpened.Load() != bodiesClosed.Load() {
			t.Fatal("pending request retained client/body owner")
		}
		reuse = true
		requests = []ugoiraWorkflowRequest{}
		output.failure = ""
		next := execute(background)
		row.Reuse = &next
		if next.Exit != 0 || next.Stdout == "" {
			t.Fatal("saved owner was not reusable after pending body stopped")
		}
	} else {
		if input.Scenario == "writer_close_failure" {
			output.failure = "writer_denied"
		}
		row.Outcome = execute(background)
	}
	probe, cancel := context.WithTimeout(background, time.Second)
	if err := gate.Acquire(probe); err == nil {
		row.GoObservations.GateReusable = true
		gate.Release()
	}
	cancel()
	row.GoObservations.ActiveAfter = active.Load()
	row.GoObservations.BodiesOpened = bodiesOpened.Load()
	row.GoObservations.BodiesClosed = bodiesClosed.Load()
	row.GoObservations.PendingBodyReads = pendingReads.Load()
	if row.GoObservations.ActiveAfter != 0 || !row.GoObservations.GateReusable || bodiesOpened.Load() != bodiesClosed.Load() {
		t.Fatal("saved ugoira leaked account, gate or response body")
	}
	if row.GoObservations.Writes > 0 && (!row.GoObservations.AllWritesUnderLease || !row.GoObservations.OutputVisibleAtClose) {
		t.Fatal("ugoira output escaped genuine account lease")
	}
	for _, committed := range row.GoObservations.CallbackCommitted {
		if committed {
			t.Fatal("ugoira callback committed output")
		}
	}

	if len(row.Outcome.Requests) == 0 {
		t.Fatal("saved workflow never reached synthetic OAuth")
	}
	for _, request := range row.Outcome.Requests {
		if request.Host == "app-api.pixiv.net" && !request.Persisted {
			t.Fatal("API request preceded refresh persistence")
		}
	}
	if input.Scenario == "success" && input.Kind == "ugoira" && input.MetadataIndex != 10 && input.MetadataIndex != 37 && input.MetadataIndex != 42 && (row.Outcome.Exit != 0 || row.Outcome.Stdout == "") {
		t.Fatal("successful saved ugoira failed to publish metadata")
	}
	if input.Kind != "ugoira" {
		if row.Outcome.Error == nil || row.Outcome.Error.Reason != sdk.NotUgoira || len(row.Outcome.Requests) != 2 {
			t.Fatal("kind preflight did not stop before metadata")
		}
	}
	if input.Scenario == "metadata_replay" || input.Scenario == "writer_retryable" {
		if row.Outcome.Exit != 0 || len(row.GoObservations.Opens) != 2 {
			t.Fatal("whole-attempt replay did not reach the next saved account")
		}
		details := map[int64]int{}
		for _, request := range row.Outcome.Requests {
			if request.URI == "/v1/illust/detail?illust_id=42" {
				details[request.Account]++
			}
		}
		if details[42] != 1 || details[43] != 1 {
			t.Fatal("metadata replay reused detail from a previous account")
		}
	}
	if input.Scenario == "writer_short" && row.Outcome.Exit != 0 {
		t.Fatal("nil-error short write was converted to failure")
	}
	if input.Scenario == "writer_broken" && (row.Outcome.Exit != 1 || row.Outcome.Error == nil || !row.Outcome.Error.BrokenPipe) {
		t.Fatal("standalone ugoira converted broken pipe to success")
	}
	if !row.Outcome.ConfigUnchanged {
		t.Fatal("ugoira changed configuration")
	}
	for _, secret := range []string{"fixture-access", "fixture-refresh", "fixture-rotated", "https://i.pximg.net", "fixture-signature", "fixture=secret"} {
		if strings.Contains(row.Outcome.Stdout+row.Outcome.Stderr, secret) {
			t.Fatalf("ugoira output exposed resource/credential: %s", secret)
		}
	}
	return row
}

func TestMigrationUgoiraWorkflowPreservesSavedReplayOutputLeaseAndPendingBodies(t *testing.T) {
	fixture := ugoiraWorkflowFixture{ReferenceCommit: ugoiraWorkflowReference, SourceSHA256: ugoiraWorkflowSourceHashes(t), Scope: []string{"Real app.ugoiraDeps command composition at the existing injected SDK ports boundary, real SQLite saved accounts/default/pool, real refresh persistence, facade, gate, scheduler and public SDK.", "Only synthetic OAuth/artwork-detail/metadata HTTP routes are accepted. No archive/media, native encoder, browser, registration, update or external service executes.", "Public outcome fields and exact requests/states are distinct from Go-only callback, writer, body and lease observations."}, Limitations: []string{"The command runs below root startup hooks; startup/config/help/child behavior is covered by the separate ugoira startup fixture.", "Synthetic transports retain public request URLs and SDK decoder behavior without proving successful external HTTPS, native OS behavior or all concurrent cancellation schedules.", "Metadata validation is sampled from the unchanged 45-row SDK fixture; that fixture remains the complete metadata-body validation boundary.", "A synthetic retryable SDK writer error establishes the uncommitted whole-attempt replay contract after partial output, not an upstream HTTP event."}}
	inputs, metadataHash := ugoiraWorkflowInputs(t)
	fixture.MetadataFixtureSHA256 = metadataHash
	for _, input := range inputs {
		t.Run(input.Name, func(t *testing.T) { fixture.Cases = append(fixture.Cases, ugoiraWorkflowCapture(t, input)) })
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli-ugoira-workflow.json")
	if *updateUgoiraWorkflow {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, expected) {
		t.Fatal("saved ugoira workflow differs from frozen Go reference")
	}
	t.Logf("%d frozen saved ugoira workflow cases", len(fixture.Cases))
}
