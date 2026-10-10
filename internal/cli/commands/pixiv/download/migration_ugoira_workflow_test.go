package download_test

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	download "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/download"
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

var updateUgoiraWorkflowCLI = flag.Bool("migration-update-ugoira-cli", false, "capture ugoira CLI contracts from pinned Go")

type ugoiraWorkflowCLIFixture struct {
	Reference     string                  `json:"reference"`
	SourceSHA256  map[string]string       `json:"source_sha256"`
	Normalization string                  `json:"normalization"`
	Remaining     []string                `json:"remaining"`
	Cases         []ugoiraWorkflowCLICase `json:"cases"`
}
type ugoiraWorkflowCLICase struct {
	Name             string                     `json:"name"`
	Args             []string                   `json:"args"`
	Input            string                     `json:"input"`
	Pages            int                        `json:"metadata_pages"`
	Kind             string                     `json:"metadata_kind"`
	RuntimeJSON      bool                       `json:"runtime_json"`
	Filename         string                     `json:"runtime_filename"`
	Directory        string                     `json:"runtime_directory"`
	ArchiveHex       string                     `json:"archive_hex"`
	Mode             string                     `json:"mode"`
	Failure          string                     `json:"failure"`
	Stdout           string                     `json:"stdout"`
	Stderr           string                     `json:"stderr"`
	Error            string                     `json:"error"`
	Cause            string                     `json:"cause"`
	Events           []string                   `json:"events"`
	Attempts         []ugoiraWorkflowCLIAttempt `json:"attempts"`
	Saves            []ugoiraWorkflowCLISave    `json:"saves"`
	Requests         []ugoiraWorkflowCLIRequest `json:"requests"`
	Accounts         []ugoiraWorkflowCLIAccount `json:"accounts"`
	Closes           int                        `json:"closes"`
	WriterUnderLease bool                       `json:"writer_under_lease"`
	Files            map[string]string          `json:"files"`
	Directories      []string                   `json:"directories"`
}
type ugoiraWorkflowCLISave struct {
	Ref         string `json:"ref"`
	Destination string `json:"destination"`
}
type ugoiraWorkflowCLIClient struct {
	downloader.DownloadClient
	saves     *[]ugoiraWorkflowCLISave
	normalize func(string) string
}

func (c ugoiraWorkflowCLIClient) SaveResource(ctx context.Context, ref sdk.ResourceRef, options sdk.SaveOptions) (sdk.SavedResource, error) {
	*c.saves = append(*c.saves, ugoiraWorkflowCLISave{ref.String(), normalizeUgoiraCLIPath(c.normalize(options.Path))})
	return c.DownloadClient.SaveResource(ctx, ref, options)
}

type ugoiraWorkflowCLIAttempt struct {
	Committed bool   `json:"committed"`
	Error     string `json:"error"`
	Cause     string `json:"cause"`
}
type ugoiraWorkflowCLIRequest struct {
	Account       int64  `json:"account"`
	Method        string `json:"method"`
	URL           string `json:"url"`
	Referer       string `json:"referer"`
	Cookie        string `json:"cookie"`
	Authorization string `json:"authorization"`
}
type ugoiraWorkflowCLIAccount struct {
	ID       int64 `json:"id"`
	Revision int64 `json:"revision"`
	Frozen   bool  `json:"frozen"`
	Selected bool  `json:"selected"`
	Rotated  bool  `json:"rotated"`
}
type ugoiraWorkflowCLIWriter struct {
	buffer *bytes.Buffer
	active *int
	under  *bool
	fail   bool
}

func (w ugoiraWorkflowCLIWriter) Write(b []byte) (int, error) {
	if *w.active > 0 {
		*w.under = true
	}
	if w.fail {
		return 0, errors.New("fixture writer failure")
	}
	return w.buffer.Write(b)
}
func ugoiraWorkflowCLICause(err error) string {
	if errors.Is(err, context.Canceled) {
		return "cancel"
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return "deadline"
	}
	var classified *sdk.Error
	if errors.As(err, &classified) {
		return string(classified.Reason)
	}
	return ""
}

func TestMigrationUgoiraWorkflowMatchesFrozenCLIContracts(t *testing.T) {
	rootRepo := filepath.Join("..", "..", "..", "..", "..")
	fixture := ugoiraWorkflowCLIFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA256: map[string]string{}, Normalization: "Only the isolated temporary download root is replaced by $ROOT; temporary ugoira archive basenames become ugoira-$TEMP.zip in recorded SaveResource destinations. File bytes are hex encoded. Request order, opaque SDK-derived resource URLs, array order, text, bytes and account state remain exact. Root source hashes are read-only wiring evidence; this harness executes a Cobra root plus the actual download leaf and saved-account facade, not native startup hooks.", Remaining: []string{"native non-Linux encoder targets, external accounts/network and native startup hooks remain unexecuted", "complete archive safety/frame/IO/native cancellation matrices are separate shared-manager fixtures; SDK rejects unsafe or duplicate declared frames before manager inspection"}, Cases: []ugoiraWorkflowCLICase{}}
	pinnedSources := map[string]string{
		"internal/media/ugoira/rust/staticlib/manifest.json":                           "3624075e2e7b80457ec453c5e7468fdb1cc6efbf4840e4350422ffefd067c80b",
		"internal/media/ugoira/rust/staticlib/x86_64-unknown-linux-gnu/libugoira_rs.a": "b29ec70678bb7fb7401c63996940c8ff60c94802cdd891ade100641a1dabb784",
		"internal/media/ugoira/ugoira.go":                                              "44fdf6867e56bc9beaa6f8be703d259cf92f55856f3329741b6ec3b8f49f3494",
		"internal/media/downloader/ugoira_archive.go":                                  "8e2c3586290a2745a774705586caf6f6d06a3add04b7161d19285cd127a61c34",
		"internal/media/ugoira/rust.go":                                                "c5bec9ab428207fa279b305ba80c215e90f92d3cf3513a47270fd5199d906e3a",
		"sdk/pixiv/map_artwork.go":                                                     "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
		"internal/media/downloader/downloader.go":                                      "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"sdk/pixiv/resource.go":                                                        "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
		"internal/cli/commands/pixiv/download/execution.go":                            "236460386b2847dc03a00f10ac1e955ad1fca9f18c173ae0a1523f960812fdf0",
		"internal/cli/commands/pixiv/download/report.go":                               "7d75a279defc954df3ab326af750fa1ca51021942a2bc4a205c34b80422b0b63",
		"internal/cli/root.go":                                                         "afbb8c2d7a90ccbe3ce124e9e2ee68ec463b9d020ba161d0d1cb270d1e1e7b1e",
		"internal/services/pixiv/facade.go":                                            "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
		"internal/services/pixiv/pool/pool.go":                                         "be7f58dba0e1957268551bbf01bd73a9edc8365a6ec57751d2668960bda681d4",
	}
	for name, expected := range pinnedSources {
		source, err := os.ReadFile(filepath.Join(rootRepo, filepath.FromSlash(name)))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(source)
		actual := hex.EncodeToString(digest[:])
		if actual != expected {
			t.Fatalf("reference source changed: %s", name)
		}
		fixture.SourceSHA256[name] = actual
	}

	add := func(name string, args ...string) int {
		fixture.Cases = append(fixture.Cases, ugoiraWorkflowCLICase{Name: name, Args: args, Pages: 1, Kind: "ugoira", Filename: "{author} - {title}_{id}"})
		return len(fixture.Cases) - 1
	}

	for _, mode := range []string{"gif", "apng", "zip", "raw"} {
		for _, output := range []string{"human", "json", "ndjson"} {
			args := []string{"42", "--ugoira-mode=" + mode}
			if output != "human" {
				args = append(args, "--"+output)
			}
			i := add(mode+"-"+output, args...)
			fixture.Cases[i].Mode = mode
		}
		i := add(mode+"-fallback-template", "42", "--ugoira-mode="+mode, "--filename-template={unknown}", "--json")
		fixture.Cases[i].Mode = mode
	}
	for _, failure := range []string{"missing", "duplicate", "unsafe", "corrupt", "undeclared", "medium-only", "no-archive", "declared-duplicate", "declared-unsafe", "archive-rate", "read", "cancel", "invalid-image"} {
		for _, mode := range []string{"zip", "gif"} {
			i := add(mode+"-"+failure, "42", "--ugoira-mode="+mode, "--json")
			fixture.Cases[i].Failure = failure
			fixture.Cases[i].Mode = mode
		}
	}
	for _, mode := range []string{"json", "ndjson", "fail-fast", "human"} {
		args := []string{"42", "https://i.pximg.net/assets/direct.png", "--ugoira-mode=zip"}
		if mode == "json" || mode == "ndjson" {
			args = append(args, "--"+mode)
		}
		if mode == "fail-fast" {
			args = append(args, "--json", "--on-error=fail-fast")
		}
		i := add("prefix-direct-rate-no-replay-"+mode, args...)
		fixture.Cases[i].Failure = "direct-rate"
		fixture.Cases[i].Mode = "zip"
	}
	add("quality-not-original", "42", "--quality=regular", "--ugoira-mode=zip", "--json")
	add("pages-not-supported", "42", "--pages=1", "--ugoira-mode=zip", "--json")
	add("invalid-mode-before-service", "42", "--ugoira-mode=webm")
	add("invalid-pages-before-service", "42", "--pages=bad")
	add("invalid-quality-before-service", "42", "--quality=bad")
	add("output-conflict-before-pool", "42", "--json", "--ndjson")
	i := add("runtime-template-directory", "42", "--ugoira-mode=zip", "--json")
	fixture.Cases[i].Directory = "{author}/{date}/{id}"
	fixture.Cases[i].Filename = "{id}_{num}"
	fixture.Cases[i].Mode = "zip"
	i = add("invalid-directory", "42", "--ugoira-mode=zip", "--json")
	fixture.Cases[i].Directory = "../escape"
	fixture.Cases[i].Mode = "zip"
	i = add("pipeline-ugoira", "--ugoira-mode=zip")
	fixture.Cases[i].Input = "{\"type\":\"ugoira\",\"id\":42,\"url\":\"https://www.pixiv.net/artworks/42\"}\n"
	fixture.Cases[i].Mode = "zip"
	i = add("writer-failure-after-commit", "42", "--ugoira-mode=zip", "--json")
	fixture.Cases[i].Failure = "writer"
	fixture.Cases[i].Mode = "zip"
	i = add("operation-cancel-before-pool", "42", "--ugoira-mode=zip", "--json")
	fixture.Cases[i].Failure = "before-cancel"
	fixture.Cases[i].Mode = "zip"
	i = add("pipeline-quarantine-failure", "--ugoira-mode=zip")
	fixture.Cases[i].Input = "{\"type\":\"ugoira\",\"id\":42,\"url\":\"https://www.pixiv.net/artworks/42\"}\n"
	fixture.Cases[i].Failure = "missing"
	fixture.Cases[i].Mode = "zip"
	i = add("pipeline-invalid-record", "--ugoira-mode=zip")
	fixture.Cases[i].Input = "{\"type\":\"ugoira\",\"id\":42}\n"
	fixture.Cases[i].Mode = "zip"
	for _, mode := range []string{"gif", "apng"} {
		i = add(mode+"-old-output-preserved", "42", "--ugoira-mode="+mode, "--json")
		fixture.Cases[i].Failure = "invalid-image-old"
		fixture.Cases[i].Mode = mode
	}
	for _, mode := range []string{"zip", "gif"} {
		i = add(mode+"-empty-template-name", "42", "--ugoira-mode="+mode, "--json")
		fixture.Cases[i].Failure = "empty-basename"
		fixture.Cases[i].Filename = "{tags}"
		fixture.Cases[i].Mode = mode
	}
	for _, failure := range []string{"metadata-rate", "all-rate"} {
		i = add("precommit-"+failure, "42", "--ugoira-mode=zip")
		fixture.Cases[i].Failure = failure
		fixture.Cases[i].Mode = "zip"
	}
	i = add("zip-missing-human", "42", "--ugoira-mode=zip")
	fixture.Cases[i].Failure = "missing"
	fixture.Cases[i].Mode = "zip"
	i = add("zip-corrupt-ndjson", "42", "--ugoira-mode=zip", "--ndjson")
	fixture.Cases[i].Failure = "corrupt"
	fixture.Cases[i].Mode = "zip"
	for n := range fixture.Cases {
		runUgoiraWorkflowCLICase(t, &fixture.Cases[n])
	}
	path := filepath.Join(rootRepo, "crates", "pixiv-cli", "tests", "fixtures", "download_ugoira.json")
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *updateUgoiraWorkflowCLI {
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
		t.Fatal("ugoira CLI differs from pinned Go reference")
	}
}

func runUgoiraWorkflowCLICase(t *testing.T, row *ugoiraWorkflowCLICase) {
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
	row.Attempts = []ugoiraWorkflowCLIAttempt{}
	row.Requests = []ugoiraWorkflowCLIRequest{}
	row.Saves = []ugoiraWorkflowCLISave{}
	row.Accounts = []ugoiraWorkflowCLIAccount{}
	row.Files = map[string]string{}
	row.Directories = []string{}
	normalize := func(s string) string { return strings.ReplaceAll(s, root, "$ROOT") }
	replace := func(s string) string { return strings.ReplaceAll(s, "$ROOT", root) }
	if row.Failure == "invalid-image-old" {
		old := filepath.Join(root, "runtime", "42 - Title_ slash_", "Author_ - Title_ slash__42."+row.Mode)
		if err := os.MkdirAll(filepath.Dir(old), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(old, []byte("old animation"), 0644); err != nil {
			t.Fatal(err)
		}
	}
	archive := generatedUgoiraCLIArchive(t, row.Failure)
	row.ArchiveHex = hex.EncodeToString(archive)
	var mutex sync.Mutex
	counts := map[string]int{}
	active := 0
	lastID := int64(0)
	transport := directDownloadTransport(func(req *http.Request) (*http.Response, error) {
		mutex.Lock()
		defer mutex.Unlock()
		accountID := lastID
		payload := "payload"
		status := 200
		header := http.Header{"Content-Type": {"image/png"}}
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
			active++
			stored, err := db.GetPixiv(ctx, parsed)
			if err != nil {
				return nil, err
			}
			if string(stored.RefreshTokenCopy()) != token {
				t.Fatal("stale refresh credential")
			}
			payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, parsed, parsed, parsed)
			header.Set("Content-Type", "application/json")
		} else {
			stored, err := db.GetPixiv(context.Background(), accountID)
			if err != nil {
				return nil, err
			}
			if stored.CredentialRevision != 2 {
				t.Fatal("request preceded persisted refresh")
			}
			if req.URL.Host == "app-api.pixiv.net" && req.Header.Get("Authorization") != fmt.Sprintf("Bearer fixture-access-%d", accountID) {
				t.Fatal("metadata request used wrong account authorization")
			}
			if req.URL.Host == "i.pximg.net" && req.Header.Get("Authorization") != "" {
				t.Fatal("resource request leaked authorization")
			}
			switch req.URL.Host {
			case "app-api.pixiv.net":
				if req.URL.Query().Get("illust_id") != "42" {
					t.Fatalf("unexpected metadata request %s", req.URL)
				}
				originals := []map[string]any{}
				for page := 0; page < row.Pages; page++ {
					originals = append(originals, map[string]any{"image_urls": map[string]string{"original": fmt.Sprintf("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p%d.jpg", page)}})
				}
				artwork := map[string]any{"id": 42, "type": row.Kind, "title": "Title: slash/", "user": map[string]any{"id": 7, "name": "Author?"}, "create_date": "2026-01-02T03:04:05+00:00", "page_count": row.Pages, "tags": []map[string]string{{"name": " first "}, {"name": "second"}}, "image_urls": map[string]string{"large": "https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p0.jpg"}, "meta_single_page": map[string]string{"original_image_url": "https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p0.jpg"}}
				if row.Failure == "empty-basename" {
					artwork["tags"] = []map[string]string{}
				}
				if row.Pages > 1 {
					artwork["meta_pages"] = originals
				}
				encoded, err := json.Marshal(map[string]any{"illust": artwork})
				if err != nil {
					return nil, err
				}
				payload = string(encoded)
				header.Set("Content-Type", "application/json")
				if req.URL.Path == "/v1/ugoira/metadata" {
					payload = string(ugoiraCLIMetadata(row.Failure))
				} else if row.Failure == "all-rate" || row.Failure == "metadata-rate" && accountID == 42 {
					status = 429
					key := fmt.Sprintf("%d:%s", accountID, req.URL.Path)
					counts[key]++
					if counts[key]%2 == 1 {
						header.Set("Retry-After", "0")
					} else {
						header.Set("Retry-After", "120")
					}

				} else if req.URL.Path != "/v1/illust/detail" {
					t.Fatalf("unexpected operation %s", req.URL.Path)
				}

			case "i.pximg.net":
				if strings.Contains(req.URL.Path, "ugoira") {
					payload = string(archive)
					header.Set("Content-Type", "application/zip")
					switch row.Failure {
					case "archive-rate":
						status = 429
						header.Set("Retry-After", "120")
					case "read", "cancel":
						bodyFailure = row.Failure
					}
				} else if row.Failure == "direct-rate" {
					status = 429
					header.Set("Retry-After", "120")
				}

			default:
				t.Fatalf("unexpected fixture host %s", req.URL.Host)
			}
		}
		row.Requests = append(row.Requests, ugoiraWorkflowCLIRequest{accountID, req.Method, req.URL.String(), req.Header.Get("Referer"), req.Header.Get("Cookie"), req.Header.Get("Authorization")})
		return &http.Response{StatusCode: status, Header: header, Body: &directDownloadBody{strings.NewReader(payload), bodyFailure, cancel}, Request: req}, nil
	})
	facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: pool.NewGate(), LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
		return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
	}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
		return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
	}, CloseClient: func(client *pixiv.Client) error { row.Closes++; active--; client.CloseIdleConnections(); return nil }})
	var stdout, stderr bytes.Buffer
	writer := ugoiraWorkflowCLIWriter{&stdout, &active, &row.WriterUnderLease, row.Failure == "writer"}
	command := download.New(download.Deps{Input: strings.NewReader(row.Input), Output: writer, ErrorOutput: &stderr, UsageError: func(err error) error { row.Events = append(row.Events, "usage"); return err }, JSONOut: func(override *bool) (bool, error) {
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
	}, Pooled: func(ctx context.Context, _ download.CommandRequest, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
		row.Events = append(row.Events, "pool")
		return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, func(ctx context.Context, client *pixiv.Client) (bool, error) {
			committed, err := invoke(ctx, client)
			attempt := ugoiraWorkflowCLIAttempt{Committed: committed, Cause: ugoiraWorkflowCLICause(err)}
			if err != nil {
				attempt.Error = normalize(err.Error())
			}
			row.Attempts = append(row.Attempts, attempt)
			return committed, err
		})
	}})
	command.SetOut(writer)
	command.SetErr(&stderr)
	cmd := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
	cmd.AddCommand(command)
	cmd.SetOut(writer)
	cmd.SetErr(&stderr)
	cmd.SetContext(ctx)
	args := []string{"download"}
	for _, arg := range row.Args {
		args = append(args, replace(arg))
	}
	cmd.SetArgs(args)
	if row.Failure == "before-cancel" {
		cancel()
	}
	resultErr := cmd.Execute()
	if resultErr != nil {
		row.Error = normalize(resultErr.Error())
		row.Cause = ugoiraWorkflowCLICause(resultErr)
	}
	if strings.Contains(row.Name, "no-replay") && (len(row.Attempts) != 1 || !row.Attempts[0].Committed) {
		t.Fatal("published archive prefix replayed or lost commit")
	}
	if strings.Contains(row.Name, "before-service") && len(row.Requests) != 0 {
		t.Fatal("validation made requests")
	}
	if row.Name == "writer-failure-after-commit" && (resultErr == nil || len(row.Attempts) != 1 || !row.Attempts[0].Committed || !row.WriterUnderLease) {
		t.Fatal("writer failure lost commit")
	}
	if row.Name == "precommit-metadata-rate" && (len(row.Attempts) != 2 || row.Attempts[0].Committed || !row.Attempts[1].Committed || resultErr != nil) {
		t.Fatal("typed precommit rate did not rotate and commit")
	}
	if row.Name == "pipeline-ugoira" && (resultErr != nil || len(row.Saves) != 1) {
		t.Fatal("action record did not run actual SDK download")
	}
	if row.Name == "zip-json" || row.Name == "raw-json" {
		if !strings.Contains(stdout.String(), `"frame_report"`) || !strings.Contains(stdout.String(), `"quality": "original"`) || strings.Contains(stdout.String(), `"page"`) {
			t.Fatal("archive artifact metadata changed")
		}
	}
	if row.Name == "gif-json" || row.Name == "apng-json" {
		if strings.Contains(stdout.String(), `"frames"`) || strings.Contains(stdout.String(), `"quality"`) || strings.Contains(stdout.String(), `"page"`) {
			t.Fatal("conversion unexpectedly carries archive metadata")
		}
	}
	row.Stdout = normalize(stdout.String())
	row.Stderr = normalize(stderr.String())
	if active != 0 {
		t.Fatalf("%s leaked account lease", row.Name)
	}
	for _, id := range []int64{42, 43} {
		stored, err := db.GetPixiv(context.Background(), id)
		if err != nil {
			t.Fatal(err)
		}
		row.Accounts = append(row.Accounts, ugoiraWorkflowCLIAccount{id, stored.CredentialRevision, stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), stored.PoolLastSelected, string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id)})
	}
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
			row.Directories = append(row.Directories, filepath.ToSlash(rel))
			return nil
		}
		if strings.HasPrefix(e.Name(), ".atomic-write-") || strings.HasPrefix(e.Name(), "ugoira-") || strings.HasPrefix(e.Name(), ".ugoira-") {
			t.Fatalf("%s leaked atomic temp", row.Name)
		}
		body, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		row.Files[filepath.ToSlash(rel)] = hex.EncodeToString(body)
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	if row.Failure == "invalid-image-old" {
		if len(row.Files) != 1 {
			t.Fatal("failed encoding altered output file count")
		}
		for _, body := range row.Files {
			if body != hex.EncodeToString([]byte("old animation")) {
				t.Fatal("failed encoding replaced old output")
			}
		}
	}
}

func normalizeUgoiraCLIPath(s string) string {
	if strings.HasPrefix(filepath.Base(s), "ugoira-") && strings.HasSuffix(s, ".zip") {
		return filepath.Join(filepath.Dir(s), "ugoira-$TEMP.zip")
	}
	return s
}
func generatedUgoiraCLIArchive(t *testing.T, failure string) []byte {
	t.Helper()
	if failure == "corrupt" {
		return []byte("corrupt ZIP")
	}
	var b bytes.Buffer
	w := zip.NewWriter(&b)
	names := []string{"000.png", "001.png"}
	switch failure {
	case "missing":
		names = names[:1]
	case "duplicate":
		names = append(names, "000.png")
	case "unsafe":
		names = append(names, "..foo")
	case "undeclared":
		names = append(names, "extra.png", "__MACOSX/ignored", "folder/")
	}
	for n, name := range names {
		entry, err := w.CreateHeader(&zip.FileHeader{Name: name, Method: zip.Store})
		if err != nil {
			t.Fatal(err)
		}
		if strings.HasSuffix(name, "/") {
			continue
		}
		if strings.HasPrefix(failure, "invalid-image") {
			_, err = entry.Write([]byte("not an image"))
		} else {
			im := image.NewNRGBA(image.Rect(0, 0, 2, 2))
			for y := 0; y < 2; y++ {
				for x := 0; x < 2; x++ {
					im.SetNRGBA(x, y, color.NRGBA{R: uint8(40 + n*50), G: uint8(x * 80), B: uint8(y * 90), A: 255})
				}
			}
			err = png.Encode(entry, im)
		}
		if err != nil {
			t.Fatal(err)
		}
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}
func ugoiraCLIMetadata(failure string) []byte {
	urls := map[string]string{"medium": "https://i.pximg.net/ugoira/42_medium.zip", "original": "https://i.pximg.net/ugoira/42_original.zip"}
	if failure == "medium-only" {
		delete(urls, "original")
	}
	if failure == "no-archive" {
		urls = map[string]string{}
	}
	frames := []map[string]any{{"file": "000.png", "delay": 70}, {"file": "001.png", "delay": 130}}
	if failure == "declared-duplicate" {
		frames[1]["file"] = "000.png"
	}
	if failure == "declared-unsafe" {
		frames[1]["file"] = "..foo"
	}
	raw, _ := json.Marshal(map[string]any{"ugoira_metadata": map[string]any{"zip_urls": urls, "frames": frames}})
	return raw
}
