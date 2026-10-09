//go:build linux

package loginhelper

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestMigrationHostContextStartOrdering(t *testing.T) {
	directory := t.TempDir()
	script := filepath.Join(directory, "trusted-child")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf started\n"), 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", directory)
	for _, deadline := range []bool{false, true} {
		var ctx context.Context
		var cancel context.CancelFunc
		if deadline {
			ctx, cancel = context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
		} else {
			ctx, cancel = context.WithCancel(context.Background())
			cancel()
		}
		defer cancel()
		wanted := context.Canceled
		if deadline {
			wanted = context.DeadlineExceeded
		}
		for _, program := range []string{"trusted-child", filepath.Join(directory, "absent")} {
			out, err := exec.CommandContext(ctx, program).Output()
			if !errors.Is(err, wanted) || len(out) != 0 {
				t.Fatalf("program=%q deadline=%v: out=%q err=%v", program, deadline, out, err)
			}
		}
		_, err := exec.CommandContext(ctx, "missing-host-context-child").Output()
		var lookup *exec.Error
		if !errors.As(err, &lookup) || errors.Is(err, wanted) {
			t.Fatalf("lookup must precede context: %v", err)
		}
	}
}

func TestMigrationHostContextPostStartCancellation(t *testing.T) {
	for _, deadline := range []bool{false, true} {
		t.Run(map[bool]string{false: "cancel", true: "deadline"}[deadline], func(t *testing.T) {
			directory := t.TempDir()
			script := filepath.Join(directory, "trusted-child")
			marker := filepath.Join(directory, "started")
			if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf 'captured stdout'\nprintf 'captured stderr' >&2\nprintf started > \"$1\"\nwhile :; do :; done\n"), 0700); err != nil {
				t.Fatal(err)
			}
			var ctx context.Context
			var cancel context.CancelFunc
			if deadline {
				ctx, cancel = context.WithTimeout(context.Background(), 500*time.Millisecond)
			} else {
				ctx, cancel = context.WithCancel(context.Background())
			}
			defer cancel()
			command := exec.CommandContext(ctx, script, marker)
			if err := command.Start(); err != nil {
				t.Fatal(err)
			}
			limit := time.Now().Add(3 * time.Second)
			for {
				if _, err := os.Stat(marker); err == nil {
					break
				}
				if time.Now().After(limit) {
					_ = command.Process.Kill()
					_ = command.Wait()
					t.Fatal("child did not start")
				}
				time.Sleep(time.Millisecond)
			}
			if !deadline {
				cancel()
			}
			err := command.Wait()
			var exited *exec.ExitError
			if !errors.As(err, &exited) || err.Error() != "signal: killed" || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("kill result=%v", err)
			}
			if command.ProcessState == nil {
				t.Fatal("child was not reaped")
			}
		})
	}
}

func TestMigrationHostContextCapturedOutputOnKilledChild(t *testing.T) {
	directory := t.TempDir()
	script := filepath.Join(directory, "trusted-child")
	marker := filepath.Join(directory, "started")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf 'captured stdout'\nprintf 'captured stderr' >&2\nprintf started > \"$1\"\nwhile :; do :; done\n"), 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	command := exec.CommandContext(ctx, script, marker)
	type result struct {
		out []byte
		err error
	}
	done := make(chan result, 1)
	go func() { out, err := command.Output(); done <- result{out, err} }()
	limit := time.Now().Add(3 * time.Second)
	for {
		if _, err := os.Stat(marker); err == nil {
			break
		}
		if time.Now().After(limit) {
			cancel()
			<-done
			t.Fatal("child did not start")
		}
		time.Sleep(time.Millisecond)
	}
	cancel()
	got := <-done
	var exited *exec.ExitError
	if !errors.As(got.err, &exited) || got.err.Error() != "signal: killed" || string(got.out) != "captured stdout" || !strings.Contains(string(exited.Stderr), "captured stderr") {
		t.Fatalf("out=%q err=%v", got.out, got.err)
	}
}

func TestMigrationHostContextOutputPrefixSuffix(t *testing.T) {
	directory := t.TempDir()
	script := filepath.Join(directory, "trusted-child")
	diagnostic := strings.Repeat("a", 32768) + strings.Repeat("b", 4464) + strings.Repeat("c", 32768)
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf stdout\nprintf '"+diagnostic+"' >&2\nexit 7\n"), 0700); err != nil {
		t.Fatal(err)
	}
	out, err := exec.CommandContext(context.Background(), script).Output()
	var exited *exec.ExitError
	if !errors.As(err, &exited) || err.Error() != "exit status 7" || string(out) != "stdout" {
		t.Fatalf("out=%q err=%v", out, err)
	}
	wanted := strings.Repeat("a", 32768) + "\n... omitting 4464 bytes ...\n" + strings.Repeat("c", 32768)
	if string(exited.Stderr) != wanted {
		t.Fatalf("stderr prefix/suffix differs: length=%d", len(exited.Stderr))
	}
}

func TestMigrationHostContextDescendantPipeFixture(t *testing.T) {
	if os.Getenv("PIXIV_CONTEXT_PIPE_FIXTURE") == "" {
		t.Skip("subprocess-only fixture")
	}
	switch os.Getenv("PIXIV_CONTEXT_PIPE_FIXTURE") {
	case "parent":
		child := exec.Command(os.Args[0], "-test.run=^TestMigrationHostContextDescendantPipeFixture$")
		child.Env = append(os.Environ(), "PIXIV_CONTEXT_PIPE_FIXTURE=child")
		child.Stdout = os.Stdout
		child.Stderr = os.Stderr
		if err := child.Start(); err != nil {
			os.Exit(42)
		}
		os.Exit(0)
	case "child":
		if err := os.WriteFile(os.Getenv("PIXIV_CONTEXT_PIPE_READY"), []byte("ready"), 0600); err != nil {
			os.Exit(43)
		}
		time.Sleep(500 * time.Millisecond)
		_, _ = os.Stdout.WriteString("pipe-tail")
		os.Exit(0)
	}
}

func TestMigrationHostContextDescendantPipeDrainsPastDeadline(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	bytes, err := os.ReadFile(executable)
	if err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	child := filepath.Join(directory, "trusted-context-pipe-child")
	ready := filepath.Join(directory, "ready")
	if err := os.WriteFile(child, bytes, 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()
	command := exec.CommandContext(ctx, child, "-test.run=^TestMigrationHostContextDescendantPipeFixture$")
	// Race instrumentation must not delay the fixture parent exit past its deadline.
	raceOptions := make([]string, 0)
	for _, option := range strings.Fields(os.Getenv("GORACE")) {
		if !strings.HasPrefix(option, "atexit_sleep_ms=") {
			raceOptions = append(raceOptions, option)
		}
	}
	raceOptions = append(raceOptions, "atexit_sleep_ms=0")
	command.Env = append(os.Environ(), "PIXIV_CONTEXT_PIPE_FIXTURE=parent", "PIXIV_CONTEXT_PIPE_READY="+ready, "GORACE="+strings.Join(raceOptions, " "))
	started := time.Now()
	out, err := command.Output()
	if value, err := os.ReadFile(ready); err != nil || string(value) != "ready" {
		t.Fatalf("readiness=%q err=%v", value, err)
	}
	if err != nil || string(out) != "pipe-tail" || time.Since(started) < 450*time.Millisecond || !errors.Is(ctx.Err(), context.DeadlineExceeded) {
		t.Fatalf("out=%q err=%v elapsed=%v context=%v", out, err, time.Since(started), ctx.Err())
	}
}
