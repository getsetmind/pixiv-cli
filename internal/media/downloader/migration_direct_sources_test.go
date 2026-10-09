package downloader_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

type migrationDirectFixture struct {
	Reference    string                `json:"reference"`
	SourceSHA256 string                `json:"source_sha256"`
	Evidence     string                `json:"evidence"`
	Deferred     []string              `json:"deferred"`
	Cases        []migrationDirectCase `json:"cases"`
}
type migrationDirectCase struct {
	CaptureDiagnostics   bool                  `json:"capture_diagnostics,omitempty"`
	Name                 string                `json:"name"`
	Sources              []string              `json:"sources"`
	DownloadPath         string                `json:"download_path"`
	UnsupportedURLClient bool                  `json:"unsupported_url_client,omitempty"`
	IgnoreArtworkOptions bool                  `json:"ignore_artwork_options,omitempty"`
	CancelBefore         bool                  `json:"cancel_before,omitempty"`
	Outcomes             []string              `json:"outcomes,omitempty"`
	Expected             migrationDirectResult `json:"expected"`
}
type migrationDirectCall struct {
	Kind   string `json:"kind"`
	Source string `json:"source"`
	Path   string `json:"path"`
}
type migrationDirectItem struct {
	IllustID int64  `json:"illust_id"`
	Title    string `json:"title"`
	Author   string `json:"author"`
	Type     string `json:"type"`
	Path     string `json:"path"`
	Page     int    `json:"page"`
	Bytes    int64  `json:"bytes"`
	Body     string `json:"body"`
}
type migrationDirectFailure struct {
	URL          string `json:"url"`
	Type         string `json:"type"`
	Message      string `json:"message"`
	CauseMessage string `json:"cause_message"`
	CauseKind    string `json:"cause_kind"`
	CauseReason  string `json:"cause_reason"`
	Code         string `json:"code"`
}
type migrationDirectDiagnostic struct {
	Module    string `json:"module"`
	Kind      string `json:"kind"`
	Operation string `json:"operation"`
	Count     int    `json:"count"`
	Reason    string `json:"reason"`
}
type migrationDirectResult struct {
	Diagnostics  []migrationDirectDiagnostic `json:"diagnostics,omitempty"`
	Calls        []migrationDirectCall       `json:"calls"`
	Items        []migrationDirectItem       `json:"items"`
	Failures     []migrationDirectFailure    `json:"failures"`
	WarningCount int                         `json:"warning_count"`
	Committed    bool                        `json:"committed"`
	Error        string                      `json:"error"`
	ErrorKind    string                      `json:"error_kind"`
}

type migrationDirectWithoutURL struct {
	downloader.DownloadTargetClient
}

func TestMigrationDirectSourcesFixture(t *testing.T) {
	fixturePath := "../../../crates/pixiv-app/tests/fixtures/download_direct_sources.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationDirectFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	source, err := os.ReadFile("downloader.go")
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(source)
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("unexpected Go reference")
	}
	if hex.EncodeToString(digest[:]) != fixture.SourceSHA256 {
		t.Fatal("pinned downloader source changed")
	}
	update := os.Getenv("PIXIV_UPDATE_DIRECT_SOURCES_FIXTURE") == "1"
	for index := range fixture.Cases {
		c := &fixture.Cases[index]
		t.Run(c.Name, func(t *testing.T) {
			root := t.TempDir()
			cwd, err := os.Getwd()
			if err != nil {
				t.Fatal(err)
			}
			relativeRoot, err := filepath.Rel(cwd, root)
			if err != nil {
				t.Fatal(err)
			}
			expand := func(path string) string {
				return strings.ReplaceAll(strings.ReplaceAll(path, "${ROOT}", root), "${REL}", relativeRoot)
			}
			normalize := func(path string) string {
				return filepath.ToSlash(strings.ReplaceAll(strings.ReplaceAll(path, relativeRoot, "${REL}"), root, "${ROOT}"))
			}
			got := migrationDirectResult{Calls: []migrationDirectCall{}, Items: []migrationDirectItem{}, Failures: []migrationDirectFailure{}}
			ctx := &directResourceErrorContext{Context: context.Background(), wantErr: context.Canceled, done: make(chan struct{})}
			if c.CancelBefore {
				ctx.trigger()
			}
			if c.CaptureDiagnostics {
				ctx.Context = diagnostics.WithScope(ctx.Context, diagnostics.SinkFunc(func(event diagnostics.Event) {
					if event.Duration < 0 {
						t.Fatal("negative diagnostic duration")
					}
					got.Diagnostics = append(got.Diagnostics, migrationDirectDiagnostic{Module: string(event.Module), Kind: string(event.Kind), Operation: event.Operation, Count: event.Count, Reason: string(event.Reason)})
				}), diagnostics.ModulePixivCLI, 0)
			}
			save := func(kind, source string, options sdk.SaveOptions) (sdk.SavedResource, error) {
				got.Calls = append(got.Calls, migrationDirectCall{Kind: kind, Source: source, Path: normalize(options.Path)})
				outcome := "ok"
				if len(got.Calls) <= len(c.Outcomes) {
					outcome = c.Outcomes[len(got.Calls)-1]
				}
				switch outcome {
				case "typed":
					return sdk.SavedResource{}, sdk.NewError("pixiv", "SaveResource", sdk.ResourceForbidden)
				case "signed_error":
					return sdk.SavedResource{}, errors.New("GET " + source + " failed: signature=secret token=hidden HTTP/1.1 attempt 1")
				case "cancel", "deadline":
					if outcome == "deadline" {
						ctx.wantErr = context.DeadlineExceeded
					}
					ctx.trigger()
					return sdk.SavedResource{}, ctx.wantErr
				case "context_error_only":
					return sdk.SavedResource{}, context.Canceled
				}
				if err := os.MkdirAll(filepath.Dir(options.Path), 0700); err != nil {
					return sdk.SavedResource{}, err
				}
				body := []byte("synthetic-image")
				if err := os.WriteFile(options.Path, body, 0600); err != nil {
					return sdk.SavedResource{}, err
				}
				return sdk.SavedResource{Path: options.Path, Size: int64(len(body)), ContentType: "image/png"}, nil
			}
			client := &downloadSourcesStub{
				saveResource: func(_ context.Context, ref sdk.ResourceRef, options sdk.SaveOptions) (sdk.SavedResource, error) {
					return save("ref", ref.String(), options)
				},
				saveResourceURL: func(_ context.Context, source string, options sdk.SaveOptions) (sdk.SavedResource, error) {
					return save("url", source, options)
				},
			}
			var target downloader.DownloadTargetClient = client
			if c.UnsupportedURLClient {
				target = &migrationDirectWithoutURL{DownloadTargetClient: client}
			}
			request := downloader.DownloadRequest{DownloadPath: expand(c.DownloadPath)}
			if c.IgnoreArtworkOptions {
				request.FilenameTemplate = "{unsupported}"
				request.DirectoryTemplate = "../unsafe"
				request.Pages = []int{-1}
				request.Quality = "invalid"
				request.UgoiraFormat = "invalid"
			}
			report, err := (downloader.DownloadService{}).DownloadSources(ctx, target, c.Sources, request)
			if err != nil {
				got.Error = err.Error()
				got.ErrorKind = "plain"
				if errors.Is(err, context.Canceled) {
					got.ErrorKind = "canceled"
				}
				if errors.Is(err, context.DeadlineExceeded) {
					got.ErrorKind = "deadline"
				}
			}
			got.Committed = report.Committed
			got.WarningCount = len(report.Warnings)
			for _, item := range report.Items {
				for _, file := range item.Files {
					body, err := os.ReadFile(file.Path)
					if err != nil {
						t.Fatal(err)
					}
					got.Items = append(got.Items, migrationDirectItem{IllustID: item.IllustID, Title: item.Title, Author: item.Author, Type: item.Type, Path: normalize(file.Path), Page: file.Page, Bytes: file.Bytes, Body: string(body)})
				}
			}
			for _, failure := range report.Failures {
				if failure.Cause == nil {
					t.Fatal("failure has no cause")
				}
				kind, reason := "plain", ""
				var typed *sdk.Error
				if errors.As(failure.Cause, &typed) {
					kind = "sdk"
					reason = string(typed.Reason)
				}
				if errors.Is(failure.Cause, context.Canceled) {
					kind = "canceled"
				}
				if errors.Is(failure.Cause, context.DeadlineExceeded) {
					kind = "deadline"
				}
				got.Failures = append(got.Failures, migrationDirectFailure{URL: failure.URL, Type: failure.Type, Message: failure.Message, CauseMessage: failure.Cause.Error(), CauseKind: kind, CauseReason: reason, Code: failure.Code})
			}
			if update {
				c.Expected = got
				return
			}
			if !reflect.DeepEqual(got, c.Expected) {
				actual, _ := json.MarshalIndent(got, "", "  ")
				expected, _ := json.MarshalIndent(c.Expected, "", "  ")
				t.Fatalf("actual:\n%s\nexpected:\n%s", actual, expected)
			}
		})
	}
	if update {
		body, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(fixturePath, append(body, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
}
