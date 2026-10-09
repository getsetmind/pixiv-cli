package download

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	goruntime "runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateInterruptShutdown = flag.Bool("migration-update-interrupt-shutdown", false, "capture owned Go stdio interrupt shutdown")

type interruptEvent struct {
	Name           string          `json:"name"`
	RootError      string          `json:"root_error,omitempty"`
	RequestError   string          `json:"request_error"`
	RequestContext bool            `json:"request_context"`
	Published      int             `json:"published"`
	Temporary      int             `json:"temporary"`
	Result         json.RawMessage `json:"result,omitempty"`
}
type interruptCase struct {
	Name                string            `json:"name"`
	Exit                int               `json:"exit"`
	Stdio               []json.RawMessage `json:"stdio"`
	Events              []interruptEvent  `json:"events"`
	Files               []directFile      `json:"files"`
	PostRunRequestError *string           `json:"post_run_request_error,omitempty"`
}
type interruptBody struct {
	context     context.Context
	reader      *bytes.Reader
	release     string
	event       func(string, context.Context)
	once        sync.Once
	cancelAware bool
	readError   error
}

func (b *interruptBody) Read(p []byte) (int, error) {
	b.once.Do(func() {
		b.event("body_waiting", b.context)
		deadline := time.Now().Add(10 * time.Second)
		for {
			if b.cancelAware && b.context.Err() != nil {
				b.readError = b.context.Err()
				b.event("body_cancelled", b.context)
				return
			}
			if _, err := os.Stat(b.release); err == nil {
				break
			}
			if time.Now().After(deadline) {
				fmt.Fprintln(os.Stderr, "release timeout")
				os.Exit(90)
			}
			time.Sleep(time.Millisecond * 5)
		}
		b.event("body_released", b.context)
	})
	if b.readError != nil {
		return 0, b.readError
	}
	return b.reader.Read(p)
}
func (b *interruptBody) Close() error { b.event("body_closed", b.context); return nil }

func TestMigrationMCPInterruptShutdownChild(t *testing.T) {
	if os.Getenv("PIXIV_INTERRUPT_CHILD") == "" {
		return
	}
	root := os.Getenv("PIXIV_INTERRUPT_ROOT")
	dest := filepath.Join(root, "downloads")
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	var mu sync.Mutex
	emit := func(name string, request context.Context, result json.RawMessage) {
		mu.Lock()
		defer mu.Unlock()
		e := interruptEvent{Name: name, Result: result, RequestContext: request != nil}
		if ctx.Err() != nil {
			e.RootError = ctx.Err().Error()
		}
		if request != nil && request.Err() != nil {
			e.RequestError = request.Err().Error()
		}
		entries, err := os.ReadDir(dest)
		if err != nil {
			os.Exit(91)
		}
		for _, entry := range entries {
			if strings.HasPrefix(entry.Name(), ".atomic-write-") {
				e.Temporary++
			} else {
				e.Published++
			}
		}
		if err := json.NewEncoder(os.Stderr).Encode(e); err != nil {
			os.Exit(92)
		}
	}
	event := func(name string, request context.Context) { emit(name, request, nil) }
	var requestContext context.Context
	waiting := make(chan struct{})
	transport := directTransport(func(r *http.Request) (*http.Response, error) {
		var body io.ReadCloser = io.NopCloser(bytes.NewReader([]byte("\x89PNG\r\n\x1a\nfixture")))
		if r.URL.Path == "/pending.png" {
			requestContext = r.Context()
			body = &interruptBody{cancelAware: os.Getenv("PIXIV_INTERRUPT_CHILD") == "active-notification-cancelled", context: r.Context(), reader: bytes.NewReader([]byte("\x89PNG\r\n\x1a\nfixture")), release: filepath.Join(root, "release"), event: func(name string, c context.Context) {
				event(name, c)
				if name == "body_waiting" {
					close(waiting)
				}
			}}
		}
		return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"image/png"}}, Body: body, Request: r}, nil
	})
	client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
	if err != nil {
		os.Exit(93)
	}
	app := runtime.NewApp(&directManager{dest}, nil, runtime.SDKPorts{OpenLease: func(c context.Context, _ runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
		event("lease_acquired", c)
		return lifecycle.NewLease(client, func() error { client.CloseIdleConnections(); event("lease_released", c); return nil }), nil
	}}, runtime.Account{})
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	server.AddReceivingMiddleware(func(next mcp.MethodHandler) mcp.MethodHandler {
		return func(c context.Context, method string, req mcp.Request) (mcp.Result, error) {
			result, err := next(c, method, req)
			if method == "tools/call" {
				event("tool_completed", c)
				raw, e := json.Marshal(result)
				if e != nil {
					os.Exit(95)
				}
				emit("tool_result", c, raw)
			}
			return result, err
		}
	})
	interrupted := make(chan struct{})
	go func() {
		<-ctx.Done()
		if os.Getenv("PIXIV_INTERRUPT_CHILD") != "idle" {
			<-waiting
		}
		event("root_interrupted", requestContext)
		close(interrupted)
	}()
	event("ready", nil)
	err = mcpserver.RunStdio(ctx, server)
	if os.Getenv("PIXIV_INTERRUPT_CHILD") != "normal-eof" {
		<-interrupted
	}
	event("server_returned", nil)
	if requestContext != nil {
		event("retained_request_context", requestContext)
	}
	if (os.Getenv("PIXIV_INTERRUPT_CHILD") == "normal-eof" && err != nil) || (os.Getenv("PIXIV_INTERRUPT_CHILD") != "normal-eof" && err != context.Canceled) {
		fmt.Fprintf(os.Stderr, "unexpected Run error: %v\n", err)
		os.Exit(94)
	}
	os.Exit(0)
}

func TestMigrationMCPInterruptShutdownMatchesFrozenContract(t *testing.T) {
	if os.Getenv("PIXIV_INTERRUPT_CHILD") != "" {
		return
	}
	if goruntime.GOOS == "windows" {
		t.Skip("owned SIGINT child requires Unix; native Windows console interrupt remains separate")
	}
	var rows []interruptCase
	for _, name := range []string{"idle", "active", "active-notification-cancelled", "normal-eof"} {
		t.Run(name, func(t *testing.T) {
			root := t.TempDir()
			dest := filepath.Join(root, "downloads")
			if err := os.Mkdir(dest, 0700); err != nil {
				t.Fatal(err)
			}
			executable, err := os.Executable()
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, executable, "-test.run=^TestMigrationMCPInterruptShutdownChild$")
			cmd.Env = append(os.Environ(), "PIXIV_INTERRUPT_CHILD="+name, "PIXIV_INTERRUPT_ROOT="+root)
			in, err := cmd.StdinPipe()
			if err != nil {
				t.Fatal(err)
			}
			out, err := cmd.StdoutPipe()
			if err != nil {
				t.Fatal(err)
			}
			stderr, err := cmd.StderrPipe()
			if err != nil {
				t.Fatal(err)
			}
			if err = cmd.Start(); err != nil {
				t.Fatal(err)
			}
			defer func() { _ = cmd.Process.Kill(); _ = in.Close() }()
			row := interruptCase{Name: name, Stdio: []json.RawMessage{}, Events: []interruptEvent{}, Files: []directFile{}}
			events := make(chan string, 20)
			errScan := make(chan error, 1)
			go func() {
				defer close(events)
				scanner := bufio.NewScanner(stderr)
				for scanner.Scan() {
					var e interruptEvent
					if err := json.Unmarshal(scanner.Bytes(), &e); err != nil {
						errScan <- fmt.Errorf("child stderr %q: %w", scanner.Text(), err)
						return
					}
					if len(e.Result) > 0 {
						e.Result = normalizeDirectJSON(t, e.Result, dest)
					}
					if e.Name == "retained_request_context" {
						value := e.RequestError
						row.PostRunRequestError = &value
					} else {
						row.Events = append(row.Events, e)
					}
					events <- e.Name
				}
				errScan <- scanner.Err()
			}()
			lines := make(chan []byte, 10)
			outScan := make(chan error, 1)
			go func() {
				scanner := bufio.NewScanner(out)
				scanner.Buffer(make([]byte, 4096), 1024*1024)
				for scanner.Scan() {
					lines <- append([]byte(nil), scanner.Bytes()...)
				}
				close(lines)
				outScan <- scanner.Err()
			}()
			waitEvent := func(want string) {
				t.Helper()
				for {
					select {
					case got, ok := <-events:
						if !ok {
							t.Fatalf("stderr ended before %s: %v", want, <-errScan)
						}
						if got == want {
							return
						}
					case <-ctx.Done():
						t.Fatal(ctx.Err())
					}
				}
			}
			read := func() []byte {
				t.Helper()
				select {
				case raw, ok := <-lines:
					if !ok {
						t.Fatal("premature stdout EOF")
					}
					row.Stdio = append(row.Stdio, normalizeDirectJSON(t, raw, dest))
					return raw
				case <-ctx.Done():
					t.Fatal(ctx.Err())
					return nil
				}
			}
			send := func(raw string) {
				t.Helper()
				if _, err := fmt.Fprintln(in, raw); err != nil {
					t.Fatal(err)
				}
			}
			waitEvent("ready")
			send(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`)
			read()
			send(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
			if name != "idle" && name != "normal-eof" {
				send(`{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":{"srcs":["https://i.pximg.net/prefix.png","https://i.pximg.net/pending.png"]}}}`)
				waitEvent("body_waiting")
			}
			if name == "normal-eof" {
				if err = in.Close(); err != nil {
					t.Fatal(err)
				}
			} else {
				if err = cmd.Process.Signal(os.Interrupt); err != nil {
					t.Fatal(err)
				}
				waitEvent("root_interrupted")
			}
			if name != "idle" && name != "normal-eof" {
				select {
				case <-lines:
					t.Fatal("active request completed before explicit body release")
				case <-time.After(50 * time.Millisecond):
				}
				if name == "active-notification-cancelled" {
					send(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`)
					waitEvent("body_cancelled")
				} else if err = os.WriteFile(filepath.Join(root, "release"), []byte("release"), 0600); err != nil {
					t.Fatal(err)
				}
			}
			waitEvent("server_returned")
			for raw := range lines {
				row.Stdio = append(row.Stdio, normalizeDirectJSON(t, raw, dest))
			}
			if err = <-outScan; err != nil {
				t.Fatal(err)
			}
			if err = <-errScan; err != nil {
				t.Fatal(err)
			}
			if err = cmd.Wait(); err != nil {
				t.Fatalf("child exit: %v", err)
			}
			row.Exit = cmd.ProcessState.ExitCode()

			entries, err := os.ReadDir(dest)
			if err != nil {
				t.Fatal(err)
			}
			for _, e := range entries {
				body, err := os.ReadFile(filepath.Join(dest, e.Name()))
				if err != nil {
					t.Fatal(err)
				}
				row.Files = append(row.Files, directFile{e.Name(), fmt.Sprintf("%x", body)})
			}
			if name != "idle" && name != "normal-eof" {
				for _, e := range row.Events {
					if e.Name == "root_interrupted" && (e.RootError != "context canceled" || e.RequestError != "" || e.Published != 1 || e.Temporary != 1) {
						t.Fatalf("unexpected interrupted lifetime: %+v", e)
					}
				}
			}
			rows = append(rows, row)
		})
	}
	hashes := map[string]string{}
	for _, path := range []string{"cmd/pixiv/main.go", "internal/mcpserver/stdio.go", "internal/mcpserver/pixiv/tools/download/download.go", "internal/mcpserver/pixiv/internal/runtime/runtime.go", "sdk/pixiv/resource.go"} {
		data, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "..", filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		hashes[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	const version = "v0.8.0"
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("set explicit GOMODCACHE for cached Go SDK provenance")
	}
	for _, path := range []string{"internal/jsonrpc2/conn.go", "mcp/server.go", "mcp/transport.go"} {
		data, err := os.ReadFile(filepath.Join(cache, "github.com/modelcontextprotocol/go-sdk@"+version, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		hashes["github.com/modelcontextprotocol/go-sdk@"+version+"/"+path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}

	result := struct {
		Source       string            `json:"source"`
		SourceHashes map[string]string `json:"source_hashes"`
		Cases        []interruptCase   `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", hashes, rows}
	data, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "..", "crates", "pixiv-mcp", "tests", "fixtures", "interrupt_shutdown.json")
	if *updateInterruptShutdown {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("owned stdio interrupt shutdown differs from frozen Go")
	}
}
