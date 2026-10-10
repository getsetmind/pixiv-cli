package download

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type randomNativeFixture struct {
	Reference           string             `json:"reference"`
	SourceSHA256        map[string]string  `json:"source_sha256"`
	ReusedFixtureSHA256 map[string]string  `json:"reused_fixture_sha256"`
	Evidence            string             `json:"evidence"`
	Deferred            []string           `json:"deferred"`
	Cases               []randomNativeCase `json:"cases"`
}
type randomNativeCase struct {
	Mode     string             `json:"mode"`
	Expected randomNativeResult `json:"expected"`
}
type randomNativeRequest struct {
	Method        string `json:"method"`
	URL           string `json:"url"`
	Authorization string `json:"authorization"`
}
type randomNativeTyped struct {
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
}
type randomNativeResult struct {
	Request              randomNativeRequest `json:"request"`
	ConnectLine          string              `json:"connect_line"`
	ParentError          string              `json:"parent_error"`
	GoPeerClosedWithin1s bool                `json:"go_peer_closed_within_1s"`
	Result               json.RawMessage     `json:"result"`
	GoSDKObservation     randomNativeTyped   `json:"go_sdk_observation"`
}

func TestMigrationRandomNativeCancellationFixture(t *testing.T) {
	const fixturePath = "../../../../../crates/pixiv-mcp/tests/fixtures/download_random_native_cancellation.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture randomNativeFixture
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" || len(fixture.Cases) != 2 {
		t.Fatal("random native cancellation coverage changed")
	}
	for path, want := range fixture.SourceSHA256 {
		current, err := os.ReadFile(filepath.Join("../../../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		frozen, err := exec.Command("git", "-C", "../../../../..", "show", fixture.Reference+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		for _, body := range [][]byte{current, frozen} {
			sum := sha256.Sum256(body)
			if hex.EncodeToString(sum[:]) != want {
				t.Fatalf("frozen source changed: %s", path)
			}
		}
	}
	for path, want := range fixture.ReusedFixtureSHA256 {
		body, err := os.ReadFile(filepath.Join("../../../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(body)
		if hex.EncodeToString(sum[:]) != want {
			t.Fatalf("published reused fixture changed: %s", path)
		}
	}
	update := os.Getenv("PIXIV_UPDATE_RANDOM_NATIVE_CANCELLATION_FIXTURE") == "1"
	for i := range fixture.Cases {
		c := &fixture.Cases[i]
		t.Run(c.Mode, func(t *testing.T) {
			if c.Mode != "cancel" && c.Mode != "deadline" {
				t.Fatal("unknown native cancellation mode")
			}
			got := randomNativeResult{}
			got.Request, got.ConnectLine, got.ParentError, got.GoPeerClosedWithin1s, got.Result, _ = runRandomNativeCancellation(t, c.Mode, true)
			_, _, _, _, _, err := runRandomNativeCancellation(t, c.Mode, false)
			if err == nil {
				t.Fatal("actual public SDK unexpectedly succeeded")
			}
			var typed *sdk.Error
			if !errors.As(err, &typed) {
				t.Fatalf("typed SDK cause lost: %T %v", err, err)
			}
			got.GoSDKObservation = randomNativeTyped{Message: err.Error(), Product: typed.Product, Operation: typed.Operation, Reason: string(typed.Reason), Transport: string(typed.Transport), HTTPStatus: typed.HTTPStatus, RetrySafe: typed.Retry.Safe, RetryHasAfter: typed.Retry.HasAfter, Canceled: errors.Is(err, context.Canceled), Deadline: errors.Is(err, context.DeadlineExceeded)}
			if cause := typed.Unwrap(); cause != nil {
				got.GoSDKObservation.CauseMessage = cause.Error()
			}
			if update {
				c.Expected = got
				return
			}
			var actualResult, expectedResult any
			if err := json.Unmarshal(got.Result, &actualResult); err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(c.Expected.Result, &expectedResult); err != nil {
				t.Fatal(err)
			}
			actualFields, expectedFields := got, c.Expected
			actualFields.Result, expectedFields.Result = nil, nil
			if !reflect.DeepEqual(actualFields, expectedFields) || !reflect.DeepEqual(actualResult, expectedResult) {
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

func runRandomNativeCancellation(t *testing.T, mode string, handler bool) (randomNativeRequest, string, string, bool, json.RawMessage, error) {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	if err = listener.(*net.TCPListener).SetDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatal(err)
	}
	proxy, err := url.Parse("http://" + listener.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	native := &http.Transport{Proxy: http.ProxyURL(proxy)}
	defer native.CloseIdleConnections()
	var request randomNativeRequest
	httpClient := &http.Client{Transport: directTransport(func(r *http.Request) (*http.Response, error) {
		request = randomNativeRequest{r.Method, r.URL.String(), r.Header.Get("Authorization")}
		return native.RoundTrip(r)
	})}
	client, err := pixiv.NewWith("fixture-access-43", pixiv.Options{HTTPClient: httpClient})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	if mode == "deadline" {
		cancel()
		ctx, cancel = context.WithTimeout(context.Background(), 500*time.Millisecond)
	}
	defer cancel()
	type completed struct {
		result json.RawMessage
		err    error
	}
	done := make(chan completed, 1)
	go func() {
		if handler {
			app := runtime.NewApp(nil, func(*pixiv.Client) runtime.DownloadManager {
				panic("media factory reached before canceled recommendation")
			}, runtime.SDKPorts{OpenLease: func(context.Context, runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
				return lifecycle.NewLease(client, nil), nil
			}}, runtime.Account{})
			result, out, err := handleDownloadRandom(ctx, app, downloadRandomIn{})
			if err != nil {
				done <- completed{err: err}
				return
			}
			raw, err := json.Marshal(map[string]any{"content": result.Content, "structuredContent": out, "isError": result.IsError})
			done <- completed{raw, err}
			return
		}
		_, err := client.RecommendedArtworks(ctx, pixiv.RecommendedArtworksRequest{})
		done <- completed{err: err}
	}()
	connection, err := listener.Accept()
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	defer connection.Close()
	if err = connection.SetReadDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatal(err)
	}
	connect, err := http.ReadRequest(bufio.NewReader(connection))
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	line := fmt.Sprintf("%s %s %s", connect.Method, connect.RequestURI, connect.Proto)
	if line != "CONNECT app-api.pixiv.net:443 HTTP/1.1" {
		cancel()
		t.Fatalf("unexpected CONNECT target: %s", line)
	}
	if mode == "cancel" {
		cancel()
	}
	var result completed
	select {
	case result = <-done:
	case <-time.After(3 * time.Second):
		cancel()
		t.Fatal("native cancellation operation exceeded bounded wait")
	}
	if handler && result.err != nil {
		t.Fatal(result.err)
	}
	if ctx.Err() == nil {
		t.Fatal("parent Context was not canceled")
	}
	peerClosed := false
	if handler {
		if err = connection.SetReadDeadline(time.Now().Add(time.Second)); err != nil {
			t.Fatal(err)
		}
		var one [1]byte
		count, readErr := connection.Read(one[:])
		peerClosed = count == 0 && errors.Is(readErr, io.EOF)
		if !peerClosed {
			var timeout net.Error
			if count != 0 || !errors.As(readErr, &timeout) || !timeout.Timeout() {
				t.Fatalf("unexpected native peer observation: n=%d err=%v", count, readErr)
			}
		}
	}
	return request, line, ctx.Err().Error(), peerClosed, result.result, result.err
}
