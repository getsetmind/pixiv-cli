package download_test

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
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	download "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/download"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateRecordWorkflowCLI = flag.Bool("migration-update-record-cli", false, "capture saved record download contracts from pinned Go")

type recordWorkflowFixture struct {
	Reference      string               `json:"reference"`
	SourceSHA256   map[string]string    `json:"source_sha256"`
	Normalization  string               `json:"normalization"`
	Correspondence []string             `json:"correspondence"`
	Remaining      []string             `json:"remaining"`
	Assets         map[string]string    `json:"assets_hex"`
	Cases          []recordWorkflowCase `json:"cases"`
}
type recordWorkflowCase struct {
	Name              string                     `json:"name"`
	Args              []string                   `json:"args"`
	Input             string                     `json:"input"`
	RuntimeJSON       bool                       `json:"runtime_json"`
	Filename          string                     `json:"runtime_filename"`
	Directory         string                     `json:"runtime_directory"`
	Failure           string                     `json:"failure"`
	FailureID         int64                      `json:"failure_id"`
	FailureAccount    int64                      `json:"failure_account"`
	StaticContentType string                     `json:"static_content_type"`
	ReaderFailure     string                     `json:"reader_failure"`
	WriterFailure     string                     `json:"writer_failure"`
	Followup          bool                       `json:"followup"`
	Stdout            string                     `json:"stdout"`
	Stderr            string                     `json:"stderr"`
	Error             string                     `json:"error"`
	Cause             string                     `json:"cause"`
	Pipeline          bool                       `json:"pipeline"`
	Events            []string                   `json:"events"`
	Pooled            []recordWorkflowPooled     `json:"pooled"`
	PrivateAttempts   []recordWorkflowAttempt    `json:"go_private_attempts"`
	Saves             []ugoiraWorkflowCLISave    `json:"saves"`
	Requests          []ugoiraWorkflowCLIRequest `json:"requests"`
	Accounts          []ugoiraWorkflowCLIAccount `json:"accounts"`
	Closes            int                        `json:"closes"`
	BodyCloses        int                        `json:"body_closes"`
	ActiveAfter       int                        `json:"active_after"`
	WriterAttempts    []recordWorkflowWrite      `json:"writer_attempts"`
	ReaderReturned    string                     `json:"reader_returned"`
	Files             map[string]string          `json:"files"`
	Directories       []string                   `json:"directories"`
	After             *recordWorkflowAfter       `json:"after"`
	MainPooled        int                        `json:"main_pooled"`
	MainRequests      int                        `json:"main_requests"`
	MainSaves         int                        `json:"main_saves"`
	GateEvents        []string                   `json:"go_gate_port_events"`
	HeldGate          *recordWorkflowHeldGate    `json:"held_gate"`
}
type recordWorkflowHeldGate struct {
	Active   int `json:"active_while_canceled"`
	Requests int `json:"requests_while_held"`
	Saves    int `json:"saves_while_held"`
	Attempts int `json:"attempts_while_held"`
}
type recordWorkflowGate struct {
	gate *pool.Gate
	row  *recordWorkflowCase
}

func (g recordWorkflowGate) Acquire(ctx context.Context) error {
	g.row.GateEvents = append(g.row.GateEvents, "acquire")
	err := g.gate.Acquire(ctx)
	result := "acquired"
	if err != nil {
		result = err.Error()
	}
	g.row.GateEvents = append(g.row.GateEvents, result)
	return err
}
func (g recordWorkflowGate) Release() {
	g.gate.Release()
	g.row.GateEvents = append(g.row.GateEvents, "released")
}

type recordWorkflowPooled struct {
	Ordinal int     `json:"ordinal"`
	Proxy   *string `json:"proxy_override"`
	Error   string  `json:"error"`
	Cause   string  `json:"cause"`
}
type recordWorkflowAttempt struct {
	Ordinal   int    `json:"pooled_ordinal"`
	Committed bool   `json:"committed"`
	Error     string `json:"error"`
	Cause     string `json:"cause"`
}
type recordWorkflowWrite struct {
	Text       string `json:"text"`
	UnderLease bool   `json:"under_lease"`
	Failed     bool   `json:"failed"`
}
type recordWorkflowAfter struct {
	Error  string            `json:"error"`
	Cause  string            `json:"cause"`
	Stdout string            `json:"stdout"`
	Stderr string            `json:"stderr"`
	Files  map[string]string `json:"files"`
	Active int               `json:"active"`
}
type recordWorkflowWriter struct {
	buffer     *bytes.Buffer
	row        *recordWorkflowCase
	active     *int
	normalize  func(string) string
	failures   int
	persistent bool
}

func (w *recordWorkflowWriter) Write(b []byte) (int, error) {
	failed := w.persistent || w.failures > 0
	if w.failures > 0 {
		w.failures--
	}
	w.row.WriterAttempts = append(w.row.WriterAttempts, recordWorkflowWrite{w.normalize(string(b)), *w.active > 0, failed})
	if failed {
		return 0, errors.New("fixture writer failure")
	}
	return w.buffer.Write(b)
}

type recordWorkflowBody struct {
	reader  *strings.Reader
	cancel  context.CancelFunc
	failure string
	closes  *int
}

func (b *recordWorkflowBody) Read(p []byte) (int, error) {
	if b.reader.Len() > 0 {
		return b.reader.Read(p)
	}
	if b.failure == "cancel" {
		b.cancel()
		return 0, context.Canceled
	}
	if b.failure == "read" {
		return 0, errors.New("fixture body failure")
	}
	return 0, io.EOF
}
func (b *recordWorkflowBody) Close() error { *b.closes++; return nil }

type recordWorkflowReader struct {
	body     []byte
	pos      int
	failure  string
	returned *string
	cancel   context.CancelFunc
}

func (r *recordWorkflowReader) Read(p []byte) (int, error) {
	if r.failure == "classification" {
		return 0, errors.New("fixture input failure")
	}
	if r.pos == len(r.body) {
		if r.failure == "after-first" || r.failure == "partial" {
			return 0, errors.New("fixture input failure")
		}
		return 0, io.EOF
	}
	n := len(p)
	if r.failure == "after-first" {
		if end := bytes.IndexByte(r.body[r.pos:], '\n'); end >= 0 && n > end+1 {
			n = end + 1
		}
	}
	if n > len(r.body)-r.pos {
		n = len(r.body) - r.pos
	}
	copy(p, r.body[r.pos:r.pos+n])
	r.pos += n
	*r.returned += string(p[:n])
	if r.failure == "partial" && r.pos == len(r.body) {
		return n, errors.New("fixture input failure")
	}
	if r.failure == "cancel-classified" && r.pos == 1 {
		r.cancel()
	}
	return n, nil
}

func recordWorkflowInput(typ string, id int64) string {
	return fmt.Sprintf(`{"type":%q,"id":%d,"url":"https://misleading.invalid/users/999?private=ignored"}`+"\n", typ, id)
}

func TestMigrationRecordWorkflowMatchesFrozenCLIContracts(t *testing.T) {
	rootRepo := filepath.Join("..", "..", "..", "..", "..")
	fixture := recordWorkflowFixture{
		Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA256: map[string]string{},
		Normalization:  "Only the isolated temporary download root becomes $ROOT and owned generated ugoira archive basenames become ugoira-$TEMP.zip. Shared synthetic archives reuse migration_ugoira_workflow_test.go. File and archive bytes are hex encoded; output, diagnostics, request order, IDs, resource refs and account state stay exact. Reader-returned bytes describe this supplied Reader, not an arbitrary no-prefetch promise.",
		Correspondence: []string{"actual download Cobra controller -> prepared Rust DownloadCommand/record consumer; actual saved facade/account/SQLite/pool -> existing Rust saved Execution/facade/session scheduler; public SDK detail/resource -> NativeDownloadSaveClient and existing downloader/media", "pooled ordinals, requests, saves, writer calls, body closes, persisted state and files are directly observed at real ports; go_private_attempts and go_gate_port_events observe Go callback/Attempt/RotationGate boundaries and do not require inventing Rust test-only production hooks; main_pooled/main_requests/main_saves split the first command from the optional same-owner followup", "a held facade.Open lease and real Gate Acquire cancel boundary are tested; fresh commands reuse the same facade/gate/SQLite owner after cancellation, not the canceled Context"},
		Remaining:      []string{"native cli root exit/envelope/startup is the separate record-startup fixture; no external network, real credentials, browser or OS association effects", "codec/parser/diagnostic identity exhaustive grammar is the separate pipeline fixture; existing static/ugoira manager fixtures own media Cartesian and native encoder matrices", "arbitrary blocking Readers, general public SDK injected/owned HTTP client cleanup, native asynchronous future-drop schedules and persisted resume/progress are not established by this fixture"},
		Assets:         map[string]string{"ugoira_valid": hex.EncodeToString(generatedUgoiraCLIArchive(t, "")), "ugoira_invalid-image": hex.EncodeToString(generatedUgoiraCLIArchive(t, "invalid-image"))}, Cases: []recordWorkflowCase{},
	}
	pinnedSources := map[string]string{
		"internal/cli/commands/pixiv/download/execution.go": "236460386b2847dc03a00f10ac1e955ad1fca9f18c173ae0a1523f960812fdf0",
		"internal/cli/commands/pixiv/download/report.go":    "7d75a279defc954df3ab326af750fa1ca51021942a2bc4a205c34b80422b0b63",
		"internal/cli/pipeline/action.go":                   "74c540329878d2f87db3643056dc0f1d58cde5d4f98d90e9bec4421cd189161c",
		"internal/cli/pipeline/pipeline.go":                 "294d15b516da78e677dfde04a59daf97d5c1adff76e0328bb43d0d3bfe7e7d3c",
		"internal/cli/pipeline/records.go":                  "c4a94d230db4ca95661baeac26c6f912ab6ade7c8a3808bff05878609152eb29",
		"internal/shared/record/json.go":                    "8a41ca7fa1e7d2841532cf435e5ae08560bb6f677794b4ba4c4b7e113c0ab858",
		"internal/utils/parse/parse.go":                     "e99a8188c2d8a5fc9f4d053a47ff8c0ed8bbaa539d4abd79ba30f06d778e7bb1",
		"internal/services/pixiv/facade.go":                 "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
		"internal/services/pixiv/account/accounts.go":       "129d83a89a09bc8a4dbd926ce19e9551fedf048a4c1fb9f71d551ee3089fad91",
		"internal/services/pixiv/pool/pool.go":              "be7f58dba0e1957268551bbf01bd73a9edc8365a6ec57751d2668960bda681d4",
		"internal/services/pixiv/pool/gate.go":              "33387af051432f6a6e6a8d0a8bbbcc88a5035ef6a65cbd30846f3ea9a70990a6",
		"internal/media/downloader/downloader.go":           "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"internal/services/pixiv/pool/chooser.go":           "e86a6dc2a57ce486522d0fdb82a1576e8386c7789d29be4c1de46914f107a811",
		"internal/services/pixiv/pool/replay.go":            "54559f3852e1070f6a6acfbf34b217a2ccfd139cd8d6e60157880c0accd6a282",
		"internal/services/pixiv/appapi/appapi.go":          "b5d9502ad9c534c88bda076740553c8bb6f389c3d2f4e3bd80c32c7f281ba9ff",
		"internal/storage/file/atomic/atomic.go":            "1881f9838ef3b18969e8554d4eba9838111481c0f367df2ef4f77a8c67913c46",
		"internal/storage/database/repository.go":           "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		"internal/storage/database/pool.go":                 "40c8a456110e5cb1b4ac88e6c49277f97696233d7a0237ada480ef85e0145ed0",
		"sdk/pixiv/pixiv.go":                                "daefb42f9f90359f5ce3d326d18df7afe8c10615750d88fef049cce78224d317",
		"sdk/pixiv/map_artwork.go":                          "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
		"sdk/pixiv/resource.go":                             "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
	}
	for name, expected := range pinnedSources {
		source, err := os.ReadFile(filepath.Join(rootRepo, filepath.FromSlash(name)))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(source)
		actual := hex.EncodeToString(sum[:])
		if actual != expected {
			t.Fatalf("reference source changed: %s", name)
		}
		fixture.SourceSHA256[name] = actual
	}
	add := func(name, input string, args ...string) *recordWorkflowCase {
		if args == nil {
			args = []string{}
		}
		fixture.Cases = append(fixture.Cases, recordWorkflowCase{Name: name, Input: input, Args: args, Filename: "{id}_{num}", FailureID: 43, FailureAccount: 42, StaticContentType: "image/jpeg"})
		return &fixture.Cases[len(fixture.Cases)-1]
	}
	first := recordWorkflowInput("artwork", 42)
	second := recordWorkflowInput("manga", 43)
	third := recordWorkflowInput("ugoira", 44)
	modes := []struct {
		name    string
		args    []string
		runtime bool
	}{
		{"human", []string{}, false}, {"runtime-json", []string{}, true}, {"json", []string{"--json"}, false}, {"ndjson", []string{"--ndjson"}, false},
		{"both", []string{"--json", "--ndjson"}, false}, {"both-false", []string{"--json=false", "--ndjson=false"}, true},
		{"json-false-ndjson", []string{"--json=false", "--ndjson"}, true}, {"json-ndjson-false", []string{"--json", "--ndjson=false"}, false},
		{"json-false", []string{"--json=false"}, true}, {"ndjson-false", []string{"--ndjson=false"}, true},
	}
	for _, mode := range modes {
		row := add("suppressed-"+mode.name, first, mode.args...)
		row.RuntimeJSON = mode.runtime
	}
	for _, mode := range []string{"gif", "apng", "zip", "raw"} {
		add("mixed-repeat-"+mode, first+recordWorkflowInput("illust", 42)+second+third+recordWorkflowInput("ugoira", 42), "--ugoira-mode="+mode, "--json", "--ndjson")
	}
	add("record-id-controls-url-and-metadata-kind", recordWorkflowInput("illust", 44)+recordWorkflowInput("ugoira", 42), "--ugoira-mode=zip")
	row := add("record-prepared-static-options", second, "--pages=2", "--quality=regular", "--filename-template={author_id}_{id}_{num}", "--no-proxy")
	row.Directory = "{date}/{id}"
	add("record-sign-leading-zero-and-unchecked-url", `{"type":"artwork","id":"+42","url":"opaque"}`+"\n"+`{"type":"illust","id":"0042","url":" "}`+"\n")
	for _, strategy := range []string{"skip", "fail-fast"} {
		add("malformed-before-success-"+strategy, "{broken}\n"+first, "--on-error="+strategy)
		add("malformed-between-success-"+strategy, first+"{broken}\n"+second, "--on-error="+strategy)
		add("malformed-after-success-"+strategy, first+"{broken}\n", "--on-error="+strategy)
		add("type-id-failure-between-success-"+strategy, first+recordWorkflowInput("user", 1)+`{"type":"artwork","id":" 43 ","url":"opaque"}`+"\n"+second, "--on-error="+strategy)
		row = add("business-between-success-"+strategy, first+second+third, "--on-error="+strategy, "--ugoira-mode=zip")
		row.Failure = "metadata-status"
	}
	row = add("prior-success-later-precommit-rate-replays-current-record", first+second+third, "--ugoira-mode=zip")
	row.Failure = "metadata-rate"
	row.FailureID = 43
	row.FailureAccount = 43
	row = add("first-record-precommit-rate", first+second, "--ugoira-mode=zip")
	row.Failure = "metadata-rate"
	row.FailureID = 42
	row = add("same-record-partial-prefix-blocks-rate-replay", second+first, "--quality=regular")
	row.Failure = "variant-rate"
	row = add("same-record-resource-429-published-prefix-no-replay", second+first)
	row.Failure = "page-rate"
	row = add("repeated-record-MIME-extension-collision", first+first)
	row.StaticContentType = "image/png"
	row = add("exhausted-accounts-no-success-or-later-network", first+second)
	row.Failure = "all-rate"
	row.FailureID = 42
	row = add("refresh-rate-not-replayed-next-record-opens-another-account", first+second)
	row.Failure = "refresh-rate"
	row.FailureID = 42
	row = add("refresh-unauthorized-skip-and-reuse", first+second)
	row.Failure = "refresh-status"
	row = add("no-saved-accounts", first+second)
	row.Failure = "no-accounts"
	row = add("no-schedulable-accounts", first+second)
	row.Failure = "no-schedulable"
	row = add("all-accounts-already-frozen", first+second)
	row.Failure = "already-frozen"
	row = add("warning-before-business-diagnostic", third+third, "--filename-template={unknown}")
	row.Failure = "invalid-image"
	row.FailureID = 44
	row = add("warning-fail-once-preserves-typed-business-and-continues", third+third, "--filename-template={unknown}")
	row.Failure = "ugoira-status"
	row.FailureID = 44
	row.WriterFailure = "once"
	row = add("warning-fail-once-preserves-typed-rate-and-replays", third+third, "--filename-template={unknown}")
	row.Failure = "ugoira-rate"
	row.FailureID = 44
	row.WriterFailure = "once"
	row = add("warning-writer-failure-after-commit-diagnostic-and-continue", third+third, "--filename-template={unknown}")
	row.WriterFailure = "once"
	row = add("persistent-warning-and-diagnostic-writer-is-fatal", third+third, "--filename-template={unknown}")
	row.Failure = "ugoira-status"
	row.FailureID = 44
	row.WriterFailure = "persistent"
	row = add("diagnostic-writer-failure-after-prior-success", first+"{broken}\n"+second)
	row.WriterFailure = "persistent"
	row = add("cancel-before-first-action", first+second)
	row.Failure = "before-cancel"
	row.Followup = true
	row = add("cancel-on-classification-before-first-action", first+second)
	row.ReaderFailure = "cancel-classified"
	row.Followup = true
	row = add("cancel-after-earlier-publication-no-later-record", first+second+third)
	row.Failure = "after-first"
	row.Followup = true
	for _, mode := range []string{"static", "ugoira-zip", "ugoira-gif"} {
		input := first + second + third
		target := int64(43)
		args := []string{}
		if mode != "static" {
			input = first + third + second
			target = 44
			args = []string{"--ugoira-mode=" + strings.TrimPrefix(mode, "ugoira-")}
		}
		row = add("owned-body-cancel-"+mode+"-prefix-retained-and-reusable", input, args...)
		row.Failure = "body-cancel"
		row.FailureID = target
		row.Followup = true
	}
	row = add("same-record-second-page-cancel-keeps-published-prefix", second+first)
	row.Failure = "page-cancel"
	row.Followup = true
	row = add("partial-static-body-failure-atomic-cleanup", second+first)
	row.Failure = "body-read"
	row.Followup = true
	row = add("gate-wait-cancel-with-live-lease-and-reusable-owner", first+second)
	row.Failure = "gate-cancel"
	row.Followup = true
	row = add("refresh-body-cancel-no-credential-commit-and-reusable", first+second)
	row.Failure = "refresh-cancel"
	row.Followup = true
	row = add("reader-classification-failure-no-actions", first+second)
	row.ReaderFailure = "classification"
	row.Followup = true
	row = add("reader-error-after-success-no-partial-action", first+strings.TrimSuffix(second, "\n"))
	row.ReaderFailure = "after-first"
	row.Followup = true
	row = add("reader-data-and-error-never-executes-incomplete-record", strings.TrimSuffix(first, "\n"))
	row.ReaderFailure = "partial"
	row.Followup = true
	row = add("explicit-source-does-not-read-record-input", first+second, "42")
	row.ReaderFailure = "classification"
	row = add("successful-final-unterminated-record-observes-EOF-before-cancellation", strings.TrimSuffix(first, "\n"))
	row.Failure = "after-first"
	row.Followup = true
	for i := range fixture.Cases {
		t.Run(fixture.Cases[i].Name, func(t *testing.T) { runRecordWorkflowCase(t, &fixture.Cases[i], fixture.Assets) })
	}
	if t.Failed() {
		return
	}
	path := filepath.Join(rootRepo, "crates", "pixiv-cli", "tests", "fixtures", "download_records.json")
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *updateRecordWorkflowCLI {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d saved record cases", len(fixture.Cases))
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("saved record workflow differs from pinned Go reference")
	}
}

func runRecordWorkflowCase(t *testing.T, row *recordWorkflowCase, assets map[string]string) {
	t.Helper()
	root := t.TempDir()
	dbRoot := t.TempDir()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	db, err := database.Open(dbRoot)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if row.Failure != "no-accounts" {
		for _, id := range []int64{42, 43} {
			if err := db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
				t.Fatal(err)
			}
		}
	}
	if row.Failure != "no-schedulable" {
		if err := db.SetAllPixivSchedulable(ctx, true); err != nil {
			t.Fatal(err)
		}
	}
	if row.Failure == "already-frozen" {
		for _, id := range []int64{42, 43} {
			if err := db.Freeze(ctx, id, time.Now().Add(time.Hour).Unix()); err != nil {
				t.Fatal(err)
			}
		}
	}
	row.Events = []string{}
	row.Pooled = []recordWorkflowPooled{}
	row.PrivateAttempts = []recordWorkflowAttempt{}
	row.Requests = []ugoiraWorkflowCLIRequest{}
	row.Saves = []ugoiraWorkflowCLISave{}
	row.Accounts = []ugoiraWorkflowCLIAccount{}
	row.WriterAttempts = []recordWorkflowWrite{}
	row.GateEvents = []string{}
	row.Files = map[string]string{}
	row.Directories = []string{}
	normalize := func(s string) string { return normalizeUgoiraCLIPath(strings.ReplaceAll(s, root, "$ROOT")) }
	var mutex sync.Mutex
	active := 0
	lastID := int64(0)
	counts := map[string]int{}
	disableFailure := false
	transport := directDownloadTransport(func(req *http.Request) (*http.Response, error) {
		mutex.Lock()
		defer mutex.Unlock()
		accountID := lastID
		payload := "payload"
		status := 200
		header := http.Header{"Content-Type": {row.StaticContentType}}
		bodyFailure := ""
		if req.URL.Host == "oauth.secure.pixiv.net" {
			if err := req.ParseForm(); err != nil {
				return nil, err
			}
			token := req.Form.Get("refresh_token")
			parsed, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
			if err != nil {
				return nil, err
			}
			accountID = parsed
			lastID = parsed
			stored, err := db.GetPixiv(context.Background(), parsed)
			if err != nil {
				return nil, err
			}
			if string(stored.RefreshTokenCopy()) != token {
				t.Fatal("stale refresh credential")
			}
			payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, parsed, parsed, parsed)
			header.Set("Content-Type", "application/json")
			if !disableFailure && (row.Failure == "refresh-rate" && accountID == 42) {
				status = 429
				header.Set("Retry-After", "120")
			}
			if !disableFailure && row.Failure == "refresh-status" && accountID == 42 {
				status = 401
			}
			if !disableFailure && row.Failure == "refresh-cancel" {
				bodyFailure = "cancel"
			}
		} else {
			stored, err := db.GetPixiv(context.Background(), accountID)
			if err != nil {
				return nil, err
			}
			if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", accountID) {
				t.Fatal("request preceded persisted refresh")
			}
			if active != 1 {
				t.Fatalf("request has %d active leases", active)
			}
			if req.URL.Host == "app-api.pixiv.net" && req.Header.Get("Authorization") != fmt.Sprintf("Bearer fixture-access-%d", accountID) {
				t.Fatal("wrong metadata authorization")
			}
			if req.URL.Host == "i.pximg.net" && req.Header.Get("Authorization") != "" {
				t.Fatal("resource leaked authorization")
			}
			if req.URL.Host == "app-api.pixiv.net" {
				id, err := strconv.ParseInt(req.URL.Query().Get("illust_id"), 10, 64)
				if err != nil {
					return nil, err
				}
				if req.URL.Path == "/v1/ugoira/metadata" {
					payload = strings.ReplaceAll(string(ugoiraCLIMetadata("")), "/42_", fmt.Sprintf("/%d_", id))
				} else if req.URL.Path == "/v1/illust/detail" {
					payload = string(recordWorkflowArtwork(t, id))
				} else {
					t.Fatalf("unexpected metadata route %s", req.URL)
				}
				header.Set("Content-Type", "application/json")
				if !disableFailure && id == row.FailureID {
					variantRate := false
					if row.Failure == "variant-rate" && req.URL.Path == "/v1/illust/detail" && accountID == row.FailureAccount {
						key := fmt.Sprintf("variant:%d:%d", accountID, id)
						counts[key]++
						variantRate = counts[key] >= 3
					}
					if row.Failure == "metadata-status" && req.URL.Path == "/v1/illust/detail" {
						status = 404
					}
					if row.Failure == "ugoira-status" && req.URL.Path == "/v1/ugoira/metadata" && accountID == row.FailureAccount {
						status = 404
					}
					if (row.Failure == "metadata-rate" && req.URL.Path == "/v1/illust/detail" || row.Failure == "ugoira-rate" && req.URL.Path == "/v1/ugoira/metadata") && accountID == row.FailureAccount || row.Failure == "all-rate" || variantRate {
						status = 429
						key := fmt.Sprintf("%d:%s:%d", accountID, req.URL.Path, id)
						counts[key]++
						if counts[key]%2 == 1 {
							header.Set("Retry-After", "0")
						} else {
							header.Set("Retry-After", "120")
						}
					}
				}
			} else if req.URL.Host == "i.pximg.net" {
				isUgoira := strings.Contains(req.URL.Path, "ugoira")
				id := int64(42)
				if isUgoira {
					fmt.Sscanf(filepath.Base(req.URL.Path), "%d_original.zip", &id)
					key := "ugoira_valid"
					if !disableFailure && row.Failure == "invalid-image" && id == row.FailureID && accountID == row.FailureAccount {
						key = "ugoira_invalid-image"
					}
					decoded, err := hex.DecodeString(assets[key])
					if err != nil {
						t.Fatal(err)
					}
					payload = string(decoded)
					header.Set("Content-Type", "application/zip")
				} else {
					fmt.Sscanf(filepath.Base(req.URL.Path), "%d_p", &id)
					payload = fmt.Sprintf("fixture image %d %s", id, filepath.Base(req.URL.Path))
				}
				if !disableFailure && id == row.FailureID {
					if row.Failure == "page-rate" && strings.Contains(req.URL.Path, "_p1.") {
						status = 429
						header.Set("Retry-After", "120")
					}
					if row.Failure == "body-cancel" || row.Failure == "page-cancel" && strings.Contains(req.URL.Path, "_p1.") {
						payload = payload[:len(payload)/2]
						bodyFailure = "cancel"
					}
					if row.Failure == "body-read" && strings.Contains(req.URL.Path, "_p1.") {
						payload = payload[:len(payload)/2]
						bodyFailure = "read"
					}
				}
			} else {
				t.Fatalf("unexpected fixture host %s", req.URL.Host)
			}
		}
		row.Requests = append(row.Requests, ugoiraWorkflowCLIRequest{accountID, req.Method, req.URL.String(), req.Header.Get("Referer"), req.Header.Get("Cookie"), req.Header.Get("Authorization")})
		return &http.Response{StatusCode: status, Header: header, Body: &recordWorkflowBody{strings.NewReader(payload), cancel, bodyFailure, &row.BodyCloses}, Request: req}, nil
	})
	gate := pool.NewGate()
	selected := make(chan struct{}, 1)
	state := recordWorkflowState{db: db, selected: selected}
	facade := pixivapp.New(pixivapp.Dependencies{Accounts: recordWorkflowAccounts{account.NewService(db, nil), &active}, Gate: recordWorkflowGate{gate, row}, LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
		return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
	}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
		return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: state, Now: time.Now}, nil
	}, CloseClient: func(client *pixiv.Client) error { row.Closes++; active--; client.CloseIdleConnections(); return nil }})
	request := pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}
	var stdout, stderr bytes.Buffer
	writer := &recordWorkflowWriter{buffer: &stderr, row: row, active: &active, normalize: normalize, persistent: row.WriterFailure == "persistent"}
	if row.WriterFailure == "once" {
		writer.failures = 1
	}
	reader := &recordWorkflowReader{body: []byte(row.Input), failure: row.ReaderFailure, returned: &row.ReaderReturned, cancel: cancel}
	execute := func(commandCtx context.Context, input io.Reader, args []string, out io.Writer, errOut io.Writer) error {
		command := download.New(download.Deps{Input: input, Output: out, ErrorOutput: errOut, UsageError: func(err error) error { row.Events = append(row.Events, "usage"); return err }, JSONOut: func(override *bool) (bool, error) {
			row.Events = append(row.Events, "json-mode")
			if override != nil {
				return *override, nil
			}
			return row.RuntimeJSON, nil
		}, Runtime: func() (download.Runtime, error) {
			row.Events = append(row.Events, "runtime")
			return download.Runtime{DownloadPath: filepath.Join(root, "runtime"), FilenameTemplate: row.Filename, DirectoryTemplate: row.Directory}, nil
		}, Download: func() downloader.DownloadService {
			row.Events = append(row.Events, "service")
			return downloader.DownloadService{NewManager: func(client downloader.DownloadClient, path, template string) (downloader.DownloadManager, error) {
				return downloader.NewManager(ugoiraWorkflowCLIClient{client, &row.Saves, normalize}, path, template), nil
			}}
		}, Pooled: func(ctx context.Context, req download.CommandRequest, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			ordinal := len(row.Pooled) + 1
			row.Events = append(row.Events, "pool")
			row.Pooled = append(row.Pooled, recordWorkflowPooled{Ordinal: ordinal, Proxy: req.HTTPSProxyOverride})
			err := facade.Use(ctx, request, func(ctx context.Context, client *pixiv.Client) (bool, error) {
				committed, err := invoke(ctx, client)
				attempt := recordWorkflowAttempt{Ordinal: ordinal, Committed: committed, Cause: ugoiraWorkflowCLICause(err)}
				if err != nil {
					attempt.Error = normalize(err.Error())
				}
				row.PrivateAttempts = append(row.PrivateAttempts, attempt)
				return committed, err
			})
			row.Pooled[ordinal-1].Cause = ugoiraWorkflowCLICause(err)
			if err != nil {
				row.Pooled[ordinal-1].Error = normalize(err.Error())
			}
			if !disableFailure && row.Failure == "after-first" && ordinal == 1 {
				cancel()
			}
			return err
		}})
		cmd := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		cmd.AddCommand(command)
		cmd.SetOut(out)
		cmd.SetErr(errOut)
		cmd.SetContext(commandCtx)
		cmd.SetArgs(append([]string{"download"}, args...))
		defer pipeline.Clear(cmd)
		return cmd.Execute()
	}
	var resultErr error
	if row.Failure == "before-cancel" {
		cancel()
	}
	if row.Failure == "gate-cancel" {
		lease, err := facade.Open(context.Background(), pixivapp.Request{UserID: 42, Options: request.Options})
		if err != nil {
			t.Fatal(err)
		}
		row.Events = append(row.Events, "holder-open")
		finished := make(chan error, 1)
		go func() { finished <- execute(ctx, reader, row.Args, &stdout, writer) }()
		select {
		case <-selected:
		case <-time.After(5 * time.Second):
			t.Fatal("record did not reach real scheduler while gate held")
		}
		cancel()
		select {
		case resultErr = <-finished:
		case <-time.After(5 * time.Second):
			t.Fatal("gate wait did not cancel")
		}
		row.HeldGate = &recordWorkflowHeldGate{Active: active, Requests: len(row.Requests), Saves: len(row.Saves), Attempts: len(row.PrivateAttempts)}
		if active != 1 || len(row.Requests) != 1 || len(row.PrivateAttempts) != 0 || len(row.Saves) != 0 {
			t.Fatal("held lease did not serialize the canceled record operation")
		}
		if err := lease.Close(); err != nil {
			t.Fatal(err)
		}
		row.Events = append(row.Events, "holder-close")
	} else {
		resultErr = execute(ctx, reader, row.Args, &stdout, writer)
	}
	if resultErr != nil {
		row.Error = normalize(resultErr.Error())
		row.Cause = ugoiraWorkflowCLICause(resultErr)
	}
	var pipelineErr *pipeline.PipelineDiagnosticError
	row.Pipeline = errors.As(resultErr, &pipelineErr)
	row.Stdout = normalize(stdout.String())
	row.Stderr = normalize(stderr.String())
	row.ActiveAfter = active
	row.MainPooled = len(row.Pooled)
	row.MainRequests = len(row.Requests)
	row.MainSaves = len(row.Saves)
	recordWorkflowFiles(t, root, row.Files, &row.Directories)
	if active != 0 {
		t.Fatalf("%s leaked active lease", row.Name)
	}
	if row.Followup {
		disableFailure = true
		var out, errOut bytes.Buffer
		followErr := execute(context.Background(), strings.NewReader(recordWorkflowInput("artwork", 45)), []string{}, &out, &errOut)
		after := &recordWorkflowAfter{Cause: ugoiraWorkflowCLICause(followErr), Stdout: normalize(out.String()), Stderr: normalize(errOut.String()), Files: map[string]string{}, Active: active}
		if followErr != nil {
			after.Error = normalize(followErr.Error())
		}
		var directories []string
		recordWorkflowFiles(t, root, after.Files, &directories)
		for path, body := range row.Files {
			if after.Files[path] != body {
				t.Fatal("subsequent execution altered retained publication")
			}
		}
		if _, ok := after.Files["runtime/45_0.jpg"]; !ok {
			t.Fatal("subsequent execution did not publish its own file")
		}
		row.After = after
		if followErr != nil || active != 0 {
			t.Fatalf("%s subsequent saved Execution was not reusable: %v", row.Name, followErr)
		}
	}
	if row.Failure != "no-accounts" {
		for _, id := range []int64{42, 43} {
			stored, err := db.GetPixiv(context.Background(), id)
			if err != nil {
				t.Fatal(err)
			}
			row.Accounts = append(row.Accounts, ugoiraWorkflowCLIAccount{id, stored.CredentialRevision, stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), stored.PoolLastSelected, string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id)})
		}
	}
	if !strings.HasPrefix(row.Name, "explicit-source") && row.Stdout != "" {
		t.Fatal("record action emitted success stdout")
	}
	if strings.HasPrefix(row.Name, "suppressed-") && (resultErr != nil || len(row.Pooled) != 1 || len(row.Files) != 1 || strings.Contains(strings.Join(row.Events, " "), "json-mode")) {
		t.Fatal("record mode performed normal output-mode resolution")
	}
	if strings.HasPrefix(row.Name, "mixed-repeat-") && (resultErr != nil || len(row.Pooled) != 5 || len(row.PrivateAttempts) != 5) {
		t.Fatal("accepted/repeated records were not separate pooled operations")
	}
	if strings.Contains(row.Name, "partial-prefix-blocks") && (len(row.PrivateAttempts) != 2 || !row.PrivateAttempts[0].Committed || row.PrivateAttempts[0].Cause != "rate_limited") {
		t.Fatal("same-record publication did not block replay")
	}
	if row.Name == "prior-success-later-precommit-rate-replays-current-record" && (resultErr != nil || len(row.PrivateAttempts) != 4 || !row.PrivateAttempts[0].Committed || row.PrivateAttempts[1].Committed || !row.PrivateAttempts[2].Committed || row.PrivateAttempts[1].Ordinal != 2 || row.PrivateAttempts[2].Ordinal != 2) {
		t.Fatal("prior record publication incorrectly blocked current-record replay")
	}
	if strings.Contains(row.Failure, "cancel") || row.Failure == "after-first" && strings.HasSuffix(row.Input, "\n") || row.ReaderFailure == "cancel-classified" {
		if !errors.Is(resultErr, context.Canceled) || row.Stderr != "" {
			t.Fatal("canceled stream produced action diagnostic or lost Context cancellation")
		}
	}
	if row.Cause == "cancel" {
		expected := 1
		if row.Failure == "before-cancel" || row.ReaderFailure == "cancel-classified" {
			expected = 0
		}
		if row.Failure == "body-cancel" {
			expected = 2
		}
		if row.MainPooled != expected {
			t.Fatalf("canceled stream started later pooled action: got %d want %d", row.MainPooled, expected)
		}
	}
	if row.Failure == "page-cancel" && (len(row.Files) != 1 || !row.PrivateAttempts[0].Committed) {
		t.Fatal("second-page cancellation lost first-page commit")
	}
	if row.Name == "warning-fail-once-preserves-typed-rate-and-replays" && (len(row.PrivateAttempts) < 2 || row.PrivateAttempts[0].Cause != "rate_limited" || row.PrivateAttempts[0].Committed) {
		t.Fatal("warning writer masked typed precommit rate error")
	}
	if row.Name == "warning-fail-once-preserves-typed-business-and-continues" && (len(row.PrivateAttempts) != 2 || row.PrivateAttempts[0].Cause != "not_found" || !row.PrivateAttempts[1].Committed || len(row.WriterAttempts) < 2 || !row.WriterAttempts[0].Failed || row.WriterAttempts[1].Failed) {
		t.Fatal("warning writer masked typed business failure or diagnostic continuation")
	}
	if row.WriterFailure == "persistent" && (row.Pipeline || row.Error != "fixture writer failure") {
		t.Fatal("persistent diagnostic writer error was not fatal original error")
	}
	if row.Name == "successful-final-unterminated-record-observes-EOF-before-cancellation" && (resultErr != nil || row.MainPooled != 1 || len(row.Files) != 1) {
		t.Fatal("successful final EOF record invented a later Context check")
	}
	if row.Failure == "refresh-rate" || row.Failure == "refresh-status" {
		if len(row.PrivateAttempts) != 1 || row.PrivateAttempts[0].Ordinal != 2 || !row.PrivateAttempts[0].Committed || row.Accounts[0].Revision != 1 || row.Accounts[0].Rotated || row.Accounts[0].Frozen {
			t.Fatal("failed refresh was replayed, persisted or frozen unexpectedly")
		}
	}
	if row.Failure == "all-rate" && (len(row.PrivateAttempts) != 2 || len(row.Files) != 0 || !row.Accounts[0].Frozen || !row.Accounts[1].Frozen || row.Pooled[1].Error != "pixiv:account_pool: rate_limited: account_pool_all_frozen") {
		t.Fatal("account exhaustion did not preserve exact frozen selection boundary")
	}
	if strings.HasPrefix(row.Name, "explicit-source-") && row.ReaderReturned != "" {
		t.Fatal("explicit source consumed record stdin")
	}
	if row.ReaderFailure == "partial" && row.MainPooled != 0 {
		t.Fatal("incomplete record executed before followup")
	}
}

type recordWorkflowAccounts struct {
	pixivapp.AccountService
	active *int
}

func (a recordWorkflowAccounts) OpenAccountClientWith(ctx context.Context, id int64, options pixiv.Options) (*pixiv.Client, error) {
	client, err := a.AccountService.OpenAccountClientWith(ctx, id, options)
	if err == nil && client != nil {
		*a.active++
	}
	return client, err
}
func (a recordWorkflowAccounts) OpenClientWith(ctx context.Context, options pixiv.Options) (*pixiv.Client, error) {
	client, err := a.AccountService.OpenClientWith(ctx, options)
	if err == nil && client != nil {
		*a.active++
	}
	return client, err
}

type recordWorkflowState struct {
	db       *database.DB
	selected chan struct{}
}

func (s recordWorkflowState) SelectPixiv(ctx context.Context, now int64, exclude []int64, chooser account.Chooser) (account.Account, error) {
	result, err := s.db.SelectPixiv(ctx, now, exclude, chooser)
	if err == nil {
		select {
		case s.selected <- struct{}{}:
		default:
		}
	}
	return result, err
}
func (s recordWorkflowState) Freeze(ctx context.Context, id, until int64) error {
	return s.db.Freeze(ctx, id, until)
}
func recordWorkflowArtwork(t *testing.T, id int64) []byte {
	t.Helper()
	pages := 1
	kind := "illust"
	if id == 43 {
		pages = 2
		kind = "manga"
	}
	if id == 44 {
		kind = "ugoira"
	}
	original := fmt.Sprintf("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/%d_p0.jpg", id)
	artwork := map[string]any{"id": id, "type": kind, "title": "Title: slash/", "user": map[string]any{"id": 7, "name": "Author?"}, "create_date": "2026-01-02T03:04:05+00:00", "page_count": pages, "tags": []map[string]string{{"name": "first"}}, "image_urls": map[string]string{"large": original}, "meta_single_page": map[string]string{"original_image_url": original}}
	if pages > 1 {
		all := []map[string]any{}
		for page := 0; page < pages; page++ {
			all = append(all, map[string]any{"image_urls": map[string]string{"original": fmt.Sprintf("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/%d_p%d.jpg", id, page)}})
		}
		artwork["meta_pages"] = all
	}
	data, err := json.Marshal(map[string]any{"illust": artwork})
	if err != nil {
		t.Fatal(err)
	}
	return data
}
func recordWorkflowFiles(t *testing.T, root string, files map[string]string, directories *[]string) {
	t.Helper()
	if err := filepath.WalkDir(root, func(p string, e os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if p == root {
			return nil
		}
		rel, err := filepath.Rel(root, p)
		if err != nil {
			return err
		}
		if e.IsDir() {
			*directories = append(*directories, filepath.ToSlash(rel))
			return nil
		}
		if strings.HasPrefix(e.Name(), ".atomic-write-") || strings.HasPrefix(e.Name(), "ugoira-") || strings.HasPrefix(e.Name(), ".ugoira-") {
			t.Fatalf("owned temp artifact leaked: %s", rel)
		}
		body, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		files[filepath.ToSlash(rel)] = hex.EncodeToString(body)
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}
