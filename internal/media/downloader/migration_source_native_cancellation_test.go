package downloader_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationExpansionRoundTrip func(*http.Request) (*http.Response, error)

func (f migrationExpansionRoundTrip) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationNativeListCancellationFixture struct {
	Reference    string                                `json:"reference"`
	SourceSHA256 map[string]string                     `json:"source_sha256"`
	Evidence     string                                `json:"evidence"`
	Deferred     []string                              `json:"deferred"`
	Cases        []migrationNativeListCancellationCase `json:"cases"`
}
type migrationNativeListCancellationCase struct {
	Operation string                                `json:"operation"`
	Mode      string                                `json:"mode"`
	Expected  migrationNativeListCancellationResult `json:"expected"`
}
type migrationNativeListCancellationResult struct {
	Method        string `json:"method"`
	URL           string `json:"url"`
	Message       string `json:"message"`
	Product       string `json:"product"`
	Operation     string `json:"operation"`
	Reason        string `json:"reason"`
	Transport     string `json:"transport"`
	HTTPStatus    int    `json:"http_status"`
	RetrySafe     bool   `json:"retry_safe"`
	RetryHasAfter bool   `json:"retry_has_after"`
	CauseMessage  string `json:"cause_message"`
	Canceled      bool   `json:"canceled"`
	Deadline      bool   `json:"deadline"`
	ParentError   string `json:"parent_error"`
}

func TestMigrationNativeListCancellationFixture(t *testing.T) {
	const fixturePath = "../../../crates/pixiv-app/tests/fixtures/download_source_native_cancellation.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationNativeListCancellationFixture
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" || len(fixture.Cases) != 4 {
		t.Fatal("native list cancellation frozen coverage changed")
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
				t.Fatalf("pinned source changed: %s", path)
			}
		}
	}
	update := os.Getenv("PIXIV_UPDATE_SOURCE_NATIVE_CANCELLATION_FIXTURE") == "1"
	for i := range fixture.Cases {
		c := &fixture.Cases[i]
		t.Run(c.Operation+"_"+c.Mode, func(t *testing.T) {
			got := migrationNativeListCancellationResult{}
			ctx, cancel := context.WithCancel(context.Background())
			if c.Mode == "deadline" {
				cancel()
				ctx, cancel = context.WithTimeout(context.Background(), 5*time.Millisecond)
			}
			defer cancel()
			client, err := pixiv.NewWith("synthetic-token", pixiv.Options{HTTPClient: &http.Client{Transport: migrationExpansionRoundTrip(func(r *http.Request) (*http.Response, error) {
				got.Method = r.Method
				got.URL = r.URL.String()
				if c.Mode == "cancel" {
					cancel()
				}
				<-r.Context().Done()
				return nil, r.Context().Err()
			})}})
			if err != nil {
				t.Fatal(err)
			}
			defer client.CloseIdleConnections()
			switch c.Operation {
			case "UserArtworks":
				_, err = client.UserArtworks(ctx, pixiv.UserArtworksRequest{UserID: 7, Kind: pixiv.ArtworkKindIllustration})
			case "UserArtworkBookmarks":
				_, err = client.UserArtworkBookmarks(ctx, pixiv.UserArtworkBookmarksRequest{UserID: 7, Restrict: pixiv.RestrictPublic})
			default:
				t.Fatal("unknown operation")
			}
			if err == nil {
				t.Fatal("expected actual SDK cancellation error")
			}
			got.Message = err.Error()
			got.Canceled = errors.Is(err, context.Canceled)
			got.Deadline = errors.Is(err, context.DeadlineExceeded)
			got.ParentError = ctx.Err().Error()
			var typed *sdk.Error
			if !errors.As(err, &typed) {
				t.Fatalf("lost typed SDK failure: %T %v", err, err)
			}
			got.Product = typed.Product
			got.Operation = typed.Operation
			got.Reason = string(typed.Reason)
			got.Transport = string(typed.Transport)
			got.HTTPStatus = typed.HTTPStatus
			got.RetrySafe = typed.Retry.Safe
			got.RetryHasAfter = typed.Retry.HasAfter
			if cause := typed.Unwrap(); cause != nil {
				got.CauseMessage = cause.Error()
			}
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
