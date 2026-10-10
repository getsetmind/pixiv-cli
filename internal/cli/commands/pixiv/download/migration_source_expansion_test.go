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
	"net/url"
	"os"
	"path/filepath"
	"runtime"
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
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateSourceExpansionCLI = flag.Bool("migration-update-source-expansion-cli", false, "capture pinned Go saved user/bookmark download contracts")

type sourceExpansionCLIFixture struct {
	GoWorkerCount  int                      `json:"go_worker_count"`
	Reference      string                   `json:"reference"`
	SourceSHA256   map[string]string        `json:"source_sha256"`
	Normalization  string                   `json:"normalization"`
	Correspondence []string                 `json:"correspondence"`
	Remaining      []string                 `json:"remaining"`
	Assets         map[string]string        `json:"assets_hex"`
	Cases          []sourceExpansionCLICase `json:"cases"`
}
type sourceExpansionCLIResponse struct {
	Status      int    `json:"status"`
	ContentType string `json:"content_type"`
	RetryAfter  string `json:"retry_after"`
	BodyHex     string `json:"body_hex"`
	BodyFailure string `json:"body_failure"`
}
type sourceExpansionCLICase struct {
	ParentCanceled   bool                         `json:"parent_canceled"`
	Name             string                       `json:"name"`
	Args             []string                     `json:"args"`
	Input            string                       `json:"input"`
	RuntimeJSON      bool                         `json:"runtime_json"`
	Filename         string                       `json:"runtime_filename"`
	Directory        string                       `json:"runtime_directory"`
	Failure          string                       `json:"failure"`
	Followup         bool                         `json:"followup"`
	Stdout           string                       `json:"stdout"`
	Stderr           string                       `json:"stderr"`
	Error            string                       `json:"error"`
	Cause            string                       `json:"cause"`
	Pipeline         bool                         `json:"pipeline"`
	Events           []string                     `json:"go_port_events"`
	Pooled           []recordWorkflowPooled       `json:"go_pooled_calls"`
	Attempts         []recordWorkflowAttempt      `json:"go_private_attempts"`
	GateEvents       []string                     `json:"go_gate_port_events"`
	Requests         []ugoiraWorkflowCLIRequest   `json:"requests"`
	Responses        []sourceExpansionCLIResponse `json:"responses"`
	Saves            []ugoiraWorkflowCLISave      `json:"saves"`
	Accounts         []ugoiraWorkflowCLIAccount   `json:"accounts"`
	Closes           int                          `json:"go_client_closes"`
	BodyCloses       int                          `json:"go_body_closes"`
	ActiveAfter      int                          `json:"go_active_after"`
	WriterUnderLease bool                         `json:"go_writer_under_lease"`
	MainRequests     int                          `json:"main_requests"`
	MainSaves        int                          `json:"main_saves"`
	Files            map[string]string            `json:"files"`
	Directories      []string                     `json:"directories"`
	After            *recordWorkflowAfter         `json:"after"`
}

type sourceExpansionCLIBody struct {
	reader  *strings.Reader
	failure string
	cancel  context.CancelFunc
	closes  *int
}

func (b *sourceExpansionCLIBody) Read(p []byte) (int, error) {
	if b.reader.Len() > 0 {
		return b.reader.Read(p)
	}
	switch b.failure {
	case "cancel":
		b.cancel()
		return 0, context.Canceled
	case "client-cancel":
		return 0, context.Canceled
	case "cancel-after":
		b.cancel()
		return 0, io.EOF
	}
	return 0, io.EOF
}
func (b *sourceExpansionCLIBody) Close() error { *b.closes++; return nil }

type sourceExpansionCLIGate struct {
	gate   *pool.Gate
	events *[]string
}

func (g sourceExpansionCLIGate) Acquire(ctx context.Context) error {
	*g.events = append(*g.events, "acquire")
	err := g.gate.Acquire(ctx)
	result := "acquired"
	if err != nil {
		result = err.Error()
	}
	*g.events = append(*g.events, result)
	return err
}
func (g sourceExpansionCLIGate) Release() {
	g.gate.Release()
	*g.events = append(*g.events, "released")
}

func TestMigrationSourceExpansionMatchesFrozenCLIContracts(t *testing.T) {
	previousWorkers := runtime.GOMAXPROCS(1)
	defer runtime.GOMAXPROCS(previousWorkers)
	rootRepo := filepath.Join("..", "..", "..", "..", "..")
	fixture := sourceExpansionCLIFixture{
		GoWorkerCount: 1, Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA256: map[string]string{},
		Normalization:  "The supplied Go runtime uses GOMAXPROCS(1), so real Manager media runs through one actual worker without order normalization. Only the isolated temporary download root becomes $ROOT and generated owned ugoira archive basenames become ugoira-$TEMP.zip. Exact output, SDK HTTP requests and scripted response bytes, save refs, file bytes and persisted accounts are retained. Shared static/manga/ugoira assets and observers reuse existing migration helpers. Responses record the finite owned transport used by genuine SDK operations; they are not manager result replacements.",
		Correspondence: []string{"actual download Cobra controller -> Rust DownloadCommand; actual saved facade/account/SQLite/pool -> saved Execution; real SDK list/detail/resource reads -> native SDK download adapter and media manager", "go_port_events, go_pooled_calls, go_private_attempts, go_gate_port_events, go_client_closes, go_body_closes, go_active_after and go_writer_under_lease are private Go port/callback/lease observers, not required Rust public hooks or source API contracts", "main_requests and main_saves split the original command from a fresh operation using the same facade/SQLite/gate owner after cancellation; responses are aligned with all requests"},
		Remaining:      []string{"full raw list DTO/cursor grammar, discovery-order manager boundary and malformed producer IDs belong to the shared source-expansion fixture; parallel scheduling beyond the supplied one-worker runtime remains separate evidence", "native root startup/exit/envelope, public SDK injected/owned client closure, arbitrary blocking reads and native non-Linux media behavior are separate evidence; no real accounts/network/browser/OS effects", "no persisted resume/job state or new progress emission exists; retries here repeat whole source expansion"},
		Assets:         map[string]string{"ugoira_valid": hex.EncodeToString(generatedUgoiraCLIArchive(t, ""))}, Cases: []sourceExpansionCLICase{},
	}
	pinned := map[string]string{
		"internal/cli/commands/pixiv/download/execution.go":             "236460386b2847dc03a00f10ac1e955ad1fca9f18c173ae0a1523f960812fdf0",
		"internal/cli/commands/pixiv/download/report.go":                "7d75a279defc954df3ab326af750fa1ca51021942a2bc4a205c34b80422b0b63",
		"internal/cli/commands/pixiv/download/request.go":               "60ca907de718599c7bd1a9f098904d01a1a070e3e6aae494a12cf11d08d4666b",
		"internal/media/downloader/downloader.go":                       "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"internal/shared/pagination/pagination.go":                      "0dfb923e82eb59991f7958b68d547e67efd44b440da1085e7e8191cc8bffe952",
		"internal/media/downloader/parallel/parallel.go":                "80ec437492c2655164df91ac8ff1d7a0a20c7df14f1c9feb083b74fc1b5e571d",
		"internal/cli/root.go":                                          "afbb8c2d7a90ccbe3ce124e9e2ee68ec463b9d020ba161d0d1cb270d1e1e7b1e",
		"internal/services/pixiv/endpoint/artwork/timeline/timeline.go": "ae2f702af7e44ab6548772986127522e036f1347929d8bb60821c45e780ce557",
		"internal/services/pixiv/endpoint/artwork/bookmark/bookmark.go": "8ad7e3837413ce05fbfed524f8665e4da157b178253c4c5a681fd47dc73e7725",
		"internal/services/pixiv/appapi/appapi.go":                      "b5d9502ad9c534c88bda076740553c8bb6f389c3d2f4e3bd80c32c7f281ba9ff",
		"sdk/pixiv/reference.go":                                        "7d467e3ae306fbd3d920f86330d80e1c6bd64787db6e56869fe76e77be468abc",
		"sdk/pixiv/ops_artwork.go":                                      "f8aa00684b84463c6ba82b18db87d4c2e403a0dcaa048f3c3282f6555fd7445c",
		"sdk/pixiv/map_artwork.go":                                      "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
		"sdk/pixiv/resource.go":                                         "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
		"internal/services/pixiv/facade.go":                             "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
		"internal/services/pixiv/account/accounts.go":                   "129d83a89a09bc8a4dbd926ce19e9551fedf048a4c1fb9f71d551ee3089fad91",
		"internal/services/pixiv/pool/pool.go":                          "be7f58dba0e1957268551bbf01bd73a9edc8365a6ec57751d2668960bda681d4",
		"internal/services/pixiv/pool/gate.go":                          "33387af051432f6a6e6a8d0a8bbbcc88a5035ef6a65cbd30846f3ea9a70990a6",
		"internal/services/pixiv/pool/replay.go":                        "54559f3852e1070f6a6acfbf34b217a2ccfd139cd8d6e60157880c0accd6a282",
		"internal/storage/database/repository.go":                       "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		"internal/storage/database/pool.go":                             "40c8a456110e5cb1b4ac88e6c49277f97696233d7a0237ada480ef85e0145ed0",
	}
	for path, want := range pinned {
		data, err := os.ReadFile(filepath.Join(rootRepo, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(data)
		got := hex.EncodeToString(sum[:])
		if got != want {
			t.Fatalf("frozen source changed: %s", path)
		}
		fixture.SourceSHA256[path] = got
	}
	home := "https://www.pixiv.net/users/7"
	bookmark := "https://www.pixiv.net/users/7/bookmarks/artworks?rest=hide&restrict=private&tag=private#ignored"
	opaque, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"ugoira_archive","id":44,"p":-1,"v":"original"}`))
	if err != nil {
		t.Fatal(err)
	}
	add := func(name string, args ...string) *sourceExpansionCLICase {
		fixture.Cases = append(fixture.Cases, sourceExpansionCLICase{Name: name, Args: args, Filename: "{id}_{num}"})
		return &fixture.Cases[len(fixture.Cases)-1]
	}
	for _, mode := range []string{"human", "json", "ndjson", "runtime-json"} {
		args := []string{home, "--ugoira-mode=zip"}
		if mode == "json" || mode == "ndjson" {
			args = append(args, "--"+mode)
		}
		row := add("every-kind-all-pages-"+mode, args...)
		row.RuntimeJSON = mode == "runtime-json"
	}
	for _, mode := range []string{"gif", "apng", "raw"} {
		add("mixed-media-"+mode, home, "--ugoira-mode="+mode, "--ndjson")
	}
	add("canonical-repeated-user-and-global-ID-dedup", "+45", "https://pixiv.net/en/users/7/artworks?tag=ignored#x", home, bookmark, "42", "https://www.pixiv.net/artworks/43", "--ugoira-mode=zip", "--ndjson")
	add("bookmarks-always-public-empty-tag", bookmark, "--ugoira-mode=zip", "--json")
	row := add("raw-stdin-user-URL", "--ugoira-mode=zip", "--ndjson")
	row.Input = home + "\r\n"
	row = add("prepared-quality-page-and-runtime-templates", home, "--pages=2", "--quality=regular", "--filename-template={author_id}_{id}_{num}", "--no-proxy", "--ugoira-mode=zip", "--json")
	row.Directory = "{date}/{id}"
	row = add("prepared-output-root-alias", home, "--output=$ROOT/override", "--ugoira-mode=zip", "--json")
	row = add("warning-from-template-after-expanded-media", home, "--filename-template={unknown}", "--ugoira-mode=zip", "--ndjson")
	row = add("explicit-json-false", home, "--json=false", "--ugoira-mode=zip")
	row.RuntimeJSON = true
	add("both-machine-flags-stop-before-pool", home, "--json", "--ndjson")
	for _, mode := range []string{"human", "json", "ndjson", "json-fail-fast", "ndjson-fail-fast"} {
		args := []string{home, "--ugoira-mode=zip"}
		if strings.HasPrefix(mode, "json") {
			args = append(args, "--json")
		}
		if strings.HasPrefix(mode, "ndjson") {
			args = append(args, "--ndjson")
		}
		if strings.HasSuffix(mode, "fail-fast") {
			args = append(args, "--on-error=fail-fast")
		}
		row = add("later-user-page-retains-prefix-and-later-kinds-"+mode, args...)
		row.Failure = "later-user-status"
	}
	for _, name := range []string{"first-user-status", "first-nonrate-masks-later-rate", "pure-list-rate", "all-list-rate", "postcommit-list-rate", "postcommit-media-rate", "postcommit-resource-429", "bookmark-first-status", "bookmark-later-status", "bookmark-rate", "user-failure-before-fatal-bookmark", "client-cancel-with-live-parent"} {
		args := []string{home, "--ugoira-mode=zip"}
		if strings.HasPrefix(name, "bookmark-") || name == "user-failure-before-fatal-bookmark" {
			args = []string{"45", home, opaque.String(), "https://i.pximg.net/owned/direct.jpg", bookmark, "--ugoira-mode=zip", "--json"}
		}
		if name == "bookmark-rate" {
			args = args[:len(args)-1]
		}
		row = add(name, args...)
		row.Failure = name
	}
	for _, mode := range []string{"json", "ndjson", "json-fail-fast"} {
		args := []string{home, "--ugoira-mode=zip", "--" + strings.TrimSuffix(mode, "-fail-fast")}
		if strings.HasSuffix(mode, "fail-fast") {
			args = append(args, "--on-error=fail-fast")
		}
		row = add("pure-list-rate-"+mode, args...)
		row.Failure = "pure-list-rate"
	}
	for _, name := range []string{"before-cancel", "list-body-cancel", "later-list-body-cancel", "list-page-completes-then-cancel", "bookmark-list-body-cancel", "bookmark-later-body-cancel"} {
		args := []string{home, "--ugoira-mode=zip", "--json"}
		if strings.HasPrefix(name, "bookmark-") {
			args = []string{"45", home, bookmark, "--ugoira-mode=zip", "--json"}
		}
		row = add(name+"-and-reuse", args...)
		row.Failure = name
		row.Followup = true
	}
	for i := range fixture.Cases {
		t.Run(fixture.Cases[i].Name, func(t *testing.T) { runSourceExpansionCLICase(t, &fixture.Cases[i], fixture.Assets) })
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(rootRepo, "crates", "pixiv-cli", "tests", "fixtures", "download_source_expansion.json")
	if *updateSourceExpansionCLI {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d saved source expansion cases", len(fixture.Cases))
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("saved source expansion differs from frozen Go")
	}
}

func sourceExpansionCLIPage(t *testing.T, kind string, later bool, empty bool) string {
	t.Helper()
	ids := []int64{}
	var next any
	if kind == "bookmarks" {
		if !later {
			ids = []int64{43, 42}
			next = "https://app-api.pixiv.net/v1/user/bookmarks/illust?user_id=7&restrict=public&max_bookmark_id=30"
		} else {
			ids = []int64{44, 45}
		}
	} else {
		switch kind {
		case "illust":
			if !later {
				ids = []int64{45, 42}
			} else {
				ids = []int64{42}
			}
		case "manga":
			if later {
				ids = []int64{43}
			}
		case "ugoira":
			ids = []int64{44}
		}
		if !later {
			next = "https://app-api.pixiv.net/v1/user/illusts?user_id=7&type=" + kind + "&offset=30"
		}
	}
	if empty {
		ids = []int64{}
	}
	items := []map[string]any{}
	for _, id := range ids {
		items = append(items, map[string]any{"id": id, "type": "unrecognized", "title": "list metadata must not replace detail", "user": map[string]any{"id": 7}, "create_date": "2026-01-02T03:04:05Z", "page_count": 99})
	}
	data, err := json.Marshal(map[string]any{"illusts": items, "next_url": next})
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func runSourceExpansionCLICase(t *testing.T, row *sourceExpansionCLICase, assets map[string]string) {
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
	for _, id := range []int64{42, 43} {
		if err := db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
			t.Fatal(err)
		}
	}
	if err := db.SetAllPixivSchedulable(ctx, true); err != nil {
		t.Fatal(err)
	}
	row.Events = []string{}
	row.Pooled = []recordWorkflowPooled{}
	row.Attempts = []recordWorkflowAttempt{}
	row.GateEvents = []string{}
	row.Requests = []ugoiraWorkflowCLIRequest{}
	row.Responses = []sourceExpansionCLIResponse{}
	row.Saves = []ugoiraWorkflowCLISave{}
	row.Accounts = []ugoiraWorkflowCLIAccount{}
	row.Files = map[string]string{}
	row.Directories = []string{}
	normalize := func(s string) string { return normalizeUgoiraCLIPath(strings.ReplaceAll(s, root, "$ROOT")) }
	var mutex sync.Mutex
	active := 0
	lastID := int64(0)
	disable := false
	counts := map[string]int{}
	transport := directDownloadTransport(func(req *http.Request) (*http.Response, error) {
		mutex.Lock()
		defer mutex.Unlock()
		accountID := lastID
		status := 200
		payload := ""
		contentType := "application/json"
		retryAfter := ""
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
		} else {
			stored, err := db.GetPixiv(context.Background(), accountID)
			if err != nil {
				return nil, err
			}
			if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", accountID) {
				t.Fatal("SDK operation preceded persisted refresh")
			}
			if active != 1 {
				t.Fatalf("request has %d active leases", active)
			}
			if req.URL.Host == "app-api.pixiv.net" && req.Header.Get("Authorization") != fmt.Sprintf("Bearer fixture-access-%d", accountID) {
				t.Fatal("wrong metadata authorization")
			}
			if req.URL.Host == "i.pximg.net" && req.Header.Get("Authorization") != "" {
				t.Fatal("media leaked authorization")
			}
			if req.URL.Host == "app-api.pixiv.net" {
				q := req.URL.Query()
				isList := req.URL.Path == "/v1/user/illusts" || req.URL.Path == "/v1/user/bookmarks/illust"
				if isList {
					kind := q.Get("type")
					later := q.Get("offset") != ""
					if req.URL.Path == "/v1/user/bookmarks/illust" {
						kind = "bookmarks"
						later = q.Get("max_bookmark_id") != ""
						if q.Get("restrict") != "public" || q.Get("tag") != "" {
							t.Fatal("bookmark URL leaked query-derived restrict/tag")
						}
					}
					if q.Get("user_id") != "7" {
						t.Fatal("wrong source user ID")
					}
					empty := !disable && accountID == 42 && (row.Failure == "pure-list-rate" || row.Failure == "all-list-rate" || row.Failure == "first-nonrate-masks-later-rate")
					if !disable && row.Failure == "all-list-rate" {
						empty = true
					}
					payload = sourceExpansionCLIPage(t, kind, later, empty)
					if !disable {
						switch row.Failure {
						case "later-user-status":
							if kind == "illust" && later {
								status = 404
							}
						case "first-user-status":
							if kind == "illust" {
								status = 404
							}
						case "first-nonrate-masks-later-rate":
							if accountID == 42 && kind == "illust" {
								status = 404
							}
							if accountID == 42 && kind == "manga" {
								status = 429
							}
						case "pure-list-rate":
							if accountID == 42 && kind == "illust" && later {
								status = 429
							}
						case "all-list-rate":
							if kind == "illust" && later {
								status = 429
							}
						case "postcommit-list-rate":
							if accountID == 42 && kind == "illust" && later {
								status = 429
							}
						case "bookmark-first-status":
							if kind == "bookmarks" {
								status = 404
							}
						case "bookmark-later-status", "user-failure-before-fatal-bookmark":
							if kind == "bookmarks" && later || row.Failure == "user-failure-before-fatal-bookmark" && kind == "illust" {
								status = 404
							}
						case "bookmark-rate":
							if accountID == 42 && kind == "bookmarks" && later {
								status = 429
							}
						case "list-body-cancel":
							if kind == "illust" && !later {
								payload = payload[:len(payload)/2]
								bodyFailure = "cancel"
							}
						case "later-list-body-cancel":
							if kind == "illust" && later {
								payload = payload[:len(payload)/2]
								bodyFailure = "cancel"
							}
						case "list-page-completes-then-cancel":
							if kind == "illust" && !later {
								bodyFailure = "cancel-after"
							}
						case "bookmark-list-body-cancel":
							if kind == "bookmarks" && !later {
								payload = payload[:len(payload)/2]
								bodyFailure = "cancel"
							}
						case "bookmark-later-body-cancel":
							if kind == "bookmarks" && later {
								payload = payload[:len(payload)/2]
								bodyFailure = "cancel"
							}
						case "client-cancel-with-live-parent":
							if kind == "illust" && later {
								payload = payload[:len(payload)/2]
								bodyFailure = "client-cancel"
							}
						}
					}
				} else {
					id, err := strconv.ParseInt(q.Get("illust_id"), 10, 64)
					if err != nil {
						return nil, err
					}
					if req.URL.Path == "/v1/illust/detail" {
						payload = string(recordWorkflowArtwork(t, id))
					} else if req.URL.Path == "/v1/ugoira/metadata" {
						payload = strings.ReplaceAll(string(ugoiraCLIMetadata("")), "/42_", fmt.Sprintf("/%d_", id))
					} else {
						t.Fatalf("unexpected SDK route %s", req.URL)
					}
				}
				if !disable && row.Failure == "postcommit-media-rate" && accountID == 42 && req.URL.Path == "/v1/ugoira/metadata" {
					status = 429
				}
			} else if req.URL.Host == "i.pximg.net" {
				contentType = "image/jpeg"
				payload = "fixture direct resource"
				if strings.Contains(req.URL.Path, "ugoira") {
					data, err := hex.DecodeString(assets["ugoira_valid"])
					if err != nil {
						t.Fatal(err)
					}
					payload = string(data)
					contentType = "application/zip"
				} else {
					var id int64
					if _, err := fmt.Sscanf(filepath.Base(req.URL.Path), "%d_p", &id); err == nil {
						payload = fmt.Sprintf("fixture image %d %s", id, filepath.Base(req.URL.Path))
					}
				}
				if !disable && row.Failure == "postcommit-resource-429" && accountID == 42 && strings.Contains(req.URL.Path, "43_p1.") {
					status = 429
				}
			} else {
				t.Fatalf("unexpected host %s", req.URL.Host)
			}
		}
		if status == 429 {
			key := fmt.Sprintf("%d:%s", accountID, req.URL.String())
			counts[key]++
			if counts[key]%2 == 1 {
				retryAfter = "0"
			} else {
				retryAfter = "120"
			}
		}
		row.Requests = append(row.Requests, ugoiraWorkflowCLIRequest{accountID, req.Method, req.URL.String(), req.Header.Get("Referer"), req.Header.Get("Cookie"), req.Header.Get("Authorization")})
		row.Responses = append(row.Responses, sourceExpansionCLIResponse{status, contentType, retryAfter, hex.EncodeToString([]byte(payload)), bodyFailure})
		header := http.Header{"Content-Type": {contentType}}
		if retryAfter != "" {
			header.Set("Retry-After", retryAfter)
		}
		return &http.Response{StatusCode: status, Header: header, Body: &sourceExpansionCLIBody{strings.NewReader(payload), bodyFailure, cancel, &row.BodyCloses}, Request: req}, nil
	})
	facade := pixivapp.New(pixivapp.Dependencies{Accounts: recordWorkflowAccounts{account.NewService(db, nil), &active}, Gate: sourceExpansionCLIGate{pool.NewGate(), &row.GateEvents}, LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
		return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
	}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
		return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
	}, CloseClient: func(client *pixiv.Client) error { row.Closes++; active--; client.CloseIdleConnections(); return nil }})
	req := pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}
	execute := func(commandCtx context.Context, args []string, input string, out, errOut io.Writer) error {
		cmd := download.New(download.Deps{Input: strings.NewReader(input), Output: out, ErrorOutput: errOut, UsageError: func(err error) error { row.Events = append(row.Events, "usage"); return err }, JSONOut: func(override *bool) (bool, error) {
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
		}, Pooled: func(ctx context.Context, commandReq download.CommandRequest, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			ordinal := len(row.Pooled) + 1
			row.Events = append(row.Events, "pool")
			row.Pooled = append(row.Pooled, recordWorkflowPooled{Ordinal: ordinal, Proxy: commandReq.HTTPSProxyOverride})
			err := facade.Use(ctx, req, func(ctx context.Context, client *pixiv.Client) (bool, error) {
				committed, err := invoke(ctx, client)
				a := recordWorkflowAttempt{Ordinal: ordinal, Committed: committed, Cause: ugoiraWorkflowCLICause(err)}
				if err != nil {
					a.Error = normalize(err.Error())
				}
				row.Attempts = append(row.Attempts, a)
				return committed, err
			})
			row.Pooled[ordinal-1].Cause = ugoiraWorkflowCLICause(err)
			if err != nil {
				row.Pooled[ordinal-1].Error = normalize(err.Error())
			}
			return err
		}})
		rootCmd := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		rootCmd.AddCommand(cmd)
		rootCmd.SetContext(commandCtx)
		rootCmd.SetOut(out)
		rootCmd.SetErr(errOut)
		actualArgs := append([]string{"download"}, args...)
		for i, arg := range actualArgs {
			actualArgs[i] = strings.ReplaceAll(arg, "$ROOT", root)
		}
		rootCmd.SetArgs(actualArgs)
		defer pipeline.Clear(rootCmd)
		return rootCmd.Execute()
	}
	var stdout, stderr bytes.Buffer
	out := ugoiraWorkflowCLIWriter{buffer: &stdout, active: &active, under: &row.WriterUnderLease}
	errOut := ugoiraWorkflowCLIWriter{buffer: &stderr, active: &active, under: &row.WriterUnderLease}
	if row.Failure == "before-cancel" {
		cancel()
	}
	finished := make(chan error, 1)
	go func() { finished <- execute(ctx, row.Args, row.Input, out, errOut) }()
	var resultErr error
	select {
	case resultErr = <-finished:
	case <-time.After(5 * time.Second):
		t.Fatal("finite saved source expansion did not complete or cancel")
	}
	if resultErr != nil {
		row.Error = normalize(resultErr.Error())
		row.Cause = ugoiraWorkflowCLICause(resultErr)
	}
	var marker *pipeline.PipelineDiagnosticError
	row.Pipeline = errors.As(resultErr, &marker)
	row.Stdout = normalize(stdout.String())
	row.Stderr = normalize(stderr.String())
	row.ParentCanceled = ctx.Err() != nil
	row.ActiveAfter = active
	row.MainRequests = len(row.Requests)
	row.MainSaves = len(row.Saves)
	recordWorkflowFiles(t, root, row.Files, &row.Directories)
	if active != 0 {
		t.Fatal("source expansion leaked saved lease")
	}
	if row.Followup {
		disable = true
		var out, errOut bytes.Buffer
		err := execute(context.Background(), []string{"45"}, "", &out, &errOut)
		after := &recordWorkflowAfter{Cause: ugoiraWorkflowCLICause(err), Stdout: normalize(out.String()), Stderr: normalize(errOut.String()), Files: map[string]string{}, Active: active}
		if err != nil {
			after.Error = normalize(err.Error())
		}
		var dirs []string
		recordWorkflowFiles(t, root, after.Files, &dirs)
		row.After = after
		if err != nil || active != 0 {
			t.Fatalf("subsequent saved owner was not reusable: %v", err)
		}
		if _, ok := after.Files["runtime/45_0.jpg"]; !ok {
			t.Fatal("subsequent saved command did not publish")
		}
	}
	for _, id := range []int64{42, 43} {
		stored, err := db.GetPixiv(context.Background(), id)
		if err != nil {
			t.Fatal(err)
		}
		row.Accounts = append(row.Accounts, ugoiraWorkflowCLIAccount{id, stored.CredentialRevision, stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), stored.PoolLastSelected, string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id)})
	}
	assertSourceExpansionCLICase(t, row, resultErr)
}

func assertSourceExpansionCLICase(t *testing.T, row *sourceExpansionCLICase, err error) {
	t.Helper()
	if strings.Contains(row.Failure, "cancel") && row.Failure != "client-cancel-with-live-parent" {
		if !errors.Is(err, context.Canceled) || row.Stdout != "" || row.MainSaves != 0 || len(row.Files) != 0 {
			t.Fatal("canceled list expansion performed media or lost parent cancellation")
		}
		return
	}
	if row.Name == "both-machine-flags-stop-before-pool" {
		if len(row.Pooled) != 0 || len(row.Requests) != 0 || row.Error != "--json and --ndjson cannot be used together" {
			t.Fatal("conflicting output flags crossed saved-account boundary")
		}
		return
	}
	mainRequests := row.Requests[:row.MainRequests]
	seenMedia := false
	lists := 0
	details := []int64{}
	for _, request := range mainRequests {
		u, parseErr := url.Parse(request.URL)
		if parseErr != nil {
			t.Fatal(parseErr)
		}
		if u.Host == "i.pximg.net" {
			seenMedia = true
		}
		if strings.Contains(u.Path, "/user/") {
			lists++
			if seenMedia {
				t.Fatal("list discovery continued after media acquisition")
			}
		}
		if u.Path == "/v1/illust/detail" {
			id, _ := strconv.ParseInt(u.Query().Get("illust_id"), 10, 64)
			details = append(details, id)
		}
	}
	if strings.HasPrefix(row.Name, "every-kind-all-pages-") || strings.HasPrefix(row.Name, "mixed-media-") {
		if err != nil || lists != 6 || len(row.Files) != 5 || len(row.Attempts) != 1 || !row.Attempts[0].Committed {
			t.Fatalf("all kinds/pages did not reach genuine mixed media: %v lists=%d files=%d", err, lists, len(row.Files))
		}
		if fmt.Sprint(details) != "[42 43 44 45]" {
			t.Fatal("real Manager did not sort/refetch discovered artwork")
		}
	}
	if row.Failure == "later-user-status" {
		if len(row.Attempts) != 1 || !row.Attempts[0].Committed || len(row.Files) != 5 || lists != 6 {
			t.Fatal("later user failure discarded prefix or later kinds")
		}
		if strings.Contains(row.Name, "fail-fast") && row.Stdout != "" {
			t.Fatal("fail-fast emitted partial report")
		}
		if strings.HasSuffix(row.Name, "json") && !row.Pipeline {
			t.Fatal("machine skip lost pipeline marker")
		}
	}
	if row.Failure == "first-nonrate-masks-later-rate" {
		if len(row.Attempts) != 1 || row.Cause != "not_found" || row.Accounts[0].Frozen || row.Accounts[1].Selected || len(row.Files) != 0 {
			t.Fatal("later rate replaced the first non-rate cause")
		}
	}
	if row.Name == "pure-list-rate" {
		if err != nil || len(row.Attempts) != 2 || row.Attempts[0].Committed || row.Attempts[0].Cause != "rate_limited" || !row.Attempts[1].Committed || !row.Accounts[0].Frozen || !row.Accounts[1].Selected {
			t.Fatal("pure precommit rate did not replay whole expansion")
		}
		if lists != 13 {
			t.Fatalf("whole source expansion did not restart: lists=%d", lists)
		}
	}
	if row.Failure == "pure-list-rate" && strings.Contains(row.Name, "fail-fast") {
		if len(row.Attempts) != 2 || err != nil {
			t.Fatal("machine fail-fast precommit rate lost replay")
		}
	}
	if row.Failure == "pure-list-rate" && (row.Name == "pure-list-rate-json" || row.Name == "pure-list-rate-ndjson") {
		if len(row.Attempts) != 1 || !row.Pipeline || row.Accounts[0].Frozen || row.Accounts[1].Selected || len(row.Files) != 0 {
			t.Fatal("machine skip marker incorrectly replayed typed report cause")
		}
	}
	if strings.HasPrefix(row.Failure, "postcommit-") {
		expectedCause := "rate_limited"
		if row.Failure == "postcommit-resource-429" {
			expectedCause = "upstream_error"
		}
		if len(row.Attempts) != 1 || !row.Attempts[0].Committed || row.Cause != expectedCause || row.Accounts[0].Frozen || row.Accounts[1].Selected {
			t.Fatalf("same-attempt media prefix changed: attempts=%+v cause=%s accounts=%+v", row.Attempts, row.Cause, row.Accounts)
		}
	}
	if row.Failure == "bookmark-first-status" || row.Failure == "bookmark-later-status" || row.Failure == "user-failure-before-fatal-bookmark" {
		if row.MainSaves != 0 || len(row.Files) != 0 || row.Stdout != "" || row.Pipeline || len(row.Attempts) != 1 || row.Attempts[0].Committed || row.Cause != "not_found" {
			t.Fatal("fatal bookmark failure downloaded queued IDs/direct media or emitted partial report")
		}
	}
	if row.Failure == "all-list-rate" {
		if len(row.Attempts) != 2 || row.Attempts[0].Committed || row.Attempts[1].Committed || !row.Accounts[0].Frozen || !row.Accounts[1].Frozen || len(row.Files) != 0 || row.Error != "pixiv:account_pool: rate_limited: account_pool_all_frozen" {
			t.Fatal("all account rate exhaustion altered frozen selection/release boundary")
		}
	}
	if row.Failure == "bookmark-rate" {
		if err != nil || len(row.Attempts) != 2 || row.Attempts[0].Committed || !row.Attempts[1].Committed || !row.Accounts[0].Frozen {
			t.Fatal("fatal precommit bookmark rate did not replay complete source expansion")
		}
	}
	if row.Failure == "client-cancel-with-live-parent" {
		if row.ParentCanceled {
			t.Fatal("client-only cancel was promoted to parent cancellation")
		}
		if len(row.Files) != 5 || lists != 6 || len(row.Attempts) != 1 || !row.Attempts[0].Committed {
			t.Fatal("client-only cancellation did not continue later visual kinds")
		}
	}
}
