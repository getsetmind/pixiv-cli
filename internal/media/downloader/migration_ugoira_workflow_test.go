package downloader_test

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/ugoira"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationUgoiraFixture struct {
	Reference      string                `json:"reference"`
	SourceSHA256   map[string]string     `json:"source_sha256"`
	Evidence       string                `json:"evidence"`
	Normalizations []string              `json:"normalizations"`
	Deferred       []string              `json:"deferred"`
	Cases          []migrationUgoiraCase `json:"cases"`
}
type migrationUgoiraArchive struct {
	Quality string `json:"quality"`
	URL     string `json:"url"`
	Payload string `json:"payload"`
}
type migrationUgoiraCase struct {
	Name                     string                   `json:"name"`
	Sources                  []string                 `json:"sources"`
	Format                   string                   `json:"format"`
	Quality                  string                   `json:"quality"`
	Pages                    []int                    `json:"pages"`
	Title                    string                   `json:"title"`
	FilenameTemplate         string                   `json:"filename_template"`
	DirectoryTemplate        string                   `json:"directory_template"`
	DefaultDirectoryTemplate string                   `json:"default_directory_template"`
	Archives                 []migrationUgoiraArchive `json:"archives"`
	Frames                   []ugoira.Frame           `json:"frames"`
	Entries                  []string                 `json:"entries"`
	Corrupt                  bool                     `json:"corrupt"`
	SaveError                string                   `json:"save_error"`
	MetadataError            string                   `json:"metadata_error"`
	EncoderError             string                   `json:"encoder_error"`
	CancelBefore             bool                     `json:"cancel_before"`
	Existing                 map[string]string        `json:"existing"`
	Expected                 migrationUgoiraResult    `json:"expected"`
}
type migrationUgoiraCall struct {
	Kind         string         `json:"kind"`
	ID           int64          `json:"id,omitempty"`
	Source       string         `json:"source,omitempty"`
	Payload      string         `json:"payload,omitempty"`
	Path         string         `json:"path,omitempty"`
	WorkDir      string         `json:"work_dir,omitempty"`
	ZipPath      string         `json:"zip_path,omitempty"`
	Format       string         `json:"format,omitempty"`
	Frames       []ugoira.Frame `json:"frames,omitempty"`
	ParentExists bool           `json:"parent_exists,omitempty"`
	ZipHex       string         `json:"zip_hex,omitempty"`
}
type migrationUgoiraItem struct {
	ID          int64                         `json:"id"`
	Title       string                        `json:"title"`
	Author      string                        `json:"author"`
	Kind        string                        `json:"kind"`
	Files       []migrationStaticFile         `json:"files"`
	Quality     string                        `json:"quality"`
	Frames      []ugoira.Frame                `json:"frames"`
	FrameReport *downloader.UgoiraFrameReport `json:"frame_report"`
}
type migrationUgoiraFailure struct {
	ID          int64    `json:"id"`
	URL         string   `json:"url"`
	Kind        string   `json:"kind"`
	Message     string   `json:"message"`
	CauseKind   string   `json:"cause_kind"`
	CauseReason string   `json:"cause_reason"`
	Code        string   `json:"code"`
	Path        string   `json:"path"`
	Missing     []string `json:"missing"`
}
type migrationUgoiraResult struct {
	Calls     []migrationUgoiraCall        `json:"calls"`
	Items     []migrationUgoiraItem        `json:"items"`
	Failures  []migrationUgoiraFailure     `json:"failures"`
	Warnings  []downloader.DownloadWarning `json:"warnings"`
	Manifest  []migrationStaticEntry       `json:"manifest"`
	Committed bool                         `json:"committed"`
	Error     string                       `json:"error"`
	ErrorKind string                       `json:"error_kind"`
}
type migrationUgoiraClient struct {
	*migrationStaticClient
	metadata func(context.Context, pixiv.UgoiraMetadataRequest) (pixiv.UgoiraMetadata, error)
}

func (c *migrationUgoiraClient) UgoiraMetadata(ctx context.Context, r pixiv.UgoiraMetadataRequest) (pixiv.UgoiraMetadata, error) {
	return c.metadata(ctx, r)
}

type migrationUgoiraEncoder func(context.Context, ugoira.Input) error

func (f migrationUgoiraEncoder) Encode(ctx context.Context, in ugoira.Input) error { return f(ctx, in) }

func TestMigrationUgoiraWorkflowFixture(t *testing.T) {
	const fixturePath = "../../../crates/pixiv-app/tests/fixtures/download_ugoira.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationUgoiraFixture
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if len(fixture.Cases) != 70 || len(fixture.SourceSHA256) != 11 {
		t.Fatal("ugoira workflow fixture coverage or source guards changed")
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("unexpected Go reference")
	}
	for path, want := range fixture.SourceSHA256 {
		raw, err := os.ReadFile(filepath.Join("../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(raw)
		if hex.EncodeToString(sum[:]) != want {
			t.Fatalf("pinned Go source changed: %s", path)
		}
	}
	previous := runtime.GOMAXPROCS(1)
	defer runtime.GOMAXPROCS(previous)
	update := os.Getenv("PIXIV_UPDATE_UGOIRA_DOWNLOAD_FIXTURE") == "1"
	for i := range fixture.Cases {
		c := &fixture.Cases[i]
		t.Run(c.Name, func(t *testing.T) {
			got := migrationRunUgoiraCase(t, *c)
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
		raw, err = json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(fixturePath, append(raw, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
}
func migrationRunUgoiraCase(t *testing.T, c migrationUgoiraCase) migrationUgoiraResult {
	t.Helper()
	root := filepath.Join(t.TempDir(), "download")
	normalize := func(p string) string { return filepath.ToSlash(strings.ReplaceAll(p, root, "${ROOT}")) }
	got := migrationUgoiraResult{Calls: []migrationUgoiraCall{}, Items: []migrationUgoiraItem{}, Failures: []migrationUgoiraFailure{}, Warnings: []downloader.DownloadWarning{}, Manifest: []migrationStaticEntry{}}
	var mu sync.Mutex
	record := func(call migrationUgoiraCall) { mu.Lock(); defer mu.Unlock(); got.Calls = append(got.Calls, call) }
	for name, body := range c.Existing {
		p := filepath.Join(root, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(p), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var buffer bytes.Buffer
	writer := zip.NewWriter(&buffer)
	for _, name := range c.Entries {
		entry, err := writer.CreateHeader(&zip.FileHeader{Name: name, Method: zip.Store})
		if err != nil {
			t.Fatal(err)
		}
		if !strings.HasSuffix(name, "/") {
			if _, err = entry.Write([]byte("synthetic-frame:" + name)); err != nil {
				t.Fatal(err)
			}
		}
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	zipBody := buffer.Bytes()
	if c.Corrupt {
		zipBody = []byte("synthetic corrupt zip")
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if c.CancelBefore {
		cancel()
	}
	refFor := func(payload string) sdk.ResourceRef {
		r, err := sdk.NewResourceRef("pixiv", []byte(payload))
		if err != nil {
			t.Fatal(err)
		}
		return r
	}
	client := &migrationUgoiraClient{migrationStaticClient: &migrationStaticClient{}}
	client.artwork = func(_ context.Context, r pixiv.ArtworkRequest) (pixiv.Artwork, error) {
		record(migrationUgoiraCall{Kind: "artwork", ID: r.ArtworkID})
		if r.ArtworkID == 7 {
			return pixiv.Artwork{ID: 7, Kind: pixiv.ArtworkKindIllustration, Title: "static", User: pixiv.User{Name: "author"}, PageCount: 1, Pages: []pixiv.ArtworkPage{{Image: pixiv.ImageResource{Resource: sdk.Resource{Ref: refFor(`{"k":"artwork","id":7,"p":0}`), URL: "https://i.pximg.net/7.jpg"}}}}}, nil
		}
		return pixiv.Artwork{ID: r.ArtworkID, Kind: pixiv.ArtworkKindUgoira, Title: c.Title, User: pixiv.User{ID: 3, Name: "author"}, PageCount: 1}, nil
	}
	client.metadata = func(_ context.Context, r pixiv.UgoiraMetadataRequest) (pixiv.UgoiraMetadata, error) {
		record(migrationUgoiraCall{Kind: "metadata", ID: r.ArtworkID})
		if c.MetadataError != "" {
			return pixiv.UgoiraMetadata{}, errors.New(c.MetadataError)
		}
		m := pixiv.UgoiraMetadata{ArtworkID: r.ArtworkID}
		for _, a := range c.Archives {
			m.Archives = append(m.Archives, pixiv.UgoiraArchive{Quality: pixiv.UgoiraQuality(a.Quality), Resource: sdk.Resource{URL: a.URL, Ref: refFor(a.Payload)}})
		}
		for _, f := range c.Frames {
			m.Frames = append(m.Frames, pixiv.UgoiraFrame{Filename: f.File, DelayMilliseconds: f.Delay})
		}
		return m, nil
	}
	tempPaths := map[string]string{}
	normalizeTemp := func(p string) string {
		if n, ok := tempPaths[p]; ok {
			return n
		}
		return normalize(p)
	}
	client.save = func(_ context.Context, r sdk.ResourceRef, o sdk.SaveOptions) (sdk.SavedResource, error) {
		payload, err := sdk.ResourceRefPayload(r)
		if err != nil {
			return sdk.SavedResource{}, err
		}
		p := normalize(o.Path)
		body := zipBody
		mime := "application/zip"
		isArchive := strings.HasPrefix(filepath.Base(o.Path), "ugoira-") && strings.HasSuffix(o.Path, ".zip")
		if isArchive {
			p = normalize(filepath.Join(filepath.Dir(o.Path), "ugoira-${TEMP}.zip"))
			tempPaths[o.Path] = p
		} else {
			body = []byte("static-or-direct")
			mime = "image/jpeg"
		}
		info, statErr := os.Stat(filepath.Dir(o.Path))
		record(migrationUgoiraCall{Kind: "ref", Source: r.String(), Payload: string(payload), Path: p, ParentExists: statErr == nil && info.IsDir()})
		if isArchive && c.SaveError != "" {
			switch c.SaveError {
			case "cancel_after_write":
				if err := os.WriteFile(o.Path, body, 0600); err != nil {
					return sdk.SavedResource{}, err
				}
				cancel()
				return sdk.SavedResource{}, context.Canceled
			case "cancel":
				cancel()
				return sdk.SavedResource{}, context.Canceled
			case "context_error_only":
				return sdk.SavedResource{}, context.Canceled
			case "deadline":
				return sdk.SavedResource{}, context.DeadlineExceeded
			case "typed":
				return sdk.SavedResource{}, sdk.NewError("pixiv", "SaveResource", sdk.ResourceForbidden)
			default:
				return sdk.SavedResource{}, errors.New(c.SaveError)
			}
		}
		if err = os.MkdirAll(filepath.Dir(o.Path), 0700); err != nil {
			return sdk.SavedResource{}, err
		}
		if err = os.WriteFile(o.Path, body, 0600); err != nil {
			return sdk.SavedResource{}, err
		}
		return sdk.SavedResource{Path: o.Path, Size: int64(len(body)), ContentType: mime}, nil
	}
	client.saveURL = func(_ context.Context, u string, o sdk.SaveOptions) (sdk.SavedResource, error) {
		record(migrationUgoiraCall{Kind: "url", Source: u, Path: normalize(o.Path)})
		body := []byte("direct-url")
		if err := os.WriteFile(o.Path, body, 0600); err != nil {
			return sdk.SavedResource{}, err
		}
		return sdk.SavedResource{Path: o.Path, Size: int64(len(body)), ContentType: "image/jpeg"}, nil
	}
	encoder := migrationUgoiraEncoder(func(_ context.Context, in ugoira.Input) error {
		body, err := os.ReadFile(in.ZipPath)
		if err != nil {
			return err
		}
		record(migrationUgoiraCall{Kind: "encoder_port", Path: normalize(in.OutputPath), ZipPath: normalizeTemp(in.ZipPath), WorkDir: normalize(in.WorkDir), Format: string(in.Format), Frames: in.Frames, ZipHex: hex.EncodeToString(body)})
		if c.EncoderError != "" {
			if c.EncoderError == "context_error_only" {
				return context.Canceled
			}
			if c.EncoderError == "deadline" {
				return context.DeadlineExceeded
			}
			if c.EncoderError == "cancel" {
				cancel()
				return context.Canceled
			}
			return errors.New(c.EncoderError)
		}
		return os.WriteFile(in.OutputPath, []byte("injected-encoder-port:"+string(in.Format)), 0600)
	})
	service := downloader.DownloadService{NewManager: func(client downloader.DownloadClient, path, template string) (downloader.DownloadManager, error) {
		m := downloader.NewManager(client, path, template)
		m.SetDirectoryTemplate(c.DefaultDirectoryTemplate)
		m.SetUgoiraEncoder(encoder)
		return m, nil
	}}
	report, err := service.DownloadSources(ctx, client, c.Sources, downloader.DownloadRequest{DownloadPath: root, FilenameTemplate: c.FilenameTemplate, DirectoryTemplate: c.DirectoryTemplate, Quality: downloader.DownloadQuality(c.Quality), Pages: c.Pages, UgoiraFormat: downloader.UgoiraFormat(c.Format)})
	got.Committed = report.Committed
	got.Warnings = append(got.Warnings, report.Warnings...)
	if err != nil {
		got.Error = normalize(err.Error())
		got.ErrorKind, _ = migrationStaticErrorKind(err)
	}
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
		got.Failures = append(got.Failures, migrationUgoiraFailure{ID: f.IllustID, URL: f.URL, Kind: f.Type, Message: normalize(f.Message), CauseKind: kind, CauseReason: reason, Code: f.Code, Path: normalize(f.Path), Missing: f.Missing})
	}
	if err := filepath.WalkDir(root, func(p string, e os.DirEntry, err error) error {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		out := migrationStaticEntry{Path: normalize(p), Directory: e.IsDir()}
		if !e.IsDir() {
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
