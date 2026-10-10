package reversesearch_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"sync"
	"testing"
	"time"

	rs "github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
)

type migrationReverseRow struct {
	Name      string         `json:"name"`
	Operation string         `json:"operation"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}
type migrationReverseFixture struct {
	Reference   string                `json:"reference"`
	Environment string                `json:"environment"`
	GoVersion   string                `json:"go_version"`
	Sources     map[string]string     `json:"sources"`
	Boundaries  map[string]string     `json:"boundaries"`
	Cases       []migrationReverseRow `json:"cases"`
}

func migrationReverseError(err error) map[string]any {
	m := map[string]any{"message": "", "code": "", "canceled": false, "deadline": false}
	if err != nil {
		m["message"] = err.Error()
		m["code"] = string(rs.CodeOf(err))
		m["canceled"] = errors.Is(err, context.Canceled)
		m["deadline"] = errors.Is(err, context.DeadlineExceeded)
	}
	return m
}
func migrationReverseContext(mode string) (context.Context, context.CancelFunc) {
	if mode == "nil" {
		return nil, func() {}
	}
	if mode == "canceled" {
		c, cancel := context.WithCancel(context.Background())
		cancel()
		return c, cancel
	}
	if mode == "deadline" {
		return context.WithDeadline(context.Background(), time.Unix(1, 0))
	}
	return context.WithCancel(context.Background())
}
func migrationReverseJSON(t *testing.T, v any) any {
	t.Helper()
	b, e := json.Marshal(v)
	if e != nil {
		t.Fatal(e)
	}
	var out any
	decoder := json.NewDecoder(bytes.NewReader(b))
	decoder.UseNumber()
	if e = decoder.Decode(&out); e != nil {
		t.Fatal(e)
	}
	return out
}
func migrationReverseHex(b []byte) string { return hex.EncodeToString(b) }

type migrationReverseBody struct {
	body         []byte
	mode         string
	read, closed int
	cancel       context.CancelFunc
}

func (b *migrationReverseBody) Read(p []byte) (int, error) {
	b.read++
	if len(b.body) > 0 {
		n := copy(p, b.body)
		b.body = b.body[n:]
		if b.mode == "cancel-with-bytes" {
			b.cancel()
			return n, io.EOF
		}
		if b.mode == "error-with-bytes" {
			return n, errors.New("owned body read error")
		}
		if b.mode == "cancel-between" {
			b.cancel()
		}
		return n, nil
	}
	if b.mode == "read-error" {
		return 0, errors.New("owned body read error")
	}
	return 0, io.EOF
}
func (b *migrationReverseBody) Close() error { b.closed++; return nil }

func migrationReverseSourceRows(t *testing.T) []migrationReverseRow {
	cases := []struct {
		name, source, context, bodyMode, state string
		status                                 int
		payload                                []byte
	}{
		{name: "file-opaque-bytes", source: "file", payload: []byte{0, 255, 254, 13, 10, 34}},
		{name: "file-empty", source: "file"}, {name: "file-large-stream", source: "file", payload: bytes.Repeat([]byte("owned"), 14000)},
		{name: "file-missing", source: "file", state: "missing"}, {name: "file-directory", source: "file", state: "directory"}, {name: "file-symlink", source: "file", state: "symlink", payload: []byte("owned-link")},
		{name: "file-pre-canceled", source: "file", context: "canceled", payload: []byte("owned")}, {name: "file-deadline", source: "file", context: "deadline"}, {name: "file-nil-context", source: "file", context: "nil"},
		{name: "file-missing-tempdir", source: "file", state: "bad-temp", payload: []byte("owned")},
		{name: "unsupported-source", source: "ftp://source.test/image"}, {name: "empty-source", source: ""}, {name: "relative-missing", source: "missing-owned-image"},
		{name: "url-opaque-bytes", source: "http://source.test/image", payload: []byte{0, 255, 13, 10, 34}},
		{name: "url-empty", source: "https://source.test/image"}, {name: "url-upper-scheme", source: "HTTPS://source.test/image", payload: []byte("owned")},
		{name: "url-userinfo", source: "http://owned:synthetic@source.test/image"}, {name: "url-malformed", source: "http://%zz"}, {name: "url-empty-host", source: "http:///image"},
		{name: "url-http-status-199", source: "http://source.test/image", status: 199}, {name: "url-http-status-204", source: "http://source.test/image", status: 204}, {name: "url-http-status-299", source: "http://source.test/image", status: 299}, {name: "url-http-status-404", source: "http://source.test/image", status: 404},
		{name: "url-transport-error", source: "http://source.test/image", state: "transport-error"}, {name: "url-final-userinfo", source: "http://source.test/image", state: "final-userinfo"}, {name: "url-final-unsupported", source: "http://source.test/image", state: "final-unsupported"}, {name: "url-final-request-nil", source: "http://source.test/image", state: "request-nil"},
		{name: "url-pre-canceled", source: "http://source.test/image", context: "canceled"}, {name: "url-transport-cancels", source: "http://source.test/image", state: "transport-cancel"}, {name: "url-read-error", source: "http://source.test/image", bodyMode: "read-error"},
		{name: "url-error-with-prefix", source: "http://source.test/image", bodyMode: "error-with-bytes", payload: []byte("owned-prefix")}, {name: "url-cancel-between-reads", source: "http://source.test/image", bodyMode: "cancel-between", payload: []byte("owned-prefix")}, {name: "url-cancel-with-final-eof", source: "http://source.test/image", bodyMode: "cancel-with-bytes", payload: []byte("owned-prefix")},
	}
	rows := []migrationReverseRow{}
	for _, c := range cases {
		t.Run("source/"+c.name, func(t *testing.T) {
			root := t.TempDir()
			temp := filepath.Join(root, "snapshots")
			if e := os.Mkdir(temp, 0700); e != nil {
				t.Fatal(e)
			}
			source := c.source
			if source == "file" {
				source = filepath.Join(root, "source.bin")
				switch c.state {
				case "missing":
				case "directory":
					if e := os.Mkdir(source, 0700); e != nil {
						t.Fatal(e)
					}
				case "symlink":
					target := filepath.Join(root, "target.bin")
					if e := os.WriteFile(target, c.payload, 0600); e != nil {
						t.Fatal(e)
					}
					if e := os.Symlink(target, source); e != nil {
						t.Fatal(e)
					}
				default:
					if e := os.WriteFile(source, c.payload, 0600); e != nil {
						t.Fatal(e)
					}
				}
			}
			if c.state == "bad-temp" {
				temp = filepath.Join(root, "missing", "snapshots")
			}
			ctx, cancel := migrationReverseContext(c.context)
			defer cancel()
			body := &migrationReverseBody{body: append([]byte(nil), c.payload...), mode: c.bodyMode, cancel: cancel}
			requests := []map[string]any{}
			client := &http.Client{Transport: roundTripFunc(func(req *http.Request) (*http.Response, error) {
				requests = append(requests, map[string]any{"method": req.Method, "url": req.URL.String(), "headers": req.Header})
				if c.state == "transport-cancel" {
					cancel()
					return nil, errors.New("owned transport error")
				}
				if c.state == "transport-error" {
					return nil, errors.New("owned transport error")
				}
				status := c.status
				if status == 0 {
					status = 200
				}
				request := req
				if c.state == "request-nil" {
					request = nil
				}
				if c.state == "final-userinfo" || c.state == "final-unsupported" {
					raw := "http://owned:synthetic@source.test/final"
					if c.state == "final-unsupported" {
						raw = "ftp://source.test/final"
					}
					u, e := url.Parse(raw)
					if e != nil {
						t.Fatal(e)
					}
					request = &http.Request{URL: u}
				}
				return &http.Response{StatusCode: status, Header: make(http.Header), Body: body, Request: request}, nil
			})}
			loader := rs.NewSourceLoader(rs.SourceLoaderOptions{TempDir: temp, HTTPClient: client})
			snapshot, err := loader.Load(ctx, source)
			out := map[string]any{"error": migrationReverseError(err), "requests": requests, "body_reads": body.read, "body_closes": body.closed, "snapshot": nil, "temporary_files_after": 0}
			if snapshot != nil {
				files, e := os.ReadDir(temp)
				if e != nil {
					t.Fatal(e)
				}
				if len(files) != 1 {
					t.Fatalf("owned snapshot count %d", len(files))
				}
				info, e := files[0].Info()
				if e != nil {
					t.Fatal(e)
				}
				if c.source == "file" {
					if e = os.WriteFile(source, []byte("owned-changed"), 0600); e != nil {
						t.Fatal(e)
					}
					changed, e := os.ReadFile(source)
					if e != nil {
						t.Fatal(e)
					}
					out["source_bytes_before_snapshot_reads_hex"] = migrationReverseHex(changed)
				}
				reads := []string{}
				for range 2 {
					r, e := snapshot.Open()
					if e != nil {
						t.Fatal(e)
					}
					b, e := io.ReadAll(r)
					if e != nil {
						t.Fatal(e)
					}
					if c.source == "file" && !bytes.Equal(b, c.payload) {
						t.Fatal("source mutation changed snapshot bytes")
					}
					if e = r.Close(); e != nil {
						t.Fatal(e)
					}
					reads = append(reads, migrationReverseHex(b))
				}
				out["snapshot"] = map[string]any{"kind": snapshot.Kind(), "sha256": snapshot.SHA256(), "size": snapshot.Size(), "mode": fmt.Sprintf("%04o", info.Mode().Perm()), "reads_hex": reads, "close_error": migrationReverseError(snapshot.Close()), "repeat_close_error": migrationReverseError(snapshot.Close())}
				r, openErr := snapshot.Open()
				if r != nil {
					_ = r.Close()
				}
				out["after_close_open_error"] = migrationReverseError(openErr)
			}
			entries, e := os.ReadDir(temp)
			if e == nil {
				out["temporary_files_after"] = len(entries)
			} else if !os.IsNotExist(e) {
				t.Fatal(e)
			}
			rows = append(rows, migrationReverseRow{Name: c.name, Operation: "source_load", Input: map[string]any{"source": c.source, "context": c.context, "body_mode": c.bodyMode, "state": c.state, "status": c.status, "payload_hex": migrationReverseHex(c.payload)}, Output: out})
		})
	}
	return rows
}

func migrationReverseRedirectRows(t *testing.T) []migrationReverseRow {
	cases := []struct {
		name, nextURL, caller string
		redirects             int
	}{
		{name: "allowed-chain-final", redirects: 2},
		{name: "invalid-next-userinfo", redirects: 1, nextURL: "http://owned:synthetic@source.test/final"},
		{name: "default-ten-redirect-cap", redirects: 12},
		{name: "caller-failure-before-validation", redirects: 1, nextURL: "http://owned:synthetic@source.test/final", caller: "failure"},
		{name: "caller-allows-beyond-default-cap", redirects: 12, caller: "allow"},
	}
	rows := []migrationReverseRow{}
	for _, c := range cases {
		t.Run("redirect/"+c.name, func(t *testing.T) {
			dir := t.TempDir()
			payload := []byte("owned redirected payload")
			requests, hooks := []map[string]any{}, []map[string]any{}
			bodies := []*migrationReverseBody{}
			callerErr := errors.New("owned caller redirect failure")
			client := &http.Client{Transport: roundTripFunc(func(request *http.Request) (*http.Response, error) {
				requests = append(requests, map[string]any{"method": request.Method, "url": request.URL.String(), "headers": request.Header.Clone()})
				status, header, content := http.StatusOK, make(http.Header), payload
				if len(requests) <= c.redirects {
					status, content = http.StatusFound, []byte("owned redirect response")
					location := fmt.Sprintf("http://source.test/hop/%d", len(requests))
					if c.nextURL != "" {
						location = c.nextURL
					}
					header.Set("Location", location)
				}
				body := &migrationReverseBody{body: append([]byte(nil), content...)}
				bodies = append(bodies, body)
				return &http.Response{StatusCode: status, Header: header, Body: body, Request: request}, nil
			})}
			if c.caller != "" {
				client.CheckRedirect = func(next *http.Request, via []*http.Request) error {
					prior := []string{}
					for _, request := range via {
						prior = append(prior, request.URL.String())
					}
					hooks = append(hooks, map[string]any{"next_url": next.URL.String(), "via": prior})
					if c.caller == "failure" {
						return callerErr
					}
					return nil
				}
			}
			snapshot, err := rs.NewSourceLoader(rs.SourceLoaderOptions{TempDir: dir, HTTPClient: client}).Load(context.Background(), "http://source.test/hop/0")
			chain := []map[string]any{}
			for cause := err; cause != nil; cause = errors.Unwrap(cause) {
				observed := migrationReverseError(cause)
				observed["type"] = fmt.Sprintf("%T", cause)
				chain = append(chain, observed)
			}
			out := map[string]any{"error": migrationReverseError(err), "error_chain": chain, "is_caller_error": errors.Is(err, callerErr), "requests": requests, "caller_hooks": hooks, "snapshot": nil}
			if snapshot != nil {
				reads := []string{}
				for range 2 {
					reader, openErr := snapshot.Open()
					if openErr != nil {
						t.Fatal(openErr)
					}
					body, readErr := io.ReadAll(reader)
					if readErr != nil {
						t.Fatal(readErr)
					}
					if closeErr := reader.Close(); closeErr != nil {
						t.Fatal(closeErr)
					}
					reads = append(reads, migrationReverseHex(body))
				}
				out["snapshot"] = map[string]any{"kind": snapshot.Kind(), "sha256": snapshot.SHA256(), "size": snapshot.Size(), "reads_hex": reads, "close_error": migrationReverseError(snapshot.Close())}
			}
			observedBodies := []map[string]any{}
			for _, body := range bodies {
				observedBodies = append(observedBodies, map[string]any{"reads": body.read, "closes": body.closed})
			}
			out["response_bodies"] = observedBodies
			entries, readErr := os.ReadDir(dir)
			if readErr != nil {
				t.Fatal(readErr)
			}
			out["temporary_files_after"] = len(entries)
			rows = append(rows, migrationReverseRow{Name: c.name, Operation: "source_redirect", Input: map[string]any{"source": "http://source.test/hop/0", "redirects": c.redirects, "next_url": c.nextURL, "caller": c.caller, "payload_hex": migrationReverseHex(payload)}, Output: out})
		})
	}
	return rows
}

func migrationReverseSnapshotPath(t *testing.T, dir string) string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || !entries[0].Type().IsRegular() {
		t.Fatalf("expected one owned regular snapshot, got %v", entries)
	}
	return filepath.Join(dir, entries[0].Name())
}

func migrationReverseBlockSnapshotClose(t *testing.T, dir string) func() {
	t.Helper()
	path := migrationReverseSnapshotPath(t, dir)
	backup := filepath.Join(filepath.Dir(dir), "owned-snapshot-backup.bin")
	if err := os.Rename(path, backup); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(path, 0700); err != nil {
		t.Fatal(err)
	}
	child := filepath.Join(path, "owned-blocker.bin")
	if err := os.WriteFile(child, []byte("owned"), 0600); err != nil {
		t.Fatal(err)
	}
	restored := false
	restore := func() {
		t.Helper()
		if restored {
			return
		}
		for _, remove := range []string{child, path} {
			if err := os.Remove(remove); err != nil {
				t.Fatal(err)
			}
		}
		if err := os.Rename(backup, path); err != nil {
			t.Fatal(err)
		}
		restored = true
	}
	t.Cleanup(restore)
	return restore
}

func migrationReversePathCause(err error) any {
	var cause *os.PathError
	if !errors.As(err, &cause) {
		return nil
	}
	return map[string]any{"operation": cause.Op, "error": cause.Err.Error(), "type": fmt.Sprintf("%T", cause.Err), "platform": runtime.GOOS}
}

func migrationReverseJoinedErrors(err error) any {
	joined, ok := err.(interface{ Unwrap() []error })
	if !ok {
		return nil
	}
	children := []map[string]any{}
	for _, child := range joined.Unwrap() {
		children = append(children, migrationReverseError(child))
	}
	return children
}

func migrationReverseSnapshotRows(t *testing.T) []migrationReverseRow {
	rows := []migrationReverseRow{}
	for _, name := range []string{"close-failure-retry", "linux-reader-held-across-close"} {
		t.Run("snapshot/"+name, func(t *testing.T) {
			if name == "linux-reader-held-across-close" && runtime.GOOS != "linux" {
				t.Skip("held-reader unlink observation is Linux-specific")
			}
			root := t.TempDir()
			dir := filepath.Join(root, "snapshots")
			if err := os.Mkdir(dir, 0700); err != nil {
				t.Fatal(err)
			}
			payload := []byte("owned retained snapshot")
			source := filepath.Join(root, "owned.bin")
			if err := os.WriteFile(source, payload, 0600); err != nil {
				t.Fatal(err)
			}
			snapshot, err := rs.NewSourceLoader(rs.SourceLoaderOptions{TempDir: dir}).Load(context.Background(), source)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = snapshot.Close() })
			out := map[string]any{"kind": snapshot.Kind(), "sha256": snapshot.SHA256(), "size": snapshot.Size()}
			if name == "close-failure-retry" {
				restore := migrationReverseBlockSnapshotClose(t, dir)
				closeErr := snapshot.Close()
				out["close_first"] = migrationReverseError(closeErr)
				out["close_cause"] = migrationReversePathCause(closeErr)
				out["close_second_before_restore"] = migrationReverseError(snapshot.Close())
				restore()
				reader, openErr := snapshot.Open()
				out["open_after_failure"] = migrationReverseError(openErr)
				if openErr != nil {
					t.Fatal(openErr)
				}
				body, readErr := io.ReadAll(reader)
				out["read_after_failure_hex"] = migrationReverseHex(body)
				out["read_after_failure_error"] = migrationReverseError(readErr)
				out["reader_close"] = migrationReverseError(reader.Close())
				out["close_after_restore"] = migrationReverseError(snapshot.Close())
			} else {
				reader, openErr := snapshot.Open()
				if openErr != nil {
					t.Fatal(openErr)
				}
				out["close_with_reader_held"] = migrationReverseError(snapshot.Close())
				body, readErr := io.ReadAll(reader)
				out["read_after_close_hex"] = migrationReverseHex(body)
				out["read_after_close_error"] = migrationReverseError(readErr)
				out["reader_close"] = migrationReverseError(reader.Close())
			}
			out["close_repeat"] = migrationReverseError(snapshot.Close())
			reader, openErr := snapshot.Open()
			if reader != nil {
				_ = reader.Close()
			}
			out["open_after_close"] = migrationReverseError(openErr)
			entries, readErr := os.ReadDir(dir)
			if readErr != nil {
				t.Fatal(readErr)
			}
			out["temporary_files_after"] = len(entries)
			rows = append(rows, migrationReverseRow{Name: name, Operation: "snapshot_lifecycle", Input: map[string]any{"payload_hex": migrationReverseHex(payload), "platform": runtime.GOOS}, Output: out})
		})
	}
	return rows
}

type migrationReverseSourcePort func(context.Context, string) (*rs.Snapshot, error)

func (load migrationReverseSourcePort) Load(ctx context.Context, source string) (*rs.Snapshot, error) {
	return load(ctx, source)
}

type migrationReversePayloadPort struct {
	preflight func(context.Context, rs.PayloadQuery) error
	search    func(context.Context, rs.PayloadRequest) (rs.Response, error)
}

func (p migrationReversePayloadPort) Preflight(ctx context.Context, query rs.PayloadQuery) error {
	return p.preflight(ctx, query)
}
func (p migrationReversePayloadPort) SearchPayload(ctx context.Context, request rs.PayloadRequest) (rs.Response, error) {
	return p.search(ctx, request)
}

func migrationReverseFacadeRows(t *testing.T) []migrationReverseRow {
	cases := []struct {
		name, context, payloadError                                                 string
		nilFacade, nilSources, nilPayloads, preflightError, sourceError, blockClose bool
	}{
		{name: "nil-facade-nil-context", context: "nil", nilFacade: true},
		{name: "nil-facade", nilFacade: true},
		{name: "nil-context-before-ports", context: "nil", nilSources: true, nilPayloads: true},
		{name: "nil-sources-before-nil-payloads", nilSources: true, nilPayloads: true},
		{name: "nil-sources-before-preflight-error", nilSources: true, preflightError: true},
		{name: "nil-payloads-before-source-error", nilPayloads: true, sourceError: true},
		{name: "nil-payloads-before-cancellation", nilPayloads: true, context: "canceled"},
		{name: "preflight-error-before-source-error", preflightError: true, sourceError: true},
		{name: "source-error-after-preflight", sourceError: true},
		{name: "canceled-context-reaches-preflight", context: "canceled"},
		{name: "payload-input-overridden"},
		{name: "payload-classified-error-input-retained", payloadError: "classified"},
		{name: "cleanup-failure-after-success", blockClose: true},
		{name: "cleanup-failure-joins-classified", blockClose: true, payloadError: "classified"},
		{name: "cleanup-failure-joins-generic", blockClose: true, payloadError: "generic"},
		{name: "cleanup-failure-joins-wrapped-canceled", blockClose: true, payloadError: "wrapped-canceled"},
	}
	rows := []migrationReverseRow{}
	for _, c := range cases {
		t.Run("facade/"+c.name, func(t *testing.T) {
			root := t.TempDir()
			dir := filepath.Join(root, "snapshots")
			if err := os.Mkdir(dir, 0700); err != nil {
				t.Fatal(err)
			}
			payload := []byte("owned facade payload")
			source := filepath.Join(root, "owned.bin")
			if err := os.WriteFile(source, payload, 0600); err != nil {
				t.Fatal(err)
			}
			preflightErr := rs.NewError(rs.CodeMissingCredential, "owned payload preflight failure", errors.New("owned preflight cause"))
			sourceErr := rs.NewError(rs.CodeSourceReadFailed, "owned source failure", errors.New("owned source cause"))
			var payloadErr error
			switch c.payloadError {
			case "classified":
				payloadErr = rs.NewError(rs.CodeProviderFailed, "owned payload search failure", errors.New("owned payload cause"))
			case "generic":
				payloadErr = errors.New("owned unclassified payload failure")
			case "wrapped-canceled":
				payloadErr = fmt.Errorf("owned payload cancellation: %w", context.Canceled)
			}
			calls := []string{}
			out := map[string]any{"preflight": nil, "payload_request": nil}
			var loaded *rs.Snapshot
			var restore func()
			deps := rs.Dependencies{}
			if !c.nilSources {
				deps.Sources = migrationReverseSourcePort(func(ctx context.Context, value string) (*rs.Snapshot, error) {
					calls = append(calls, "source.load")
					out["source_matches_request"] = value == source
					if c.sourceError {
						return nil, sourceErr
					}
					var err error
					loaded, err = rs.NewSourceLoader(rs.SourceLoaderOptions{TempDir: dir}).Load(ctx, value)
					return loaded, err
				})
			}
			if !c.nilPayloads {
				deps.Payloads = migrationReversePayloadPort{
					preflight: func(ctx context.Context, query rs.PayloadQuery) error {
						calls = append(calls, "payload.preflight")
						out["preflight"] = map[string]any{"provider": query.Provider, "pixiv_only": query.PixivOnly, "context_error": migrationReverseError(ctx.Err())}
						if c.preflightError {
							return preflightErr
						}
						return nil
					},
					search: func(_ context.Context, request rs.PayloadRequest) (rs.Response, error) {
						calls = append(calls, "payload.search")
						out["payload_request"] = map[string]any{"provider": request.Provider, "pixiv_only": request.PixivOnly, "same_loaded_snapshot": request.Snapshot == loaded}
						if c.blockClose {
							restore = migrationReverseBlockSnapshotClose(t, dir)
						}
						return rs.Response{Input: rs.Input{Kind: rs.SourceKindURL, SHA256: "owned payload input"}, Partial: true}, payloadErr
					},
				}
			}
			var facade *rs.Facade
			if !c.nilFacade {
				facade = rs.NewFacade(deps)
			}
			ctx, cancel := migrationReverseContext(c.context)
			defer cancel()
			response, err := facade.Search(ctx, rs.Request{Source: source, Provider: rs.ProviderSauceNAO, PixivOnly: true})
			out["response"], out["error"], out["calls"] = migrationReverseJSON(t, response), migrationReverseError(err), calls
			out["joined_errors"], out["filesystem_cause"] = migrationReverseJoinedErrors(err), migrationReversePathCause(err)
			out["is_preflight_error"] = errors.Is(err, preflightErr)
			out["is_source_error"] = errors.Is(err, sourceErr)
			out["is_payload_error"] = payloadErr != nil && errors.Is(err, payloadErr)
			entries, readErr := os.ReadDir(dir)
			if readErr != nil {
				t.Fatal(readErr)
			}
			out["temporary_files_after_search"] = len(entries)
			if restore != nil {
				restore()
			}
			if loaded != nil {
				reader, openErr := loaded.Open()
				out["snapshot_open_after_search"] = migrationReverseError(openErr)
				if reader != nil {
					body, readErr := io.ReadAll(reader)
					out["snapshot_read_after_search_hex"] = migrationReverseHex(body)
					out["snapshot_read_after_search_error"] = migrationReverseError(readErr)
					out["snapshot_reader_close"] = migrationReverseError(reader.Close())
				}
				out["snapshot_close_retry"] = migrationReverseError(loaded.Close())
				out["snapshot_close_repeat"] = migrationReverseError(loaded.Close())
			}
			entries, readErr = os.ReadDir(dir)
			if readErr != nil {
				t.Fatal(readErr)
			}
			out["temporary_files_after_retry"] = len(entries)
			in := map[string]any{"context": c.context, "nil_facade": c.nilFacade, "nil_sources": c.nilSources, "nil_payloads": c.nilPayloads, "preflight_error": c.preflightError, "source_error": c.sourceError, "payload_error": c.payloadError, "block_snapshot_close": c.blockClose, "provider": "saucenao", "pixiv_only": true, "payload_hex": migrationReverseHex(payload)}
			rows = append(rows, migrationReverseRow{Name: c.name, Operation: "facade_lifecycle", Input: in, Output: out})
		})
	}
	return rows
}

type migrationReversePorts struct {
	mu                 sync.Mutex
	calls              map[string]int
	reads              map[string][]string
	snapshots          []*rs.Snapshot
	closeOrder         []string
	errors             map[string]string
	matches            map[string][]rs.Match
	gate               chan struct{}
	started            int
	concurrent         bool
	branchGate         chan struct{}
	branchesStarted    int
	branchesConcurrent bool
	gateTimedOut       bool
}

func (p *migrationReversePorts) call(name string) { p.mu.Lock(); defer p.mu.Unlock(); p.calls[name]++ }
func (p *migrationReversePorts) error(name string) error {
	switch p.errors[name] {
	case "generic":
		return errors.New("owned-private-provider-canary")
	case "classified":
		return rs.NewError(rs.CodeUpstreamHTTPStatus, "owned reviewed provider failure", errors.New("owned-private-cause-canary"))
	case "canceled":
		return context.Canceled
	case "deadline":
		return context.DeadlineExceeded
	case "wrapped-canceled":
		return fmt.Errorf("owned cancellation wrapper: %w", context.Canceled)
	default:
		return nil
	}
}
func (p *migrationReversePorts) read(ctx context.Context, name string, s *rs.Snapshot) error {
	p.call(name)
	if p.branchesConcurrent && (name == "sauce.search" || name == "ascii.upload") {
		p.mu.Lock()
		p.branchesStarted++
		if p.branchesStarted == 2 {
			close(p.branchGate)
		}
		p.mu.Unlock()
		select {
		case <-p.branchGate:
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(2 * time.Second):
			p.mu.Lock()
			p.gateTimedOut = true
			p.mu.Unlock()
			return errors.New("owned provider branch concurrency gate timed out")
		}
	}
	p.mu.Lock()
	p.snapshots = append(p.snapshots, s)
	p.mu.Unlock()
	for range 2 {
		r, e := s.Open()
		if e != nil {
			return e
		}
		b, e := io.ReadAll(r)
		_ = r.Close()
		if e != nil {
			return e
		}
		p.mu.Lock()
		p.reads[name] = append(p.reads[name], migrationReverseHex(b))
		p.mu.Unlock()
	}
	return p.error(name)
}

type migrationReverseSauce struct{ p *migrationReversePorts }

func (s migrationReverseSauce) Preflight(context.Context) error {
	s.p.call("sauce.preflight")
	return s.p.error("sauce.preflight")
}
func (s migrationReverseSauce) Search(ctx context.Context, snap *rs.Snapshot) (rs.ProviderResponse, error) {
	e := s.p.read(ctx, "sauce.search", snap)
	return rs.ProviderResponse{Provider: rs.ProviderSauceNAO, Matches: s.p.matches["sauce"], Quota: &rs.Quota{ShortRemaining: 3, LongRemaining: 7, ShortLimit: 4, LongLimit: 8}}, e
}
func (s migrationReverseSauce) Close() error {
	s.p.mu.Lock()
	s.p.closeOrder = append(s.p.closeOrder, "sauce")
	s.p.mu.Unlock()
	s.p.call("sauce.close")
	return s.p.error("sauce.close")
}

type migrationReverseASCII struct{ p *migrationReversePorts }

func (s migrationReverseASCII) Preflight(context.Context) error {
	s.p.call("ascii.preflight")
	return s.p.error("ascii.preflight")
}
func (s migrationReverseASCII) Upload(ctx context.Context, snap *rs.Snapshot) (rs.ASCII2DSession, error) {
	if e := s.p.read(ctx, "ascii.upload", snap); e != nil {
		return nil, e
	}
	return migrationReverseSession{s.p, snap}, nil
}
func (s migrationReverseASCII) Close() error {
	s.p.mu.Lock()
	s.p.closeOrder = append(s.p.closeOrder, "ascii")
	s.p.mu.Unlock()
	s.p.call("ascii.close")
	return s.p.error("ascii.close")
}

type migrationReverseSession struct {
	p    *migrationReversePorts
	snap *rs.Snapshot
}

func (s migrationReverseSession) Search(ctx context.Context, provider rs.Provider) (rs.ProviderResponse, error) {
	name := "ascii.color"
	key := "color"
	if provider == rs.ProviderASCII2DBOVW {
		name = "ascii.bovw"
		key = "bovw"
	}
	if s.p.concurrent {
		s.p.mu.Lock()
		s.p.started++
		if s.p.started == 2 {
			close(s.p.gate)
		}
		s.p.mu.Unlock()
		select {
		case <-s.p.gate:
		case <-ctx.Done():
			return rs.ProviderResponse{}, ctx.Err()
		case <-time.After(2 * time.Second):
			s.p.mu.Lock()
			s.p.gateTimedOut = true
			s.p.mu.Unlock()
			return rs.ProviderResponse{}, errors.New("owned concurrency gate timed out")
		}
	}
	e := s.p.read(ctx, name, s.snap)
	return rs.ProviderResponse{Provider: provider, Matches: s.p.matches[key]}, e
}

func migrationReverseAggregateRows(t *testing.T) []migrationReverseRow {
	canonical := []rs.Match{{Rank: 2, Similarity: 90, Title: "owned title", Author: "owned author", ArtworkID: 42, UserID: 9, ExternalURLs: []string{"https://www.pixiv.net/artworks/77"}}, {Rank: 1, Similarity: 75, UserID: 9}, {Rank: 2, Similarity: 80, ArtworkID: 42, Title: "later title"}, {Rank: 3, Similarity: 20, ExternalURLs: []string{"https://example.test/external"}}}
	cases := []struct {
		name, provider, context                                               string
		pixivOnly, missingSauce, missingASCII, concurrent, branchesConcurrent bool
		errors                                                                map[string]string
		matches                                                               map[string][]rs.Match
	}{
		{name: "sauce-canonical-order", provider: "saucenao", matches: map[string][]rs.Match{"sauce": canonical}},
		{name: "sauce-pixiv-only", provider: "saucenao", pixivOnly: true, matches: map[string][]rs.Match{"sauce": canonical}},
		{name: "all-one-upload-stable-merge", provider: "all", concurrent: true, matches: map[string][]rs.Match{"sauce": canonical, "color": {{Rank: 1, ArtworkID: 42, Title: "color title"}, {Rank: 2, UserID: 9}}, "bovw": {{Rank: 1, ExternalURLs: []string{"https://www.pixiv.net/artworks/42"}}}}},
		{name: "all-provider-branches-overlap", provider: "all", concurrent: true, branchesConcurrent: true},
		{name: "all-empty-is-success", provider: "all", concurrent: true}, {name: "color-only", provider: "ascii2d-color", matches: map[string][]rs.Match{"color": canonical}}, {name: "bovw-only", provider: "ascii2d-bovw", matches: map[string][]rs.Match{"bovw": canonical}},
		{name: "invalid-provider", provider: "invalid"}, {name: "empty-provider", provider: ""}, {name: "pre-canceled", provider: "all", context: "canceled"}, {name: "deadline", provider: "saucenao", context: "deadline"}, {name: "nil-context", provider: "all", context: "nil"},
		{name: "missing-sauce", provider: "saucenao", missingSauce: true}, {name: "missing-ascii", provider: "ascii2d-color", missingASCII: true}, {name: "all-missing", provider: "all", missingSauce: true, missingASCII: true},
		{name: "sauce-preflight-generic", provider: "saucenao", errors: map[string]string{"sauce.preflight": "generic"}}, {name: "all-sauce-preflight-failure", provider: "all", errors: map[string]string{"sauce.preflight": "classified"}},
		{name: "all-ascii-preflight-failure", provider: "all", errors: map[string]string{"ascii.preflight": "generic"}}, {name: "all-upload-failure", provider: "all", errors: map[string]string{"ascii.upload": "classified"}},
		{name: "all-one-mode-failure", provider: "all", errors: map[string]string{"ascii.bovw": "generic"}, matches: map[string][]rs.Match{"color": canonical}},
		{name: "all-all-failed", provider: "all", errors: map[string]string{"sauce.search": "generic", "ascii.color": "classified", "ascii.bovw": "generic"}},
		{name: "single-safe-generic", provider: "saucenao", errors: map[string]string{"sauce.search": "generic"}}, {name: "single-safe-classified", provider: "saucenao", errors: map[string]string{"sauce.search": "classified"}},
		{name: "all-child-canceled", provider: "all", errors: map[string]string{"ascii.color": "canceled"}}, {name: "single-child-deadline", provider: "saucenao", errors: map[string]string{"sauce.search": "deadline"}},
		{name: "single-wrapped-canceled", provider: "saucenao", errors: map[string]string{"sauce.search": "wrapped-canceled"}},
		{name: "close-errors-reverse-once", provider: "all", errors: map[string]string{"ascii.close": "generic", "sauce.close": "classified"}},
	}
	urls := []string{"https://www.pixiv.net/artworks/1", "https://WWW.PIXIV.NET/users/2", "http://www.pixiv.net/artworks/3", "https://pixiv.net/artworks/4", "https://www.pixiv.net:443/artworks/5", "https://user@www.pixiv.net/artworks/6", "https://www.pixiv.net/artworks/01", "https://www.pixiv.net/artworks/+1", "https://www.pixiv.net/artworks/1/", "https://www.pixiv.net/artworks/%31", "https://www.pixiv.net/artworks/1?q=1", "https://www.pixiv.net/artworks/1#fragment", "https://www.pixiv.net/artworks/9223372036854775807", "https://www.pixiv.net/artworks/9223372036854775808", "https://www.pixiv.net/users/9", "https://www.pixiv.net/ARTWORKS/1"}
	for i, u := range urls {
		cases = append(cases, struct {
			name, provider, context                                               string
			pixivOnly, missingSauce, missingASCII, concurrent, branchesConcurrent bool
			errors                                                                map[string]string
			matches                                                               map[string][]rs.Match
		}{name: fmt.Sprintf("canonical-url-%02d", i), provider: "saucenao", matches: map[string][]rs.Match{"sauce": {{Rank: 1, ExternalURLs: []string{u}}}}})
	}
	rows := []migrationReverseRow{}
	for _, c := range cases {
		t.Run("aggregate/"+c.name, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, "owned.bin")
			if e := os.WriteFile(path, []byte("owned immutable payload"), 0600); e != nil {
				t.Fatal(e)
			}
			loader := rs.NewSourceLoader(rs.SourceLoaderOptions{TempDir: root})
			p := &migrationReversePorts{calls: map[string]int{}, reads: map[string][]string{}, errors: c.errors, matches: c.matches, concurrent: c.concurrent, gate: make(chan struct{}), branchesConcurrent: c.branchesConcurrent, branchGate: make(chan struct{})}
			deps := rs.AggregatorDependencies{}
			if !c.missingSauce {
				deps.SauceNAO = migrationReverseSauce{p}
			}
			if !c.missingASCII {
				deps.ASCII2D = migrationReverseASCII{p}
			}
			a := rs.NewAggregator(deps)
			f := rs.NewFacade(rs.Dependencies{Sources: loader, Payloads: a})
			ctx, cancel := migrationReverseContext(c.context)
			defer cancel()
			response, err := f.Search(ctx, rs.Request{Source: path, Provider: rs.Provider(c.provider), PixivOnly: c.pixivOnly})
			if p.gateTimedOut || (c.concurrent && p.started != 2) || (c.branchesConcurrent && p.branchesStarted != 2) {
				t.Fatalf("concurrent provider gates were not satisfied: modes=%d branches=%d timed_out=%t", p.started, p.branchesStarted, p.gateTimedOut)
			}
			close1, close2 := f.Close(), f.Close()
			closed := true
			same := true
			for i, snap := range p.snapshots {
				r, e := snap.Open()
				if r != nil {
					_ = r.Close()
				}
				closed = closed && e != nil
				if i > 0 {
					same = same && p.snapshots[0] == snap
				}
			}
			entries, e := os.ReadDir(root)
			if e != nil {
				t.Fatal(e)
			}
			temporary := 0
			for _, entry := range entries {
				if entry.Name() != "owned.bin" {
					temporary++
				}
			}
			out := map[string]any{"response": migrationReverseJSON(t, response), "error": migrationReverseError(err), "calls": p.calls, "reads_hex": p.reads, "same_snapshot": same, "closed_after_search": closed, "temporary_files_after": temporary, "close_order": p.closeOrder, "close_first": migrationReverseError(close1), "close_repeat": migrationReverseError(close2), "concurrent_modes_started": p.started, "concurrent_branches_started": p.branchesStarted, "concurrent_gate_timed_out": p.gateTimedOut}
			in := map[string]any{"provider": c.provider, "context": c.context, "pixiv_only": c.pixivOnly, "missing_sauce": c.missingSauce, "missing_ascii": c.missingASCII, "concurrent": c.concurrent, "branches_concurrent": c.branchesConcurrent, "errors": c.errors, "matches": c.matches, "payload_hex": migrationReverseHex([]byte("owned immutable payload"))}
			rows = append(rows, migrationReverseRow{Name: c.name, Operation: "facade_aggregate", Input: in, Output: out})
		})
	}
	return rows
}

func TestMigrationReverseSearchCore(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	fixture := migrationReverseFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", Environment: runtime.GOOS + "/" + runtime.GOARCH, GoVersion: runtime.Version(), Sources: map[string]string{}, Boundaries: map[string]string{"source": "Actual unchanged Loader/Snapshot with owned regular files/symlinks/directories and mocked http.Client RoundTripper/body; source mutation precedes snapshot reads; no external fetch or images", "redirect": "Actual http.Client redirects through owned mocked 302/final responses exercise Loader CheckRedirect, default ten-redirect cap and original caller hook precedence/override; actual request/body/hook traces and original error chain retained", "snapshot": "Actual public Snapshot Close failure/retry through replacement of its owned backing file by a nonempty owned directory; actual filesystem cause text/platform retained; held-reader unlink behavior isolated to Linux and does not establish portable caller lifecycle guarantees", "facade": "Actual unchanged Facade dependency/context/preflight precedence and cleanup errors.Join through genuine SourceLoader/PayloadSearcher ports; no production hooks; original error membership, child order, code and messages retained", "aggregate": "Actual unchanged Facade/Aggregator/canonical normalization with genuine provider/session dependency mocks, one real owned snapshot; separate actual Sauce search/ASCII upload and color/BOVW concurrency barriers, each bounded at two seconds; close reverse-order/error/once", "representation": "nil context input and wrapped cancellation are original Go contracts; exact JSON integers retained with UseNumber; no Rust projection or native platform equivalence claimed", "scope": "Linux owned filesystem/Go runtime; private URLs are carried only by synthetic dependencies, no third-party uploads, credentials or denied supplemental native probe"}}
	for _, path := range []string{"go.mod", "go.sum", "internal/services/reversesearch/source.go", "internal/services/reversesearch/aggregator.go", "internal/services/reversesearch/normalize.go", "internal/services/reversesearch/facade.go", "internal/services/reversesearch/contracts.go", "internal/services/reversesearch/errors.go", "internal/services/reversesearch/migration_core_test.go"} {
		b, e := os.ReadFile(filepath.Join(root, path))
		if e != nil {
			t.Fatal(e)
		}
		sum := sha256.Sum256(b)
		fixture.Sources[path] = hex.EncodeToString(sum[:])
	}
	fixture.Cases = append(migrationReverseSourceRows(t), migrationReverseAggregateRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationReverseSnapshotRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationReverseFacadeRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationReverseRedirectRows(t)...)
	sort.SliceStable(fixture.Cases, func(i, j int) bool {
		if fixture.Cases[i].Operation != fixture.Cases[j].Operation {
			return fixture.Cases[i].Operation < fixture.Cases[j].Operation
		}
		return fixture.Cases[i].Name < fixture.Cases[j].Name
	})
	b, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	b = append(b, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "reverse-search-core.json")
	if os.Getenv("PIXIV_CAPTURE_REVERSE_CORE") == "1" {
		if e = os.WriteFile(path, b, 0600); e != nil {
			t.Fatal(e)
		}
		return
	}
	want, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(want, b) {
		t.Fatalf("actual reverse core Go observations differ from sealed fixture (%d bytes/%d rows)", len(b), len(fixture.Cases))
	}
}
