package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
)

var updateInterrupt = flag.Bool("migration-update-cli-interrupt", false, "capture actual root interrupt contracts from the fixed Go reference")

type interruptRow struct {
	Name               string   `json:"name"`
	Stage              string   `json:"stage"`
	Mode               string   `json:"mode"`
	Existing           bool     `json:"existing"`
	Exit               int      `json:"exit"`
	Stdout             string   `json:"stdout"`
	Stderr             string   `json:"stderr"`
	Requests           []string `json:"requests"`
	ClosedConnections  int      `json:"closed_connections"`
	Final              string   `json:"final"`
	Exists             bool     `json:"exists"`
	TemporaryFiles     int      `json:"temporary_files"`
	RefreshToken       string   `json:"refresh_token"`
	CredentialRevision int64    `json:"credential_revision"`
}

func TestMigrationCLIInterruptEntryMatchesFrozenSource(t *testing.T) {
	for path, want := range map[string]string{
		"../../internal/mcpserver/stdio.go": "c48f34ffdcc0f92eadea6766aeb331ceab9899d982dcb59565900aa4adaeb46e",
		"main.go":                           "fe6b94296b80fc4a5d9aad1a3034a4e6ebe4d72600222e2d7e3f0ea4619bcd68",
		"../../internal/cli/commands/pixiv/auth/login.go": "8ff99d517ebcfd8edbbb417d2c7ac90cbffac2ed23a6291ffb383f2172512f11",
		"../../internal/cli/execution.go":                 "627d8d8ccd6e7c35508abe0daab3209d3daf34dbf1927ab6f9f70145b623c864",
	} {
		source, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(source)
		if got := hex.EncodeToString(digest[:]); got != want {
			t.Fatalf("root entry %s differs from frozen Go 4b4426487ef18bed276706daec385e0d0a6979f9: %s", path, got)
		}
	}
}

func TestMigrationCLIInterruptChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_CLI_INTERRUPT_CHILD")
	if encoded == "" {
		t.Skip("owned interrupt child")
	}
	var row interruptRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	if strings.HasPrefix(row.Stage, "mcp-") {
		migrationIdleMCPChild(t, home, row)
		return
	}
	directory := filepath.Join(home, ".pixiv-cli")
	db, err := database.Open(directory)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.SavePixivCredential(context.Background(), account.New(42, "synthetic", []byte("synthetic-refresh"))); err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	destination := filepath.Join(home, "downloads")
	const source = "https://i.pximg.net/assets/interrupt.png?signature=synthetic"
	hash := sha256.Sum256([]byte(source))
	final := filepath.Join(destination, "interrupt-"+hex.EncodeToString(hash[:])[:12]+".png")
	if row.Existing {
		if err := os.MkdirAll(destination, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(final, []byte("previous"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var mu sync.Mutex
	var pending sync.WaitGroup
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	transport.DialContext = func(context.Context, string, string) (net.Conn, error) {
		return nil, errors.New("unexpected non-TLS fixture connection")
	}
	transport.DialTLSContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		if network != "tcp" || (address != "oauth.secure.pixiv.net:443" && address != "i.pximg.net:443") {
			return nil, fmt.Errorf("unexpected synthetic TLS target %s %s", network, address)
		}
		client, server := net.Pipe()
		pending.Add(1)
		go func() {
			defer pending.Done()
			defer server.Close()
			req, err := http.ReadRequest(bufio.NewReader(server))
			if err != nil {
				return
			}
			if req.Body != nil {
				_, _ = io.Copy(io.Discard, req.Body)
				_ = req.Body.Close()
			}
			stage := "resource"
			if req.URL.Path == "/auth/token" {
				stage = "oauth"
			}
			mu.Lock()
			row.Requests = append(row.Requests, stage)
			mu.Unlock()
			if stage == "oauth" && row.Stage != "oauth" {
				body := `{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"synthetic"}}`
				_, _ = fmt.Fprintf(server, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s", len(body), body)
			} else {
				if row.Stage == "body" {
					_, _ = io.WriteString(server, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 100\r\n\r\npart")
				}
				fmt.Fprintln(os.Stdout, "READY")
			}
			_, _ = io.Copy(io.Discard, server)
			mu.Lock()
			row.ClosedConnections++
			mu.Unlock()
		}()
		return client, nil
	}
	http.DefaultTransport = transport
	out, err := os.Create(filepath.Join(home, "stdout"))
	if err != nil {
		t.Fatal(err)
	}
	diagnostics, err := os.Create(filepath.Join(home, "stderr"))
	if err != nil {
		t.Fatal(err)
	}
	input, err := os.Open(os.DevNull)
	if err != nil {
		t.Fatal(err)
	}
	args := []string{"pixiv", "download", source, "--download-path", destination, "--no-proxy"}
	if row.Mode != "text" {
		args = append(args, "--"+row.Mode)
	}
	row.Exit = run(args, input, out, diagnostics)
	_ = input.Close()
	_ = out.Close()
	_ = diagnostics.Close()
	done := make(chan struct{})
	go func() { pending.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("CLI returned without closing owned SDK connections")
	}
	read := func(name string) string {
		b, e := os.ReadFile(filepath.Join(home, name))
		if e != nil {
			t.Fatal(e)
		}
		return strings.ReplaceAll(string(b), home, "$HOME")
	}
	row.Stdout, row.Stderr = read("stdout"), read("stderr")
	b, err := os.ReadFile(final)
	if err == nil {
		row.Exists = true
		row.Final = string(b)
	} else if !os.IsNotExist(err) {
		t.Fatal(err)
	}
	temporary, err := filepath.Glob(filepath.Join(destination, ".atomic-write-*"))
	if err != nil {
		t.Fatal(err)
	}
	row.TemporaryFiles = len(temporary)
	db, err = database.Open(directory)
	if err != nil {
		t.Fatal(err)
	}
	stored, err := db.GetPixiv(context.Background(), 42)
	if err != nil {
		t.Fatal(err)
	}
	row.RefreshToken = string(stored.RefreshTokenCopy())
	row.CredentialRevision = stored.CredentialRevision
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
	os.Exit(row.Exit)
}

func migrationIdleMCPChild(t *testing.T, home string, row interruptRow) {
	row.Requests = []string{}
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	var networkCalls atomic.Int32
	deny := func(context.Context, string, string) (net.Conn, error) {
		networkCalls.Add(1)
		return nil, errors.New("idle MCP must not open any network connection")
	}
	transport.DialContext, transport.DialTLSContext = deny, deny
	http.DefaultTransport = transport
	outputPath := filepath.Join(home, "stdout")
	out, err := os.Create(outputPath)
	if err != nil {
		t.Fatal(err)
	}
	diagnostics, err := os.Create(filepath.Join(home, "stderr"))
	if err != nil {
		t.Fatal(err)
	}
	control := os.Stdout
	os.Stdout = out
	ready := make(chan error, 1)
	go func() {
		deadline := time.NewTimer(10 * time.Second)
		defer deadline.Stop()
		ticker := time.NewTicker(time.Millisecond)
		defer ticker.Stop()
		for {
			body, _ := os.ReadFile(outputPath)
			var response struct {
				ID     int `json:"id"`
				Result struct {
					ProtocolVersion string `json:"protocolVersion"`
				} `json:"result"`
			}
			if json.Unmarshal(body, &response) == nil && response.ID == 1 && response.Result.ProtocolVersion != "" {
				fmt.Fprintln(control, "READY")
				ready <- nil
				return
			}
			select {
			case <-deadline.C:
				ready <- errors.New("idle MCP did not initialize")
				return
			case <-ticker.C:
			}
		}
	}()
	row.Exit = run([]string{"pixiv", "mcp"}, os.Stdin, out, diagnostics)
	if networkCalls.Load() != 0 {
		t.Fatal("idle MCP attempted synthetic network transport")
	}
	if err := <-ready; err != nil {
		t.Fatal(err)
	}
	_ = out.Close()
	_ = diagnostics.Close()
	body, err := os.ReadFile(outputPath)
	if err != nil {
		t.Fatal(err)
	}
	row.Stdout = string(body)
	body, err = os.ReadFile(filepath.Join(home, "stderr"))
	if err != nil {
		t.Fatal(err)
	}
	row.Stderr = string(body)
	data, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
	os.Exit(row.Exit)
}

func TestMigrationCLIInterruptMatchesFrozenRoot(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("actual os.Interrupt delivery and isolated startup evidence is Linux-only")
	}
	rows := []interruptRow{}
	for _, stage := range []string{"oauth", "headers", "body"} {
		for _, mode := range []string{"text", "json", "ndjson"} {
			rows = append(rows, interruptRow{Name: stage + "-" + mode, Stage: stage, Mode: mode})
		}
	}
	rows = append(rows, interruptRow{Name: "body-existing-json", Stage: "body", Mode: "json", Existing: true})
	rows = append(rows, interruptRow{Name: "mcp-idle", Stage: "mcp-idle", Mode: "text"})
	rows = append(rows, interruptRow{Name: "mcp-eof", Stage: "mcp-eof", Mode: "text"})
	for i := range rows {
		row := &rows[i]
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			encoded, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationCLIInterruptChild$")
			child.Env = []string{"HOME=" + home, "PATH=" + os.Getenv("PATH"), "MIGRATION_CLI_INTERRUPT_CHILD=" + string(encoded)}

			var input io.WriteCloser
			if strings.HasPrefix(row.Stage, "mcp-") {
				input, err = child.StdinPipe()
				if err != nil {
					t.Fatal(err)
				}
				defer input.Close()
			}
			pipe, err := child.StdoutPipe()
			if err != nil {
				t.Fatal(err)
			}
			var diagnostics bytes.Buffer
			child.Stderr = &diagnostics
			if err := child.Start(); err != nil {
				t.Fatal(err)
			}
			defer func() {
				if child.ProcessState == nil {
					_ = child.Process.Kill()
					_ = child.Wait()
				}
			}()

			if strings.HasPrefix(row.Stage, "mcp-") {
				_, err = io.WriteString(input, `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`+"\n")
				if err != nil {
					t.Fatal(err)
				}
			}
			ready := make(chan error, 1)
			go func() {
				scanner := bufio.NewScanner(pipe)
				for scanner.Scan() {
					if scanner.Text() == "READY" {
						ready <- nil
						_, _ = io.Copy(io.Discard, pipe)
						return
					}
				}
				ready <- fmt.Errorf("child ended before READY: %v", scanner.Err())
			}()
			select {
			case err := <-ready:
				if err != nil {
					t.Fatalf("%v: %s", err, diagnostics.String())
				}
			case <-ctx.Done():
				t.Fatal("bounded READY timeout")
			}
			if row.Stage == "body" {
				for {
					paths, _ := filepath.Glob(filepath.Join(home, "downloads", ".atomic-write-*"))
					written := false
					for _, path := range paths {
						if info, err := os.Stat(path); err == nil && info.Size() == 4 {
							written = true
						}
					}
					if written {
						break
					}
					select {
					case <-ctx.Done():
						t.Fatal("partial atomic destination never became ready")
					case <-time.After(time.Millisecond):
					}
				}
			}

			if row.Stage == "mcp-eof" {
				if err := input.Close(); err != nil {
					t.Fatal(err)
				}
			} else if err := child.Process.Signal(os.Interrupt); err != nil {
				t.Fatal(err)
			}
			err = child.Wait()
			if ctx.Err() != nil {
				t.Fatalf("interrupt failed to finish owned child: %v %s", err, diagnostics.String())
			}

			if row.Stage == "mcp-eof" {
				if err != nil {
					t.Fatalf("want normal EOF process exit 0, got %v: %s", err, diagnostics.String())
				}
			} else {
				var exit *exec.ExitError
				if !errors.As(err, &exit) || exit.ExitCode() != 1 {
					t.Fatalf("want process exit 1, got %v: %s", err, diagnostics.String())
				}
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatalf("%v: %s", err, diagnostics.String())
			}
			if err := json.Unmarshal(result, row); err != nil {
				t.Fatal(err)
			}
			if row.TemporaryFiles != 0 || row.Exists != row.Existing || (row.Existing && row.Final != "previous") {
				t.Fatalf("atomic destination changed after interrupt: %+v", row)
			}
		})
	}
	if t.Failed() {
		return
	}
	fixture := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli_interrupt.json")
	captured, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	captured = append(captured, '\n')
	if *updateInterrupt {
		if err := os.WriteFile(fixture, captured, 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	expected, err := os.ReadFile(fixture)
	if err != nil {
		t.Fatal(err)
	}
	var frozen []interruptRow
	if err := json.Unmarshal(expected, &frozen); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(rows, frozen) {
		t.Fatalf("actual root interrupt differs from frozen fixture\n%s", captured)
	}
}
