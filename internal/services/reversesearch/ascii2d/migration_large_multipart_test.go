package ascii2d

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"image"
	"image/color"
	"image/png"
	"io"
	"mime/multipart"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	reversesearch "github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
)

var migrationCaptureASCII2DLarge = flag.String("capture-reverse-ascii2d-large-multipart", "", "write owned large multipart observations")

type migrationLargeContextKey struct{}

type migrationLargeTransport struct {
	roundTrip func(*http.Request) (*http.Response, error)
	idle      int
}

func (m *migrationLargeTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	return m.roundTrip(r)
}

func (m *migrationLargeTransport) CloseIdleConnections() { m.idle++ }

type migrationLargeBody struct {
	io.Reader
	closes *int
}

func (b *migrationLargeBody) Close() error { *b.closes++; return nil }

func migrationLargeSHA(data []byte) string {
	digest := sha256.Sum256(data)
	return hex.EncodeToString(digest[:])
}

func migrationLargeMultipart(t *testing.T, request *http.Request, imageBytes []byte, token string) map[string]any {
	t.Helper()
	_, info, boundary := migrationASCII2DMultipartMetadata(t, request.Header.Get("Content-Type"))
	data, err := io.ReadAll(request.Body)
	if err != nil {
		t.Fatal(err)
	}
	if err := request.Body.Close(); err != nil {
		t.Fatal(err)
	}
	reader := multipart.NewReader(bytes.NewReader(data), boundary)
	parts := []map[string]any{}
	for index, expected := range [][]byte{[]byte(token), imageBytes} {
		part, err := reader.NextPart()
		if err != nil {
			t.Fatal(err)
		}
		payload, err := io.ReadAll(part)
		if err != nil || !bytes.Equal(payload, expected) {
			t.Fatalf("multipart part %d did not retain the complete owned bytes: %v", index, err)
		}
		parts = append(parts, map[string]any{"name": part.FormName(), "filename": part.FileName(), "content_type": part.Header.Get("Content-Type"), "headers": http.Header(part.Header), "size": len(payload), "sha256": migrationLargeSHA(payload)})
		if err := part.Close(); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := reader.NextPart(); !errors.Is(err, io.EOF) {
		t.Fatalf("multipart has extra or incomplete parts: %v", err)
	}
	var expected bytes.Buffer
	expected.WriteString("--" + boundary + "\r\nContent-Disposition: form-data; name=\"authenticity_token\"\r\n\r\n")
	expected.WriteString(token)
	expected.WriteString("\r\n--" + boundary + "\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\n")
	expected.Write(imageBytes)
	expected.WriteString("\r\n--" + boundary + "--\r\n")
	if !bytes.Equal(data, expected.Bytes()) {
		t.Fatal("complete framing, raw headers, ordered parts, or payload bytes changed")
	}
	oldLimit := 10*1024*1024 + 64*1024
	if len(data) <= oldLimit {
		t.Fatal("owned large token must cross the previous native multipart cap")
	}
	canonical := bytes.ReplaceAll(data, []byte(boundary), []byte("<GENERATED_BOUNDARY>"))
	return map[string]any{"media_type": info.MediaType, "parameters": info.Parameters, "boundary_length": info.BoundaryLength, "boundary_decoded_length": info.BoundaryDecodedLength, "boundary_lower_hex": info.BoundaryLowerHex, "wire_size": len(data), "canonical_sha256": migrationLargeSHA(canonical), "complete_framing": true, "image_matches_snapshot": true, "token_matches_form": true, "exceeds_old_native_limit": true, "parts": parts}
}

func TestMigrationASCII2DLargeMultipartFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..")
	for path, expected := range migrationASCII2DSources {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil || migrationLargeSHA(data) != expected {
			t.Fatalf("frozen production source changed: %s (%v)", path, err)
		}
	}
	owned := image.NewNRGBA(image.Rect(0, 0, 1, 1))
	owned.SetNRGBA(0, 0, color.NRGBA{R: 17, G: 34, B: 51, A: 255})
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, owned); err != nil {
		t.Fatal(err)
	}
	imageBytes := make([]byte, MaxImageBytes)
	copy(imageBytes, encoded.Bytes())
	token := strings.Repeat("T", 131072)
	endpoint := "https://ascii2d.invalid"
	home := strings.Replace(migrationASCII2DForm, "fixture-csrf", token, 1)
	rows := []map[string]any{}
	for _, name := range []string{"success", "cancel-after-complete-upload", "pre-canceled-upload"} {
		t.Run(name, func(t *testing.T) {
			directory := t.TempDir()
			source := filepath.Join(directory, "owned-image.png")
			if err := os.WriteFile(source, imageBytes, 0600); err != nil {
				t.Fatal(err)
			}
			snapshot, err := reversesearch.NewSourceLoader(reversesearch.SourceLoaderOptions{TempDir: directory}).Load(context.Background(), source)
			if err != nil {
				t.Fatal(err)
			}
			defer snapshot.Close()
			if err := os.WriteFile(source, []byte("changed original after snapshot"), 0600); err != nil {
				t.Fatal(err)
			}
			base, cancel := context.WithCancel(context.Background())
			defer cancel()
			ctx := context.WithValue(base, migrationLargeContextKey{}, "owned-large-multipart")
			if name == "pre-canceled-upload" {
				cancel()
			}
			requests := []map[string]any{}
			closes := []*int{}
			transport := &migrationLargeTransport{}
			transport.roundTrip = func(request *http.Request) (*http.Response, error) {
				if request.Context() != ctx {
					t.Fatal("caller context identity was not retained")
				}
				var multipartObservation any
				if request.Method == http.MethodPost {
					multipartObservation = migrationLargeMultipart(t, request, imageBytes, token)
				}
				requests = append(requests, map[string]any{"method": request.Method, "url": request.URL.String(), "path": request.URL.Path, "body_nil": request.Body == nil, "unknown_length": request.Body != nil && request.ContentLength == 0, "context_same": request.Context() == ctx, "context_value": request.Context().Value(migrationLargeContextKey{}), "context_canceled": errors.Is(request.Context().Err(), context.Canceled), "multipart": multipartObservation})
				if request.Method == http.MethodPost && name == "cancel-after-complete-upload" {
					cancel()
					return nil, ctx.Err()
				}
				status, responseText := http.StatusOK, home
				headers := make(http.Header)
				switch {
				case request.Method == http.MethodPost && request.URL.Path == "/search/file":
					status, responseText = http.StatusSeeOther, ""
					headers.Set("Location", "/search/color/"+migrationASCII2DHash)
				case request.Method == http.MethodGet && request.URL.Path == "/search/color/"+migrationASCII2DHash:
					responseText = migrationASCII2DResults
				case request.Method != http.MethodGet || request.URL.Path != "":
					t.Fatalf("unexpected provider request: %s %s", request.Method, request.URL)
				}
				closed := new(int)
				closes = append(closes, closed)
				return &http.Response{StatusCode: status, Header: headers, Body: &migrationLargeBody{Reader: strings.NewReader(responseText), closes: closed}, ContentLength: int64(len(responseText)), Request: request}, nil
			}
			client, err := New(Options{HTTPClient: &http.Client{Transport: transport}, Endpoint: endpoint})
			if err != nil {
				t.Fatal(err)
			}
			session, uploadErr := client.Upload(ctx, snapshot)
			var search any
			if session != nil {
				result, err := session.Search(ctx, reversesearch.ProviderASCII2DColor)
				if err != nil {
					t.Fatal(err)
				}
				search = map[string]any{"provider": result.Provider, "matches": result.Matches}
			}
			if name == "success" && (session == nil || uploadErr != nil) {
				t.Fatalf("legal maximum image and uncapped token rejected: %v", uploadErr)
			}
			if name != "success" && (session != nil || !errors.Is(uploadErr, context.Canceled)) {
				t.Fatalf("cancellation did not retain the public contract: %v", uploadErr)
			}
			var errorObservation any
			if uploadErr != nil {
				errorObservation = map[string]any{"message": uploadErr.Error(), "canceled": errors.Is(uploadErr, context.Canceled)}
			}
			reader, err := snapshot.Open()
			if err != nil {
				t.Fatal(err)
			}
			retained, err := io.ReadAll(reader)
			if closeErr := reader.Close(); err != nil || closeErr != nil || !bytes.Equal(retained, imageBytes) {
				t.Fatalf("snapshot ownership changed: %v %v", err, closeErr)
			}
			if err := client.Close(); err != nil {
				t.Fatal(err)
			}
			if err := client.Close(); err != nil || transport.idle != 1 {
				t.Fatalf("public client close must release the shared transport once: %v %d", err, transport.idle)
			}
			closeCounts := []int{}
			for _, closed := range closes {
				if *closed != 1 {
					t.Fatalf("response body close count: %d", *closed)
				}
				closeCounts = append(closeCounts, *closed)
			}
			if err := snapshot.Close(); err != nil {
				t.Fatal(err)
			}
			if err := snapshot.Close(); err != nil {
				t.Fatal(err)
			}
			_, closedErr := snapshot.Open()
			if closedErr == nil {
				t.Fatal("closed snapshot could still be opened")
			}
			rows = append(rows, map[string]any{"name": name, "requests": requests, "session": session != nil, "error": errorObservation, "search": search, "snapshot_size": snapshot.Size(), "snapshot_sha256": snapshot.SHA256(), "snapshot_bytes_retained": bytes.Equal(retained, imageBytes), "snapshot_closed_error": closedErr.Error(), "response_close_counts": closeCounts, "idle_closes": transport.idle})
		})
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{"schema": 1, "source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "repository_head": "71d798690ac0e80942803641d9d7e5129d0e749b", "go_version": runtime.Version(), "source_sha256": migrationASCII2DSources, "input": map[string]any{"image_prefix_hex": hex.EncodeToString(encoded.Bytes()), "image_size": len(imageBytes), "image_sha256": migrationLargeSHA(imageBytes), "token_byte": "T", "token_size": len(token), "token_sha256": migrationLargeSHA([]byte(token)), "endpoint": endpoint, "home_template": migrationASCII2DForm, "result_html": migrationASCII2DResults, "hash": migrationASCII2DHash}, "cases": rows, "evidence": "Actual unchanged frozen Go public Loader/New/Upload/Session.Search/Close with an owned generated PNG padded to exactly 10 MiB and a 131072-byte synthetic CSRF token. Injected http.RoundTripper consumes every multipart byte before returning; only generated boundary tokens are canonicalized. Source file changes after Load, while snapshot and uploaded image retain exact original bytes. Caller context identity/value, cancellation, response body closure, snapshot closure and one-time idle closure are observed.", "limitations": []string{"Synthetic dependency responses only; no external image, provider request, browser, credential, account or native wire probe.", "The Rust NativeHttpTransport/SDK RawTransport comparison exercises the genuine body Vec boundary, not native TLS/fingerprint or HTTP wire equivalence. Go reader content length zero maps to SDK unknown length minus one."}}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "reverse-ascii2d-large-multipart.json")
	if *migrationCaptureASCII2DLarge != "" {
		if err := os.WriteFile(*migrationCaptureASCII2DLarge, data, 0644); err != nil {
			t.Fatal(err)
		}
	} else {
		want, err := os.ReadFile(path)
		if err != nil || !bytes.Equal(data, want) {
			t.Fatalf("actual frozen large multipart differs from new fixture: %v", err)
		}
	}
	t.Logf("large multipart public cases=%d image_bytes=%d token_bytes=%d image_sha256=%s token_sha256=%s", len(rows), len(imageBytes), len(token), migrationLargeSHA(imageBytes), migrationLargeSHA([]byte(token)))
}
