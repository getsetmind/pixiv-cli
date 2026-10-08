package lifecycle_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
)

var updateLease = flag.Bool("migration-update-lease", false, "update resource lease contracts")

type migrationLeaseCase struct {
	Mode      string   `json:"mode"`
	Value     int      `json:"value"`
	Calls     int32    `json:"calls"`
	Errors    []string `json:"errors"`
	SameError bool     `json:"same_error"`
	Panicked  bool     `json:"panicked"`
}

func TestMigrationLeasePreservesValueRepeatedConcurrentCloseAndPanic(t *testing.T) {
	var cases []migrationLeaseCase
	for _, mode := range []string{"nil", "no_closer", "success", "failure", "panic", "concurrent_success", "concurrent_failure"} {
		row := migrationLeaseCase{Mode: mode, SameError: true}
		var calls atomic.Int32
		failure := errors.New("synthetic release failure")
		closer := func() error {
			calls.Add(1)
			switch mode {
			case "failure", "concurrent_failure":
				return failure
			case "panic":
				panic("synthetic release panic")
			}
			return nil
		}
		if mode == "no_closer" {
			closer = nil
		}
		lease := lifecycle.NewLease(37, closer)
		if mode == "nil" {
			lease = nil
		}
		if mode == "panic" {
			func() { defer func() { row.Panicked = recover() == "synthetic release panic" }(); _ = lease.Close() }()
			if !row.Panicked {
				t.Fatal("release panic missing")
			}
		}
		count := 3
		if mode == "concurrent_success" || mode == "concurrent_failure" {
			count = 8
		}
		results := make([]error, count)
		var group sync.WaitGroup
		for i := range results {
			group.Add(1)
			go func() { defer group.Done(); results[i] = lease.Close() }()
		}
		group.Wait()
		for _, err := range results {
			text := ""
			if err != nil {
				text = err.Error()
			}
			row.Errors = append(row.Errors, text)
			if err != results[0] {
				row.SameError = false
			}
		}
		row.Value = lease.Value()
		row.Calls = calls.Load()
		cases = append(cases, row)
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "lease.json")
	if *updateLease {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, encoded) {
		t.Fatal("resource lease contracts differ")
	}
}
