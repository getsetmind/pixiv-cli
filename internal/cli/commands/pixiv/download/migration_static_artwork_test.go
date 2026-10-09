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

var updateStaticArtworkCLI = flag.Bool("migration-update-static-artwork-cli", false, "capture static artwork CLI contracts from pinned Go")

type staticArtworkCLIFixture struct {
	Reference     string                 `json:"reference"`
	SourceSHA256  map[string]string      `json:"source_sha256"`
	Normalization string                 `json:"normalization"`
	Remaining     []string               `json:"remaining"`
	Cases         []staticArtworkCLICase `json:"cases"`
}
type staticArtworkCLICase struct {
	Name             string                    `json:"name"`
	Args             []string                  `json:"args"`
	Input            string                    `json:"input"`
	Pages            int                       `json:"metadata_pages"`
	Kind             string                    `json:"metadata_kind"`
	RuntimeJSON      bool                      `json:"runtime_json"`
	Filename         string                    `json:"runtime_filename"`
	Directory        string                    `json:"runtime_directory"`
	Failure          string                    `json:"failure"`
	Stdout           string                    `json:"stdout"`
	Stderr           string                    `json:"stderr"`
	Error            string                    `json:"error"`
	Cause            string                    `json:"cause"`
	Events           []string                  `json:"events"`
	Attempts         []staticArtworkCLIAttempt `json:"attempts"`
	Saves            []staticArtworkCLISave    `json:"saves"`
	Requests         []staticArtworkCLIRequest `json:"requests"`
	Accounts         []staticArtworkCLIAccount `json:"accounts"`
	Closes           int                       `json:"closes"`
	WriterUnderLease bool                      `json:"writer_under_lease"`
	Files            map[string]string         `json:"files"`
	Directories      []string                  `json:"directories"`
}
type staticArtworkCLISave struct {
	Ref         string `json:"ref"`
	Destination string `json:"destination"`
}
type staticArtworkCLIClient struct {
	downloader.DownloadClient
	saves     *[]staticArtworkCLISave
	normalize func(string) string
}

func (c staticArtworkCLIClient) SaveResource(ctx context.Context, ref sdk.ResourceRef, options sdk.SaveOptions) (sdk.SavedResource, error) {
	*c.saves = append(*c.saves, staticArtworkCLISave{ref.String(), c.normalize(options.Path)})
	return c.DownloadClient.SaveResource(ctx, ref, options)
}

type staticArtworkCLIAttempt struct {
	Committed bool   `json:"committed"`
	Error     string `json:"error"`
	Cause     string `json:"cause"`
}
type staticArtworkCLIRequest struct {
	Account int64  `json:"account"`
	Method  string `json:"method"`
	URL     string `json:"url"`
	Referer string `json:"referer"`
	Cookie  string `json:"cookie"`
}
type staticArtworkCLIAccount struct {
	ID       int64 `json:"id"`
	Revision int64 `json:"revision"`
	Frozen   bool  `json:"frozen"`
	Selected bool  `json:"selected"`
	Rotated  bool  `json:"rotated"`
}
type staticArtworkCLIWriter struct {
	buffer *bytes.Buffer
	active *int
	under  *bool
	fail   bool
}

func (w staticArtworkCLIWriter) Write(b []byte) (int, error) {
	if *w.active > 0 {
		*w.under = true
	}
	if w.fail {
		return 0, errors.New("fixture writer failure")
	}
	return w.buffer.Write(b)
}
func staticArtworkCLICause(err error) string {
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

func TestMigrationStaticArtworkMatchesFrozenCLIContracts(t *testing.T) {
	rootRepo := filepath.Join("..", "..", "..", "..", "..")
	fixture := staticArtworkCLIFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA256: map[string]string{}, Normalization: "Only the isolated temporary download root is replaced by $ROOT. Request order, opaque SDK-derived resource URLs, array order, text, bytes and account state remain exact. Root source hashes are read-only wiring evidence; this harness executes a Cobra root plus the actual download leaf and saved-account facade, not native startup hooks.", Remaining: []string{"user/bookmark expansion, ugoira conversion/archive, visual action-record execution and recommendation-random remain separate accepted Go workflows; no replacement success or invalid-source expectations are introduced", "native root startup/association/browser and real network/accounts are not executed; existing isolated startup fixture remains independent", "full metadata/page/template/MIME manager matrices belong to the shared downloader fixture"}, Cases: []staticArtworkCLICase{}}
	pinnedSources := map[string]string{
		"internal/media/downloader/downloader.go":           "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"sdk/pixiv/resource.go":                             "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
		"internal/cli/commands/pixiv/download/execution.go": "236460386b2847dc03a00f10ac1e955ad1fca9f18c173ae0a1523f960812fdf0",
		"internal/cli/commands/pixiv/download/report.go":    "7d75a279defc954df3ab326af750fa1ca51021942a2bc4a205c34b80422b0b63",
		"internal/cli/root.go":                              "afbb8c2d7a90ccbe3ce124e9e2ee68ec463b9d020ba161d0d1cb270d1e1e7b1e",
		"internal/services/pixiv/facade.go":                 "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
		"internal/services/pixiv/pool/pool.go":              "be7f58dba0e1957268551bbf01bd73a9edc8365a6ec57751d2668960bda681d4",
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
		fixture.Cases = append(fixture.Cases, staticArtworkCLICase{Name: name, Args: args, Pages: 1, Kind: "illust", Filename: "{author} - {title}_{id}"})
		return len(fixture.Cases) - 1
	}
	add("pid-human", "42")
	add("pid-json", "42", "--json")
	add("artwork-url-ndjson", "https://www.pixiv.net/en/artworks/42", "--ndjson")
	add("canonical-dedup", "+42", "https://www.pixiv.net/artworks/42", "42", "--json")
	i := add("runtime-json", "42")
	fixture.Cases[i].RuntimeJSON = true
	i = add("explicit-json-false", "42", "--json=false")
	fixture.Cases[i].RuntimeJSON = true
	i = add("multipage-default", "42", "--ndjson")
	fixture.Cases[i].Pages = 3
	fixture.Cases[i].Kind = "manga"
	i = add("selection-sorted-dedup", "42", "--pages=3,1,3", "--json")
	fixture.Cases[i].Pages = 3
	for _, q := range []string{"original", "regular", "small", "thumb", "mini", ""} {
		i = add("quality-"+q, "42", "--quality="+q, "--json")
		fixture.Cases[i].Pages = 2
	}
	i = add("runtime-templates", "42", "--json")
	fixture.Cases[i].Pages = 2
	fixture.Cases[i].Filename = "{author_id}_{date}_{tags}_{num}"
	fixture.Cases[i].Directory = "{author}/{id}/{num}"
	i = add("template-override", "42", "--filename-template={id}_{num}", "--json")
	fixture.Cases[i].Pages = 2
	fixture.Cases[i].Directory = "nested/{id}"
	add("output-alias", "42", "-o", "$ROOT/override", "--json")
	add("download-path", "42", "--download-path=$ROOT/override", "--json")
	add("empty-template-default", "42", "--filename-template=", "--json")
	add("invalid-template", "42", "--filename-template={bad}", "--json")
	i = add("invalid-directory", "42", "--json")
	fixture.Cases[i].Directory = "../escape"
	add("missing-page", "42", "--pages=2", "--json")
	add("invalid-pages-before-service", "42", "--pages=bad", "--quality=bad")
	add("invalid-quality-before-service", "42", "--quality=bad")
	add("proxy-conflict-before-service", "42", "--proxy=x", "--no-proxy")
	add("output-conflict-before-pool", "42", "--json", "--ndjson")
	add("path-conflict-before-service", "42", "-o", "one", "--download-path=two")
	i = add("stdin-pid", "--json")
	fixture.Cases[i].Input = "42\r\n"
	i = add("typed-rate-limit-precommit-rotation", "42")
	fixture.Cases[i].Failure = "metadata-rate"
	i = add("typed-rate-limit-all-accounts", "42")
	fixture.Cases[i].Failure = "all-rate"
	for _, mode := range []string{"human", "json", "ndjson", "fail-fast"} {
		args := []string{"42"}
		if mode == "json" || mode == "ndjson" {
			args = append(args, "--"+mode)
		}
		if mode == "fail-fast" {
			args = append(args, "--json", "--on-error=fail-fast")
		}
		i = add("partial-page-rate-no-replay-"+mode, args...)
		fixture.Cases[i].Pages = 2
		fixture.Cases[i].Failure = "page-rate"
	}
	i = add("partial-page-read-no-replay", "42", "--json")
	fixture.Cases[i].Pages = 2
	fixture.Cases[i].Failure = "read"
	i = add("operation-cancel-after-page", "42", "--json")
	fixture.Cases[i].Pages = 2
	fixture.Cases[i].Failure = "cancel"
	i = add("operation-cancel-before-pool", "42", "--json")
	fixture.Cases[i].Failure = "before-cancel"
	i = add("writer-failure-after-commit", "42", "--json")
	fixture.Cases[i].Failure = "writer"
	add("mixed-static-before-direct", "https://i.pximg.net/assets/direct.png", "42", "--ndjson")
	i = add("unknown-upstream-kind-static", "42", "--json")
	fixture.Cases[i].Kind = "synthetic_new_kind"
	for n := range fixture.Cases {
		runStaticArtworkCLICase(t, &fixture.Cases[n])
	}
	path := filepath.Join(rootRepo, "crates", "pixiv-cli", "tests", "fixtures", "download_static.json")
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *updateStaticArtworkCLI {
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
		t.Fatal("static artwork CLI differs from pinned Go reference")
	}
}

func runStaticArtworkCLICase(t *testing.T, row *staticArtworkCLICase) {
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
	row.Attempts = []staticArtworkCLIAttempt{}
	row.Requests = []staticArtworkCLIRequest{}
	row.Saves = []staticArtworkCLISave{}
	row.Accounts = []staticArtworkCLIAccount{}
	row.Files = map[string]string{}
	row.Directories = []string{}
	normalize := func(s string) string { return strings.ReplaceAll(s, root, "$ROOT") }
	replace := func(s string) string { return strings.ReplaceAll(s, "$ROOT", root) }
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
				if req.URL.Path != "/v1/illust/detail" || req.URL.Query().Get("illust_id") != "42" {
					t.Fatalf("unexpected metadata request %s", req.URL)
				}
				originals := []map[string]any{}
				for page := 0; page < row.Pages; page++ {
					originals = append(originals, map[string]any{"image_urls": map[string]string{"original": fmt.Sprintf("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p%d.jpg", page)}})
				}
				artwork := map[string]any{"id": 42, "type": row.Kind, "title": "Title: slash/", "user": map[string]any{"id": 7, "name": "Author?"}, "create_date": "2026-01-02T03:04:05+00:00", "page_count": row.Pages, "tags": []map[string]string{{"name": " first "}, {"name": "second"}}, "image_urls": map[string]string{"large": "https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p0.jpg"}, "meta_single_page": map[string]string{"original_image_url": "https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p0.jpg"}}
				if row.Pages > 1 {
					artwork["meta_pages"] = originals
				}
				encoded, err := json.Marshal(map[string]any{"illust": artwork})
				if err != nil {
					return nil, err
				}
				payload = string(encoded)
				header.Set("Content-Type", "application/json")
				if row.Failure == "all-rate" || row.Failure == "metadata-rate" && accountID == 42 {
					status = 429
					key := fmt.Sprintf("%d:%s", accountID, req.URL.Path)
					counts[key]++
					if counts[key]%2 == 1 {
						header.Set("Retry-After", "0")
					} else {
						header.Set("Retry-After", "120")
					}
				}
			case "i.pximg.net":
				if strings.Contains(req.URL.Path, "42_p1") {
					switch row.Failure {
					case "page-rate":
						status = 429
						header.Set("Retry-After", "120")
					case "read", "cancel":
						bodyFailure = row.Failure
					}
				}
			default:
				t.Fatalf("unexpected fixture host %s", req.URL.Host)
			}
		}
		row.Requests = append(row.Requests, staticArtworkCLIRequest{accountID, req.Method, req.URL.String(), req.Header.Get("Referer"), req.Header.Get("Cookie")})
		return &http.Response{StatusCode: status, Header: header, Body: &directDownloadBody{strings.NewReader(payload), bodyFailure, cancel}, Request: req}, nil
	})
	facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: pool.NewGate(), LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
		return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
	}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
		return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
	}, CloseClient: func(client *pixiv.Client) error { row.Closes++; active--; client.CloseIdleConnections(); return nil }})
	var stdout, stderr bytes.Buffer
	writer := staticArtworkCLIWriter{&stdout, &active, &row.WriterUnderLease, row.Failure == "writer"}
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
			return downloader.NewManager(staticArtworkCLIClient{client, &row.Saves, normalize}, path, template), nil
		}}
	}, Pooled: func(ctx context.Context, _ download.CommandRequest, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
		row.Events = append(row.Events, "pool")
		return facade.Use(ctx, pixivapp.Request{Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, func(ctx context.Context, client *pixiv.Client) (bool, error) {
			committed, err := invoke(ctx, client)
			attempt := staticArtworkCLIAttempt{Committed: committed, Cause: staticArtworkCLICause(err)}
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
		row.Cause = staticArtworkCLICause(resultErr)
	}
	if row.Name == "pid-json" && (resultErr != nil || len(row.Saves) != 1 || len(row.Requests) != 3) {
		t.Fatalf("static SDK path did not execute: %v", resultErr)
	}
	if row.Name == "typed-rate-limit-precommit-rotation" && (len(row.Attempts) != 2 || row.Attempts[0].Committed || !row.Attempts[1].Committed || resultErr != nil) {
		t.Fatal("precommit account rotation did not execute")
	}
	if strings.HasPrefix(row.Name, "partial-page-") && (len(row.Attempts) != 1 || !row.Attempts[0].Committed) {
		t.Fatal("partial page publication replayed or lost commit")
	}
	if row.Name == "operation-cancel-after-page" && (!errors.Is(resultErr, context.Canceled) || len(row.Attempts) != 1 || !row.Attempts[0].Committed || stdout.Len() != 0) {
		t.Fatal("cancellation lost committed prefix or emitted ordinary machine report")
	}
	if row.Name == "writer-failure-after-commit" && (resultErr == nil || len(row.Attempts) != 1 || !row.Attempts[0].Committed || !row.WriterUnderLease) {
		t.Fatal("writer failure escaped committed lease")
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
		row.Accounts = append(row.Accounts, staticArtworkCLIAccount{id, stored.CredentialRevision, stored.PoolFrozenUntil != nil && *stored.PoolFrozenUntil > time.Now().Unix(), stored.PoolLastSelected, string(stored.RefreshTokenCopy()) == fmt.Sprintf("fixture-rotated-%d", id)})
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
		if strings.HasPrefix(e.Name(), ".atomic-write-") {
			t.Fatalf("%s leaked atomic temp", row.Name)
		}
		body, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		row.Files[filepath.ToSlash(rel)] = string(body)
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}
