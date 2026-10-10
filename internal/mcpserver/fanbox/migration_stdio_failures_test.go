package fanbox_test

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationCaptureFanboxStdioFailures = flag.Bool("migration-capture-fanbox-stdio-failures", false, "capture genuine FANBOX server injected connection failures")

type migrationFailureRead struct {
	line string
	err  error
}
type migrationFailureConnection struct {
	mu        sync.Mutex
	reads     chan migrationFailureRead
	closed    chan struct{}
	once      sync.Once
	frames    []json.RawMessage
	failWrite bool
	closes    int
	harness   *migrationMCPReadHarness
}

func (c *migrationFailureConnection) Connect(context.Context) (mcp.Connection, error) { return c, nil }
func (c *migrationFailureConnection) Read(ctx context.Context) (jsonrpc.Message, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-c.closed:
		return nil, io.EOF
	case r := <-c.reads:
		if r.err != nil {
			return nil, r.err
		}
		return jsonrpc.DecodeMessage([]byte(r.line))
	}
}
func (c *migrationFailureConnection) Write(_ context.Context, msg jsonrpc.Message) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.failWrite {
		c.failWrite = false
		c.harness.trace("owned.write.failure")
		return errors.New("owned write failure")
	}
	b, e := jsonrpc.EncodeMessage(msg)
	if e != nil {
		return e
	}
	c.frames = append(c.frames, b)
	return nil
}
func (c *migrationFailureConnection) Close() error {
	c.once.Do(func() { c.mu.Lock(); c.closes++; c.mu.Unlock(); close(c.closed) })
	return nil
}
func (c *migrationFailureConnection) SessionID() string { return "" }
func migrationFailureAwait(t *testing.T, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !condition() {
		if time.Now().After(deadline) {
			t.Fatal("owned FANBOX failure schedule timed out")
		}
		time.Sleep(time.Millisecond)
	}
}

func TestMigrationFanboxStdioFailuresFrozenGo(t *testing.T) {
	for p, want := range migrationMCPReadSources {
		b, e := os.ReadFile(filepath.Join("../../..", p))
		if e != nil {
			t.Fatal(e)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(b)); got != want {
			t.Fatalf("frozen source drift %s: %s", p, got)
		}
	}
	rows := []map[string]any{}
	for _, kind := range []string{"parse_failure", "read_failure", "write_failure"} {
		t.Run(kind, func(t *testing.T) {
			scenario := "eof_pending"
			if kind == "write_failure" {
				scenario = "cancel_reuse"
			}
			h := migrationMCPReadNewHarness(t, kind, scenario, "lease", "", nil)
			connection := &migrationFailureConnection{reads: make(chan migrationFailureRead, 8), closed: make(chan struct{}), frames: []json.RawMessage{}, harness: h}
			ctx, cancel := context.WithCancel(diagnostics.WithScope(context.Background(), diagnostics.SinkFunc(h.event), diagnostics.ModuleFanboxCLI, 0))
			defer cancel()
			done := make(chan error, 1)
			go func() { done <- h.server().Run(ctx, connection) }()
			defer func() { cancel(); _ = connection.Close() }()
			connection.reads <- migrationFailureRead{line: `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-failure","version":"1"}}}`}
			migrationFailureAwait(t, func() bool { connection.mu.Lock(); defer connection.mu.Unlock(); return len(connection.frames) == 1 })
			connection.reads <- migrationFailureRead{line: `{"jsonrpc":"2.0","method":"notifications/initialized"}`}
			connection.reads <- migrationFailureRead{line: `{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fanbox_home","arguments":{}}}`}
			migrationFailureAwait(t, func() bool { h.mu.Lock(); defer h.mu.Unlock(); return h.network == 1 })
			switch kind {
			case "parse_failure":
				connection.reads <- migrationFailureRead{line: "{"}
			case "read_failure":
				connection.reads <- migrationFailureRead{err: errors.New("owned read failure")}
			case "write_failure":
				connection.mu.Lock()
				connection.failWrite = true
				connection.mu.Unlock()
				connection.reads <- migrationFailureRead{line: `{"jsonrpc":"2.0","id":3,"method":"ping","params":{}}`}
			}
			var failure error
			select {
			case failure = <-done:
			case <-time.After(5 * time.Second):
				t.Fatal("owned failed connection did not drain")
			}
			connection.mu.Lock()
			frames := append([]json.RawMessage{}, connection.frames...)
			closes := connection.closes
			connection.mu.Unlock()
			if failure == nil {
				t.Fatal("fatal failure was lost")
			}
			row := h.finish()
			if row.LeaseOpens != 1 || row.LeaseCloses != 1 || closes != 1 {
				t.Fatalf("bad ownership: %+v closes=%d", row, closes)
			}
			rows = append(rows, map[string]any{"name": kind, "server_error": failure.Error(), "connection_closes": closes, "frames": frames, "observation": row})
		})
	}
	fixture := map[string]any{"frozen_go": "4b4426487ef18bed276706daec385e0d0a6979f9", "boundary": "actual registered FANBOX mcp.Server.Run with injected public mcp.Connection failures; real saved SQLite/settings -> Facade -> public SDK injected HTTP; not StdioTransport OS IO or successful cmd/pixiv bootstrap", "cases": rows}
	actual := migrationMCPReadJSON(t, fixture)
	path := filepath.Join("../../..", "crates/pixiv-mcp/tests/fixtures/fanbox-stdio-failures.json")
	if *migrationCaptureFanboxStdioFailures {
		var value any
		if e := json.Unmarshal(actual, &value); e != nil {
			t.Fatal(e)
		}
		b, e := json.MarshalIndent(value, "", "  ")
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(path, append(b, '\n'), 0644); e != nil {
			t.Fatal(e)
		}
		return
	}
	b, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	var want, got any
	if e = json.Unmarshal(b, &want); e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(actual, &got); e != nil {
		t.Fatal(e)
	}
	if !reflect.DeepEqual(want, got) {
		output := filepath.Join(os.TempDir(), "fanbox-stdio-failures-actual.json")
		_ = os.WriteFile(output, actual, 0600)
		t.Fatalf("genuine Go failure mismatch: %s", output)
	}
}
