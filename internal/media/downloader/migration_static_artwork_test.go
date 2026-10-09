package downloader_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationStaticFixture struct {
	Reference      string                `json:"reference"`
	SourceSHA256   map[string]string     `json:"source_sha256"`
	Evidence       string                `json:"evidence"`
	Normalizations []string              `json:"normalizations"`
	Deferred       []string              `json:"deferred"`
	Cases          []migrationStaticCase `json:"cases"`
}
type migrationStaticCase struct {
	Name                     string                   `json:"name"`
	Sources                  []string                 `json:"sources"`
	Artworks                 []migrationStaticArtwork `json:"artworks"`
	FilenameTemplate         string                   `json:"filename_template"`
	DirectoryTemplate        string                   `json:"directory_template"`
	DefaultDirectoryTemplate string                   `json:"default_directory_template,omitempty"`
	Quality                  string                   `json:"quality"`
	Pages                    []int                    `json:"pages"`
	CancelBefore             bool                     `json:"cancel_before,omitempty"`
	Existing                 map[string]string        `json:"existing,omitempty"`
	Expected                 migrationStaticResult    `json:"expected"`
}
type migrationStaticArtwork struct {
	ID          int64                 `json:"id"`
	Kind        string                `json:"kind"`
	RawKind     string                `json:"raw_kind,omitempty"`
	Title       string                `json:"title"`
	Author      string                `json:"author"`
	AuthorID    int64                 `json:"author_id"`
	PublishedAt string                `json:"published_at"`
	Tags        []string              `json:"tags"`
	PageCount   int                   `json:"page_count"`
	Pages       []migrationStaticPage `json:"pages"`
	Error       string                `json:"error,omitempty"`
}
type migrationStaticPage struct {
	URL       string `json:"url"`
	PageIndex int    `json:"page_index"`
	Payload   string `json:"payload,omitempty"`
	Product   string `json:"product,omitempty"`
	BodyHex   string `json:"body_hex"`
	MIME      string `json:"mime"`
	Error     string `json:"error,omitempty"`
}
type migrationStaticCall struct {
	Kind         string `json:"kind"`
	ID           int64  `json:"id,omitempty"`
	Source       string `json:"source,omitempty"`
	Payload      string `json:"payload,omitempty"`
	Path         string `json:"path,omitempty"`
	ParentExists bool   `json:"parent_exists,omitempty"`
}
type migrationStaticFile struct {
	Path  string `json:"path"`
	Page  int    `json:"page"`
	Bytes int64  `json:"bytes"`
}
type migrationStaticItem struct {
	ID     int64                 `json:"id"`
	Title  string                `json:"title"`
	Author string                `json:"author"`
	Kind   string                `json:"kind"`
	Files  []migrationStaticFile `json:"files"`
}
type migrationStaticFailure struct {
	ID          int64  `json:"id"`
	URL         string `json:"url"`
	Kind        string `json:"kind"`
	Message     string `json:"message"`
	CauseKind   string `json:"cause_kind"`
	CauseReason string `json:"cause_reason"`
	Code        string `json:"code"`
}
type migrationStaticEntry struct {
	Path      string `json:"path"`
	Directory bool   `json:"directory"`
	BodyHex   string `json:"body_hex,omitempty"`
}
type migrationStaticResult struct {
	Calls        []migrationStaticCall    `json:"calls"`
	Items        []migrationStaticItem    `json:"items"`
	Failures     []migrationStaticFailure `json:"failures"`
	Manifest     []migrationStaticEntry   `json:"manifest"`
	Committed    bool                     `json:"committed"`
	WarningCount int                      `json:"warning_count"`
	Error        string                   `json:"error"`
	ErrorKind    string                   `json:"error_kind"`
}

type migrationStaticClient struct {
	downloader.DownloadTargetClient
	artwork func(context.Context, pixiv.ArtworkRequest) (pixiv.Artwork, error)
	save    func(context.Context, sdk.ResourceRef, sdk.SaveOptions) (sdk.SavedResource, error)
	saveURL func(context.Context, string, sdk.SaveOptions) (sdk.SavedResource, error)
}

func (c *migrationStaticClient) Artwork(ctx context.Context, r pixiv.ArtworkRequest) (pixiv.Artwork, error) {
	return c.artwork(ctx, r)
}
func (c *migrationStaticClient) SaveResource(ctx context.Context, r sdk.ResourceRef, o sdk.SaveOptions) (sdk.SavedResource, error) {
	return c.save(ctx, r, o)
}
func (c *migrationStaticClient) SaveResourceURL(ctx context.Context, u string, o sdk.SaveOptions) (sdk.SavedResource, error) {
	return c.saveURL(ctx, u, o)
}
func (c *migrationStaticClient) UgoiraMetadata(context.Context, pixiv.UgoiraMetadataRequest) (pixiv.UgoiraMetadata, error) {
	panic("ugoira workflow is outside static fixture")
}

func migrationStaticErrorKind(err error) (string, string) {
	if err == nil {
		return "", ""
	}
	if errors.Is(err, context.Canceled) {
		return "canceled", ""
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return "deadline", ""
	}
	var typed *sdk.Error
	if errors.As(err, &typed) {
		return "sdk", string(typed.Reason)
	}
	return "plain", ""
}

func TestMigrationStaticArtworkFixture(t *testing.T) {
	const fixturePath = "../../../crates/pixiv-app/tests/fixtures/download_static.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationStaticFixture
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("unexpected Go reference")
	}
	for path, want := range fixture.SourceSHA256 {
		raw, err := os.ReadFile(filepath.Join("../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(raw)
		if hex.EncodeToString(digest[:]) != want {
			t.Fatalf("pinned Go source changed: %s", path)
		}
	}
	previous := runtime.GOMAXPROCS(1)
	defer runtime.GOMAXPROCS(previous)
	update := os.Getenv("PIXIV_UPDATE_STATIC_DOWNLOAD_FIXTURE") == "1"
	for index := range fixture.Cases {
		c := &fixture.Cases[index]
		t.Run(c.Name, func(t *testing.T) {
			got := migrationRunStaticCase(t, *c)
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
		body, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(fixturePath, append(body, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
}

func migrationRunStaticCase(t *testing.T, c migrationStaticCase) migrationStaticResult {
	t.Helper()
	root := filepath.Join(t.TempDir(), "download")
	normalize := func(path string) string { return filepath.ToSlash(strings.ReplaceAll(path, root, "${ROOT}")) }
	got := migrationStaticResult{Calls: []migrationStaticCall{}, Items: []migrationStaticItem{}, Failures: []migrationStaticFailure{}, Manifest: []migrationStaticEntry{}}
	for name, body := range c.Existing {
		path := filepath.Join(root, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if c.CancelBefore {
		cancel()
	}
	metadata := map[int64]migrationStaticArtwork{}
	pages := map[string]migrationStaticPage{}
	for _, art := range c.Artworks {
		metadata[art.ID] = art
		for index, page := range art.Pages {
			pages[fmt.Sprintf("%d/%d", art.ID, index)] = page
		}
	}
	save := func(kind, source, payload string, o sdk.SaveOptions, page migrationStaticPage) (sdk.SavedResource, error) {
		info, err := os.Stat(filepath.Dir(o.Path))
		got.Calls = append(got.Calls, migrationStaticCall{Kind: kind, Source: source, Payload: payload, Path: normalize(o.Path), ParentExists: err == nil && info.IsDir()})
		switch page.Error {
		case "business":
			return sdk.SavedResource{}, errors.New("synthetic page failure")
		case "typed":
			return sdk.SavedResource{}, sdk.NewError("pixiv", "SaveResource", sdk.ResourceForbidden)
		case "cancel":
			cancel()
			return sdk.SavedResource{}, context.Canceled
		case "context_error_only":
			return sdk.SavedResource{}, context.Canceled
		case "deadline":
			return sdk.SavedResource{}, context.DeadlineExceeded
		}
		body, err := hex.DecodeString(page.BodyHex)
		if err != nil {
			t.Fatal(err)
		}
		if err = os.MkdirAll(filepath.Dir(o.Path), 0700); err != nil {
			return sdk.SavedResource{}, err
		}
		if err = os.WriteFile(o.Path, body, 0600); err != nil {
			return sdk.SavedResource{}, err
		}
		return sdk.SavedResource{Path: o.Path, Size: int64(len(body)), ContentType: page.MIME}, nil
	}
	client := &migrationStaticClient{}
	client.artwork = func(_ context.Context, r pixiv.ArtworkRequest) (pixiv.Artwork, error) {
		got.Calls = append(got.Calls, migrationStaticCall{Kind: "artwork", ID: r.ArtworkID})
		input, ok := metadata[r.ArtworkID]
		if !ok {
			t.Fatalf("missing fixture artwork %d", r.ArtworkID)
		}
		if input.Error == "business" {
			return pixiv.Artwork{}, errors.New("synthetic metadata failure")
		}
		if input.Error == "typed" {
			return pixiv.Artwork{}, sdk.NewError("pixiv", "Artwork", sdk.RateLimited)
		}
		art := pixiv.Artwork{ID: input.ID, Kind: pixiv.ArtworkKind(input.Kind), RawKind: input.RawKind, Title: input.Title, User: pixiv.User{ID: input.AuthorID, Name: input.Author}, PageCount: input.PageCount}
		if input.PublishedAt != "" {
			var err error
			art.PublishedAt, err = time.Parse(time.RFC3339, input.PublishedAt)
			if err != nil {
				t.Fatal(err)
			}
		}
		for _, tag := range input.Tags {
			art.Tags = append(art.Tags, pixiv.Tag{Name: tag})
		}
		for index, page := range input.Pages {
			payload := page.Payload
			if payload == "" {
				payload = fmt.Sprintf(`{"k":"artwork","id":%d,"p":%d}`, input.ID, index)
			}
			product := page.Product
			if product == "" {
				product = "pixiv"
			}
			ref, err := sdk.NewResourceRef(product, []byte(payload))
			if err != nil {
				t.Fatal(err)
			}
			art.Pages = append(art.Pages, pixiv.ArtworkPage{PageIndex: page.PageIndex, Image: pixiv.ImageResource{Resource: sdk.Resource{Ref: ref, URL: page.URL}}})
		}
		return art, nil
	}
	client.save = func(_ context.Context, ref sdk.ResourceRef, o sdk.SaveOptions) (sdk.SavedResource, error) {
		payload, err := sdk.ResourceRefPayload(ref)
		if err != nil {
			return sdk.SavedResource{}, err
		}
		var identity struct {
			ID   int64 `json:"id"`
			Page int   `json:"p"`
		}
		if err = json.Unmarshal(payload, &identity); err != nil {
			return sdk.SavedResource{}, err
		}
		page, ok := pages[fmt.Sprintf("%d/%d", identity.ID, identity.Page)]
		if !ok {
			page = migrationStaticPage{BodyHex: hex.EncodeToString([]byte("direct-ref")), MIME: "image/jpeg"}
		}
		return save("ref", ref.String(), string(payload), o, page)
	}
	client.saveURL = func(_ context.Context, u string, o sdk.SaveOptions) (sdk.SavedResource, error) {
		return save("url", u, "", o, migrationStaticPage{BodyHex: hex.EncodeToString([]byte("direct-url")), MIME: "image/jpeg"})
	}
	service := downloader.DownloadService{NewManager: func(client downloader.DownloadClient, path, template string) (downloader.DownloadManager, error) {
		m := downloader.NewManager(client, path, template)
		m.SetDirectoryTemplate(c.DefaultDirectoryTemplate)
		return m, nil
	}}
	report, err := service.DownloadSources(ctx, client, c.Sources, downloader.DownloadRequest{DownloadPath: root, FilenameTemplate: c.FilenameTemplate, DirectoryTemplate: c.DirectoryTemplate, Quality: downloader.DownloadQuality(c.Quality), Pages: c.Pages})
	got.Committed = report.Committed
	got.WarningCount = len(report.Warnings)
	if err != nil {
		got.Error = normalize(err.Error())
		got.ErrorKind, _ = migrationStaticErrorKind(err)
	}
	for _, item := range report.Items {
		out := migrationStaticItem{ID: item.IllustID, Title: item.Title, Author: item.Author, Kind: item.Type, Files: []migrationStaticFile{}}
		for _, file := range item.Files {
			out.Files = append(out.Files, migrationStaticFile{Path: normalize(file.Path), Page: file.Page, Bytes: file.Bytes})
		}
		got.Items = append(got.Items, out)
	}
	for _, failure := range report.Failures {
		var linkError *os.LinkError
		if errors.As(failure.Cause, &linkError) && errors.Is(linkError, os.ErrExist) {
			failure.Message = "publish detected image extension: destination exists"
		}
		kind, reason := migrationStaticErrorKind(failure.Cause)
		got.Failures = append(got.Failures, migrationStaticFailure{ID: failure.IllustID, URL: failure.URL, Kind: failure.Type, Message: normalize(failure.Message), CauseKind: kind, CauseReason: reason, Code: failure.Code})
	}
	err = filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		out := migrationStaticEntry{Path: normalize(path), Directory: entry.IsDir()}
		if !entry.IsDir() {
			body, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			out.BodyHex = hex.EncodeToString(body)
		}
		got.Manifest = append(got.Manifest, out)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func TestMigrationStaticArtworkWorkerCompletionDoesNotReorderSortedReport(t *testing.T) {
	previous := runtime.GOMAXPROCS(2)
	defer runtime.GOMAXPROCS(previous)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	completed99 := make(chan struct{})
	var mu sync.Mutex
	completion := []int64{}
	client := &migrationStaticClient{}
	client.artwork = func(ctx context.Context, request pixiv.ArtworkRequest) (pixiv.Artwork, error) {
		if request.ArtworkID == 42 {
			select {
			case <-completed99:
			case <-ctx.Done():
				return pixiv.Artwork{}, ctx.Err()
			}
		}
		payload := fmt.Sprintf(`{"k":"artwork","id":%d,"p":0}`, request.ArtworkID)
		ref, err := sdk.NewResourceRef("pixiv", []byte(payload))
		if err != nil {
			return pixiv.Artwork{}, err
		}
		return pixiv.Artwork{ID: request.ArtworkID, Kind: pixiv.ArtworkKindIllustration, PageCount: 1, Pages: []pixiv.ArtworkPage{{Image: pixiv.ImageResource{Resource: sdk.Resource{URL: fmt.Sprintf("https://i.pximg.net/%d.jpg", request.ArtworkID), Ref: ref}}}}}, nil
	}
	client.save = func(_ context.Context, ref sdk.ResourceRef, options sdk.SaveOptions) (sdk.SavedResource, error) {
		payload, err := sdk.ResourceRefPayload(ref)
		if err != nil {
			return sdk.SavedResource{}, err
		}
		var identity struct {
			ID int64 `json:"id"`
		}
		if err := json.Unmarshal(payload, &identity); err != nil {
			return sdk.SavedResource{}, err
		}
		if err := os.WriteFile(options.Path, []byte("image"), 0600); err != nil {
			return sdk.SavedResource{}, err
		}
		mu.Lock()
		completion = append(completion, identity.ID)
		mu.Unlock()
		if identity.ID == 99 {
			close(completed99)
		}
		return sdk.SavedResource{Path: options.Path, Size: 5, ContentType: "image/jpeg"}, nil
	}
	batch, err := downloader.NewManager(client, t.TempDir(), "{id}").Download(ctx, downloader.DownloadRequest{IllustIDs: []int64{99, 42, 99, 0, -1}})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(completion, []int64{99, 42}) {
		t.Fatalf("completion = %v", completion)
	}
	ids := []int64{}
	for _, item := range batch.Items {
		ids = append(ids, item.IllustID)
	}
	if !reflect.DeepEqual(ids, []int64{42, 99}) || len(batch.Failures) != 0 {
		t.Fatalf("batch = %+v", batch)
	}
}

func TestMigrationStaticArtworkOpenStringProducerKindGoOnly(t *testing.T) {
	for _, kind := range []string{"illust", "synthetic_custom_kind"} {
		t.Run(kind, func(t *testing.T) {
			c := migrationStaticCase{Name: "open_string_producer_kind", Sources: []string{"42"}, FilenameTemplate: "{id}", Quality: "original", Artworks: []migrationStaticArtwork{{ID: 42, Kind: kind, Title: "title", Author: "author", PageCount: 1, Pages: []migrationStaticPage{{URL: "https://i.pximg.net/42.jpg", MIME: "image/jpeg", BodyHex: hex.EncodeToString([]byte("image"))}}}}}
			got := migrationRunStaticCase(t, c)
			if got.Error != "" || len(got.Failures) != 0 || len(got.Items) != 1 || got.Items[0].Kind != kind || len(got.Items[0].Files) != 1 || !got.Committed {
				t.Fatalf("open-string producer kind did not retain its static behavior: %+v", got)
			}
		})
	}
}
