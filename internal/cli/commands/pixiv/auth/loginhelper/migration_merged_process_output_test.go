//go:build linux

package loginhelper

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestMigrationMergedProcessOutput(t *testing.T) {
	data, err := os.ReadFile("../../../../../../crates/pixiv-app/tests/fixtures/merged_process_output.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name   string
		Exit   int
		Writes []struct {
			Stderr bool
			Hex    string
			Repeat int
		}
	}
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			var script strings.Builder
			script.WriteString("#!/bin/sh\nif read value; then exit 41; fi\n")
			var wanted []byte
			for _, write := range tc.Writes {
				chunk, err := hex.DecodeString(write.Hex)
				if err != nil {
					t.Fatal(err)
				}
				var octal strings.Builder
				for _, b := range chunk {
					fmt.Fprintf(&octal, "\\%03o", b)
				}
				redirect := ""
				if write.Stderr {
					redirect = " >&2"
				}
				fmt.Fprintf(&script, "i=0; while [ \"$i\" -lt %d ]; do printf '%s'%s; i=$((i+1)); done\n", write.Repeat, octal.String(), redirect)
				wanted = append(wanted, bytes.Repeat(chunk, write.Repeat)...)
			}
			fmt.Fprintf(&script, "exit %d\n", tc.Exit)
			path := filepath.Join(t.TempDir(), "trusted-merged-child")
			if err := os.WriteFile(path, []byte(script.String()), 0700); err != nil {
				t.Fatal(err)
			}
			for _, withContext := range []bool{false, true} {
				command := exec.Command(path)
				if withContext {
					command = exec.CommandContext(context.Background(), path)
				}
				out, err := command.CombinedOutput()
				if !bytes.Equal(out, wanted) {
					t.Fatalf("context=%v: got %d bytes, wanted %d", withContext, len(out), len(wanted))
				}
				if tc.Exit == 0 {
					if err != nil {
						t.Fatal(err)
					}
				} else {
					var exited *exec.ExitError
					if !errors.As(err, &exited) || exited.ExitCode() != tc.Exit || len(exited.Stderr) != 0 {
						t.Fatalf("combined exit=%v", err)
					}
				}
			}
		})
	}
}

func TestMigrationMergedProcessOutputCancellation(t *testing.T) {
	directory := t.TempDir()
	path := filepath.Join(directory, "trusted-merged-child")
	marker := filepath.Join(directory, "started")
	if err := os.WriteFile(path, []byte("#!/bin/sh\nprintf '\\000\\377'; printf '\\200err' >&2; printf started > \"$1\"; while :; do :; done\n"), 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	command := exec.CommandContext(ctx, path, marker)
	type result struct {
		out []byte
		err error
	}
	done := make(chan result, 1)
	go func() { out, err := command.CombinedOutput(); done <- result{out, err} }()
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
	if !errors.As(got.err, &exited) || got.err.Error() != "signal: killed" || !bytes.Equal(got.out, []byte{0, 255, 128, 'e', 'r', 'r'}) || len(exited.Stderr) != 0 || command.ProcessState == nil {
		t.Fatalf("out=%q err=%v", got.out, got.err)
	}
}

func TestMigrationMergedProcessOutputDescendantEOF(t *testing.T) {
	directory := t.TempDir()
	path := filepath.Join(directory, "trusted-merged-child")
	ready := filepath.Join(directory, "ready")
	release := filepath.Join(directory, "release")
	parent := filepath.Join(directory, "parent")
	defer os.WriteFile(release, []byte("release"), 0600)
	body := "#!/bin/sh\nprintf '%s' \"$$\" > \"$3\"; (printf ready > \"$1\"; while [ ! -f \"$2\" ]; do :; done; printf tail; printf diagnostic >&2) &\nexit 0\n"
	if err := os.WriteFile(path, []byte(body), 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	command := exec.CommandContext(ctx, path, ready, release, parent)
	type result struct {
		out []byte
		err error
	}
	done := make(chan result, 1)
	go func() { out, err := command.CombinedOutput(); done <- result{out, err} }()
	limit := time.Now().Add(3 * time.Second)
	for {
		if _, err := os.Stat(ready); err == nil {
			break
		}
		if time.Now().After(limit) {
			t.Fatal("descendant did not start")
		}
		time.Sleep(time.Millisecond)
	}
	data, err := os.ReadFile(parent)
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(string(data))
	if err != nil {
		t.Fatal(err)
	}
	for syscall.Kill(pid, 0) == nil {
		if time.Now().After(limit) {
			t.Fatal("direct child was not reaped")
		}
		time.Sleep(time.Millisecond)
	}
	if err := syscall.Kill(pid, 0); err != syscall.ESRCH {
		t.Fatalf("parent state=%v", err)
	}
	cancel()
	select {
	case got := <-done:
		t.Fatalf("returned before descendant closed pipe: %v", got.err)
	default:
	}
	if err := os.WriteFile(release, []byte("release"), 0600); err != nil {
		t.Fatal(err)
	}
	got := <-done
	if got.err != nil || string(got.out) != "taildiagnostic" {
		t.Fatalf("out=%q err=%v", got.out, got.err)
	}
}

func TestMigrationMergedProcessOutputSuccessfulExitRetainsCancellationErrorAndBytes(t *testing.T) {
	for _, native := range []bool{false, true} {
		t.Run(fmt.Sprintf("native=%v", native), func(t *testing.T) {
			directory := t.TempDir()
			path := filepath.Join(directory, "trusted-merged-child")
			ready := filepath.Join(directory, "ready")
			release := filepath.Join(directory, "release")
			body := "#!/bin/sh\nprintf '\\000\\377'; printf '\\200err' >&2; printf ready > \"$1\"; while [ ! -f \"$2\" ]; do :; done; printf tail; exit 0\n"
			if err := os.WriteFile(path, []byte(body), 0700); err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			command := exec.CommandContext(ctx, path, ready, release)
			sentinel := errors.New("synthetic cancellation failure")
			command.Cancel = func() error {
				if err := os.WriteFile(release, []byte("release"), 0600); err != nil {
					return err
				}
				if native {
					return sentinel
				}
				return nil
			}
			type result struct {
				out []byte
				err error
			}
			done := make(chan result, 1)
			go func() { out, err := command.CombinedOutput(); done <- result{out, err} }()
			limit := time.Now().Add(3 * time.Second)
			for {
				if _, err := os.Stat(ready); err == nil {
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
			wanted := []byte{0, 255, 128, 'e', 'r', 'r', 't', 'a', 'i', 'l'}
			if !bytes.Equal(got.out, wanted) || command.ProcessState == nil || !command.ProcessState.Success() {
				t.Fatalf("out=%q err=%v state=%v", got.out, got.err, command.ProcessState)
			}
			if native {
				if !errors.Is(got.err, sentinel) || got.err.Error() != "exec: canceling Cmd: synthetic cancellation failure" {
					t.Fatalf("native error=%v", got.err)
				}
			} else if !errors.Is(got.err, context.Canceled) {
				t.Fatalf("context error=%v", got.err)
			}
		})
	}
}
