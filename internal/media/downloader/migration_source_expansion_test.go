package downloader_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/ugoira"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationExpansionFixture struct {
	Reference           string                   `json:"reference"`
	SourceSHA256        map[string]string        `json:"source_sha256"`
	ReusedFixtureSHA256 map[string]string        `json:"reused_fixture_sha256"`
	Evidence            string                   `json:"evidence"`
	Normalizations      []string                 `json:"normalizations"`
	Deferred            []string                 `json:"deferred"`
	Cases               []migrationExpansionCase `json:"cases"`
}
type migrationExpansionCase struct {
	Name              string                   `json:"name"`
	Sources           []string                 `json:"sources"`
	Lists             []migrationExpansionPage `json:"lists"`
	Artworks          []migrationStaticArtwork `json:"artworks"`
	FilenameTemplate  string                   `json:"filename_template"`
	DirectoryTemplate string                   `json:"directory_template"`
	Quality           string                   `json:"quality"`
	Format            string                   `json:"format"`
	Pages             []int                    `json:"pages"`
	CancelBefore      bool                     `json:"cancel_before,omitempty"`
	UgoiraFixtureCase string                   `json:"ugoira_fixture_case,omitempty"`
	Expected          migrationExpansionResult `json:"expected"`
}
type migrationExpansionPage struct {
	Operation string                            `json:"operation"`
	UserID    int64                             `json:"user_id"`
	Kind      string                            `json:"kind"`
	Cursor    string                            `json:"cursor"`
	Next      string                            `json:"next"`
	Items     []migrationExpansionListedArtwork `json:"items"`
	Error     string                            `json:"error,omitempty"`
	Cancel    bool                              `json:"cancel,omitempty"`
}
type migrationExpansionListedArtwork struct {
	ID    int64  `json:"id"`
	Kind  string `json:"kind"`
	Title string `json:"title"`
}
type migrationExpansionCall struct {
	Operation    string `json:"operation"`
	ID           int64  `json:"id,omitempty"`
	Kind         string `json:"kind,omitempty"`
	Cursor       string `json:"cursor,omitempty"`
	Restrict     string `json:"restrict,omitempty"`
	Tag          string `json:"tag,omitempty"`
	ContextError string `json:"context_error,omitempty"`
	Source       string `json:"source,omitempty"`
	Payload      string `json:"payload,omitempty"`
	Path         string `json:"path,omitempty"`
	ParentExists bool   `json:"parent_exists,omitempty"`
	Progress     bool   `json:"progress,omitempty"`
}
type migrationExpansionManagerRequest struct {
	IDs               []int64 `json:"ids"`
	DownloadPath      string  `json:"download_path"`
	FilenameTemplate  string  `json:"filename_template"`
	DirectoryTemplate string  `json:"directory_template"`
	Pages             []int   `json:"pages"`
	Quality           string  `json:"quality"`
	Format            string  `json:"format"`
}
type migrationExpansionFailure struct {
	migrationUgoiraFailure
	CauseMessage   string `json:"cause_message"`
	CauseProduct   string `json:"cause_product"`
	CauseOperation string `json:"cause_operation"`
	RetrySafe      bool   `json:"retry_safe"`
	RetryHasAfter  bool   `json:"retry_has_after"`
	RetryAfter     string `json:"retry_after"`
	OriginalCause  bool   `json:"original_cause"`
}
type migrationExpansionResult struct {
	Calls           []migrationExpansionCall           `json:"calls"`
	ManagerRequests []migrationExpansionManagerRequest `json:"manager_requests"`
	Items           []migrationUgoiraItem              `json:"items"`
	Failures        []migrationExpansionFailure        `json:"failures"`
	Warnings        []downloader.DownloadWarning       `json:"warnings"`
	Manifest        []migrationStaticEntry             `json:"manifest"`
	Diagnostics     []migrationDirectDiagnostic        `json:"diagnostics"`
	Committed       bool                               `json:"committed"`
	Error           string                             `json:"error"`
	ErrorKind       string                             `json:"error_kind"`
	ErrorReason     string                             `json:"error_reason"`
	OriginalError   bool                               `json:"original_error"`
}
type migrationExpansionClient struct {
	*migrationUgoiraClient
	list func(context.Context, string, int64, string, string, string, sdk.Cursor) (sdk.Page[pixiv.Artwork], error)
}

func (c *migrationExpansionClient) UserArtworks(ctx context.Context, r pixiv.UserArtworksRequest) (sdk.Page[pixiv.Artwork], error) {
	return c.list(ctx, "user_artworks", r.UserID, string(r.Kind), "", "", r.Cursor)
}
func (c *migrationExpansionClient) UserArtworkBookmarks(ctx context.Context, r pixiv.UserArtworkBookmarksRequest) (sdk.Page[pixiv.Artwork], error) {
	return c.list(ctx, "user_bookmarks", r.UserID, "", string(r.Restrict), r.Tag, r.Cursor)
}

type migrationExpansionObservedManager struct {
	downloader.DownloadManager
	observe func(downloader.DownloadRequest)
}

func (m *migrationExpansionObservedManager) Download(ctx context.Context, r downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	m.observe(r)
	return m.DownloadManager.Download(ctx, r)
}

func TestMigrationSourceExpansionFixture(t *testing.T) {
	const fixturePath = "../../../crates/pixiv-app/tests/fixtures/download_source_expansion.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationExpansionFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" || len(fixture.Cases) != 52 || len(fixture.SourceSHA256) != 12 || len(fixture.ReusedFixtureSHA256) != 1 {
		t.Fatal("source expansion frozen coverage changed")
	}
	for path, want := range fixture.SourceSHA256 {
		current, err := os.ReadFile(filepath.Join("../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		original, err := exec.Command("git", "-C", "../../..", "show", fixture.Reference+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		for _, body := range [][]byte{current, original} {
			sum := sha256.Sum256(body)
			if hex.EncodeToString(sum[:]) != want {
				t.Fatalf("pinned Go source changed: %s", path)
			}
		}
	}
	for path, want := range fixture.ReusedFixtureSHA256 {
		body, err := os.ReadFile(filepath.Join("../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(body)
		if hex.EncodeToString(sum[:]) != want {
			t.Fatalf("reused media fixture changed: %s", path)
		}
	}
	previous := runtime.GOMAXPROCS(1)
	defer runtime.GOMAXPROCS(previous)
	update := os.Getenv("PIXIV_UPDATE_SOURCE_EXPANSION_FIXTURE") == "1"
	for i := range fixture.Cases {
		c := &fixture.Cases[i]
		t.Run(c.Name, func(t *testing.T) {
			got := migrationRunExpansionCase(t, *c)
			if update {
				c.Expected = got
				return
			}
			if !reflect.DeepEqual(got, c.Expected) {
				actual, _ := json.MarshalIndent(got, "", "  ")
				want, _ := json.MarshalIndent(c.Expected, "", "  ")
				t.Fatalf("actual:\n%s\nexpected:\n%s", actual, want)
			}
		})
	}
	if update {
		raw, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(fixturePath, append(raw, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
}

func migrationRunExpansionCase(t *testing.T, c migrationExpansionCase) migrationExpansionResult {
	t.Helper()
	root := filepath.Join(t.TempDir(), "download")
	temporary := regexp.MustCompile(`ugoira-[0-9]+\.zip`)
	normalize := func(p string) string {
		return temporary.ReplaceAllStringFunc(filepath.ToSlash(strings.ReplaceAll(p, root, "${ROOT}")), func(string) string { return "ugoira-${TEMP}.zip" })
	}
	got := migrationExpansionResult{Calls: []migrationExpansionCall{}, ManagerRequests: []migrationExpansionManagerRequest{}, Items: []migrationUgoiraItem{}, Failures: []migrationExpansionFailure{}, Warnings: []downloader.DownloadWarning{}, Manifest: []migrationStaticEntry{}, Diagnostics: []migrationDirectDiagnostic{}}
	var mu sync.Mutex
	record := func(call migrationExpansionCall) { mu.Lock(); defer mu.Unlock(); got.Calls = append(got.Calls, call) }
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if c.CancelBefore {
		cancel()
	}
	ctx = diagnostics.WithScope(ctx, diagnostics.SinkFunc(func(e diagnostics.Event) {
		if e.Duration < 0 {
			t.Error("negative diagnostic duration")
		}
		got.Diagnostics = append(got.Diagnostics, migrationDirectDiagnostic{Module: string(e.Module), Kind: string(e.Kind), Operation: e.Operation, Count: e.Count, Reason: string(e.Reason)})
	}), diagnostics.ModulePixivCLI, 0)
	causes := []error{}
	failureError := func(operation, outcome string) error {
		var err error
		switch outcome {
		case "":
			return nil
		case "business":
			err = errors.New("synthetic " + operation + " failure")
		case "rate":
			err = sdk.NewError("pixiv", operation, sdk.RateLimited, sdk.WithHTTPStatus(429), sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: time.Date(2026, 10, 11, 1, 2, 3, 0, time.UTC)}))
		case "forbidden":
			err = sdk.NewError("pixiv", operation, sdk.ResourceForbidden)
		case "context_error_only":
			err = context.Canceled
		case "wrapped_context_error_only":
			err = sdk.NewError("pixiv", operation, sdk.UpstreamUnavailable, sdk.WithCause(context.Canceled))
		case "deadline_error_only":
			err = context.DeadlineExceeded
		default:
			t.Fatalf("unknown synthetic outcome %q", outcome)
		}
		causes = append(causes, err)
		return err
	}
	originalCause := func(err error) bool {
		for _, cause := range causes {
			if err == cause {
				return true
			}
		}
		return false
	}
	parseCursor := func(raw string) sdk.Cursor {
		if raw == "" {
			return sdk.Cursor{}
		}
		cursor, err := sdk.ParseCursor(raw)
		if err != nil {
			t.Fatal(err)
		}
		return cursor
	}
	nextList := 0
	client := &migrationExpansionClient{migrationUgoiraClient: &migrationUgoiraClient{migrationStaticClient: &migrationStaticClient{}}}
	client.list = func(ctx context.Context, operation string, id int64, kind, restrict, tag string, cursor sdk.Cursor) (sdk.Page[pixiv.Artwork], error) {
		call := migrationExpansionCall{Operation: operation, ID: id, Kind: kind, Cursor: cursor.String(), Restrict: restrict, Tag: tag}
		if err := ctx.Err(); err != nil {
			call.ContextError = err.Error()
		}
		record(call)
		if nextList >= len(c.Lists) {
			t.Fatalf("unexpected list call %+v", call)
		}
		page := c.Lists[nextList]
		nextList++
		if page.Operation != operation || page.UserID != id || page.Kind != kind || page.Cursor != cursor.String() {
			t.Fatalf("list request %+v does not match scripted page %+v", call, page)
		}
		if operation == "user_bookmarks" && (restrict != "public" || tag != "") {
			t.Fatalf("bookmark restriction/tag changed: %+v", call)
		}
		if page.Cancel {
			cancel()
		}
		if err := failureError(operation, page.Error); err != nil {
			return sdk.Page[pixiv.Artwork]{}, err
		}
		items := []pixiv.Artwork{}
		for _, item := range page.Items {
			items = append(items, pixiv.Artwork{ID: item.ID, Kind: pixiv.ArtworkKind(item.Kind), Title: item.Title, User: pixiv.User{Name: "list author must be discarded"}})
		}
		return sdk.Page[pixiv.Artwork]{Items: items, Next: parseCursor(page.Next)}, nil
	}
	metadata := map[int64]migrationStaticArtwork{}
	pages := map[string]migrationStaticPage{}
	for _, artwork := range c.Artworks {
		metadata[artwork.ID] = artwork
		for i, page := range artwork.Pages {
			pages[fmt.Sprintf("%d/%d", artwork.ID, i)] = page
		}
	}
	resource := func(payload string) sdk.ResourceRef {
		ref, err := sdk.NewResourceRef("pixiv", []byte(payload))
		if err != nil {
			t.Fatal(err)
		}
		return ref
	}
	client.artwork = func(_ context.Context, r pixiv.ArtworkRequest) (pixiv.Artwork, error) {
		record(migrationExpansionCall{Operation: "artwork", ID: r.ArtworkID})
		input, ok := metadata[r.ArtworkID]
		if !ok {
			t.Fatalf("missing detail fixture %d", r.ArtworkID)
		}
		if err := failureError("Artwork", input.Error); err != nil {
			return pixiv.Artwork{}, err
		}
		out := pixiv.Artwork{ID: input.ID, Kind: pixiv.ArtworkKind(input.Kind), RawKind: input.RawKind, Title: input.Title, User: pixiv.User{ID: input.AuthorID, Name: input.Author}, PageCount: input.PageCount}
		for i, page := range input.Pages {
			payload := page.Payload
			if payload == "" {
				payload = fmt.Sprintf(`{"k":"artwork","id":%d,"p":%d}`, input.ID, i)
			}
			out.Pages = append(out.Pages, pixiv.ArtworkPage{PageIndex: page.PageIndex, Image: pixiv.ImageResource{Resource: sdk.Resource{Ref: resource(payload), URL: page.URL}}})
		}
		return out, nil
	}
	var native struct {
		Cases []struct {
			Name   string         `json:"name"`
			Frames []ugoira.Frame `json:"frames"`
			ZIPHex string         `json:"zip_hex"`
		} `json:"cases"`
	}
	var zipBody []byte
	var frames []ugoira.Frame
	if c.UgoiraFixtureCase != "" {
		raw, err := os.ReadFile("../../../crates/pixiv-app/tests/fixtures/ugoira_encoder.json")
		if err != nil {
			t.Fatal(err)
		}
		if err = json.Unmarshal(raw, &native); err != nil {
			t.Fatal(err)
		}
		for _, fixture := range native.Cases {
			if fixture.Name == c.UgoiraFixtureCase {
				frames = fixture.Frames
				zipBody, err = hex.DecodeString(fixture.ZIPHex)
				if err != nil {
					t.Fatal(err)
				}
				break
			}
		}
		if zipBody == nil {
			t.Fatal("missing reusable native fixture")
		}
	}
	client.metadata = func(_ context.Context, r pixiv.UgoiraMetadataRequest) (pixiv.UgoiraMetadata, error) {
		record(migrationExpansionCall{Operation: "ugoira_metadata", ID: r.ArtworkID})
		out := pixiv.UgoiraMetadata{ArtworkID: r.ArtworkID, Archives: []pixiv.UgoiraArchive{{Quality: pixiv.UgoiraQualityOriginal, Resource: sdk.Resource{URL: fmt.Sprintf("https://i.pximg.net/%d.zip", r.ArtworkID), Ref: resource(fmt.Sprintf(`{"k":"ugoira","id":%d,"q":"original"}`, r.ArtworkID))}}}}
		for _, frame := range frames {
			out.Frames = append(out.Frames, pixiv.UgoiraFrame{Filename: frame.File, DelayMilliseconds: frame.Delay})
		}
		return out, nil
	}
	save := func(ctx context.Context, operation, source, payload string, o sdk.SaveOptions, body []byte, mime, outcome string) (sdk.SavedResource, error) {
		info, err := os.Stat(filepath.Dir(o.Path))
		call := migrationExpansionCall{Operation: operation, Source: source, Payload: payload, Path: normalize(o.Path), ParentExists: err == nil && info.IsDir(), Progress: o.Progress != nil}
		if err := ctx.Err(); err != nil {
			call.ContextError = err.Error()
		}
		record(call)
		if err := failureError("SaveResource", outcome); err != nil {
			return sdk.SavedResource{}, err
		}
		if err = os.MkdirAll(filepath.Dir(o.Path), 0700); err != nil {
			return sdk.SavedResource{}, err
		}
		if err = os.WriteFile(o.Path, body, 0600); err != nil {
			return sdk.SavedResource{}, err
		}
		return sdk.SavedResource{Path: o.Path, Size: int64(len(body)), ContentType: mime}, nil
	}
	client.save = func(ctx context.Context, ref sdk.ResourceRef, o sdk.SaveOptions) (sdk.SavedResource, error) {
		payload, err := sdk.ResourceRefPayload(ref)
		if err != nil {
			return sdk.SavedResource{}, err
		}
		var id struct {
			Kind string `json:"k"`
			ID   int64  `json:"id"`
			Page int    `json:"p"`
		}
		if err = json.Unmarshal(payload, &id); err != nil {
			return sdk.SavedResource{}, err
		}
		body, mime, outcome := []byte("direct-ref"), "image/jpeg", ""
		if id.Kind == "ugoira" {
			body, mime = zipBody, "application/zip"
		} else if page, ok := pages[fmt.Sprintf("%d/%d", id.ID, id.Page)]; ok {
			body, err = hex.DecodeString(page.BodyHex)
			if err != nil {
				t.Fatal(err)
			}
			mime, outcome = page.MIME, page.Error
		}
		return save(ctx, "save_ref", ref.String(), string(payload), o, body, mime, outcome)
	}
	client.saveURL = func(ctx context.Context, url string, o sdk.SaveOptions) (sdk.SavedResource, error) {
		return save(ctx, "save_url", url, "", o, []byte("direct-url"), "image/jpeg", "")
	}
	service := downloader.DownloadService{NewManager: func(client downloader.DownloadClient, path, template string) (downloader.DownloadManager, error) {
		record(migrationExpansionCall{Operation: "manager_factory", Path: normalize(path)})
		return &migrationExpansionObservedManager{DownloadManager: downloader.NewManager(client, path, template), observe: func(r downloader.DownloadRequest) {
			got.ManagerRequests = append(got.ManagerRequests, migrationExpansionManagerRequest{IDs: append([]int64{}, r.IllustIDs...), DownloadPath: normalize(r.DownloadPath), FilenameTemplate: r.FilenameTemplate, DirectoryTemplate: r.DirectoryTemplate, Pages: r.Pages, Quality: string(r.Quality), Format: string(r.UgoiraFormat)})
		}}, nil
	}}
	report, err := service.DownloadSources(ctx, client, c.Sources, downloader.DownloadRequest{DownloadPath: root, FilenameTemplate: c.FilenameTemplate, DirectoryTemplate: c.DirectoryTemplate, Quality: downloader.DownloadQuality(c.Quality), UgoiraFormat: downloader.UgoiraFormat(c.Format), Pages: c.Pages})
	if nextList != len(c.Lists) {
		t.Fatalf("consumed %d/%d scripted pages", nextList, len(c.Lists))
	}
	if err != nil {
		got.Error = normalize(err.Error())
		got.ErrorKind, got.ErrorReason = migrationStaticErrorKind(err)
		got.OriginalError = originalCause(err)
	}
	got.Committed = report.Committed
	got.Warnings = append(got.Warnings, report.Warnings...)
	for _, item := range report.Items {
		out := migrationUgoiraItem{ID: item.IllustID, Title: item.Title, Author: item.Author, Kind: item.Type, Files: []migrationStaticFile{}, Quality: item.Quality, FrameReport: item.FrameReport}
		for _, f := range item.Frames {
			out.Frames = append(out.Frames, ugoira.Frame{File: f.Filename, Delay: f.DelayMilliseconds})
		}
		for _, f := range item.Files {
			out.Files = append(out.Files, migrationStaticFile{Path: normalize(f.Path), Page: f.Page, Bytes: f.Bytes})
		}
		got.Items = append(got.Items, out)
	}
	for _, f := range report.Failures {
		kind, reason := migrationStaticErrorKind(f.Cause)
		out := migrationExpansionFailure{migrationUgoiraFailure: migrationUgoiraFailure{ID: f.IllustID, URL: f.URL, Kind: f.Type, Message: normalize(f.Message), CauseKind: kind, CauseReason: reason, Code: f.Code, Path: normalize(f.Path), Missing: f.Missing}, OriginalCause: originalCause(f.Cause)}
		if f.Cause != nil {
			out.CauseMessage = normalize(f.Cause.Error())
		}
		var typed *sdk.Error
		if errors.As(f.Cause, &typed) {
			out.CauseProduct = typed.Product
			out.CauseOperation = typed.Operation
			out.RetrySafe = typed.Retry.Safe
			out.RetryHasAfter = typed.Retry.HasAfter
			if typed.Retry.HasAfter {
				out.RetryAfter = typed.Retry.After.Format(time.RFC3339)
			}
		}
		got.Failures = append(got.Failures, out)
	}
	if err := filepath.WalkDir(root, func(p string, entry os.DirEntry, err error) error {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		out := migrationStaticEntry{Path: normalize(p), Directory: entry.IsDir()}
		if !entry.IsDir() {
			body, err := os.ReadFile(p)
			if err != nil {
				return err
			}
			out.BodyHex = hex.EncodeToString(body)
		}
		got.Manifest = append(got.Manifest, out)
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	return got
}
