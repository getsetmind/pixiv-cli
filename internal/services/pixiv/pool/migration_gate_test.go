package pool_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
)

var updateGate = flag.Bool("migration-update-gate", false, "update rotation gate contracts")

type migrationGateCase struct {
	Mode     string `json:"mode"`
	Error    string `json:"error"`
	Canceled bool   `json:"canceled"`
	Deadline bool   `json:"deadline"`
	Called   bool   `json:"called"`
	Reusable bool   `json:"reusable"`
}

func TestMigrationGatePreservesValidationCancellationAndRelease(t *testing.T) {
	var cases []migrationGateCase
	for _, mode := range []string{"nil_acquire", "nil_run", "zero_acquire", "zero_run", "nil_function", "success", "failure", "cancel_callback", "occupied_canceled", "occupied_deadline", "pending_canceled", "panic"} {
		row := migrationGateCase{Mode: mode}
		gate := pool.NewGate()
		if mode == "nil_acquire" || mode == "nil_run" {
			gate = nil
		}
		if mode == "zero_acquire" || mode == "zero_run" {
			gate = &pool.Gate{}
		}
		ctx, cancel := context.WithCancel(context.Background())
		var err error
		fn := func(context.Context) error {
			row.Called = true
			switch mode {
			case "failure":
				return errors.New("synthetic callback error")
			case "cancel_callback":
				cancel()
				return errors.New("synthetic callback error")
			case "panic":
				panic("synthetic panic")
			}
			return nil
		}
		switch mode {
		case "nil_acquire", "zero_acquire":
			err = gate.Acquire(ctx)
		case "nil_function":
			err = gate.Run(ctx, nil)
		case "occupied_canceled", "occupied_deadline", "pending_canceled":
			if err = gate.Acquire(context.Background()); err != nil {
				t.Fatal(err)
			}
			if mode == "occupied_canceled" {
				cancel()
			}
			if mode == "occupied_deadline" {
				var stop context.CancelFunc
				ctx, stop = context.WithDeadline(ctx, time.Now().Add(-time.Second))
				defer stop()
			}
			if mode == "pending_canceled" {
				started := make(chan struct{})
				done := make(chan error, 1)
				go func() { close(started); done <- gate.Run(ctx, fn) }()
				<-started
				cancel()
				select {
				case err = <-done:
				case <-time.After(10 * time.Second):
					t.Fatal("pending gate did not cancel")
				}
			} else {
				err = gate.Run(ctx, fn)
			}
			gate.Release()
		case "panic":
			func() {
				defer func() {
					if recover() != "synthetic panic" {
						t.Fatal("callback panic missing")
					}
				}()
				_ = gate.Run(ctx, fn)
			}()
		default:
			err = gate.Run(ctx, fn)
		}
		if err != nil {
			row.Error = err.Error()
		}
		row.Canceled = errors.Is(err, context.Canceled)
		row.Deadline = errors.Is(err, context.DeadlineExceeded)
		if mode != "nil_acquire" && mode != "nil_run" && mode != "zero_acquire" && mode != "zero_run" {
			probe, stop := context.WithTimeout(context.Background(), 10*time.Second)
			if err = gate.Acquire(probe); err != nil {
				stop()
				t.Fatal(err)
			}
			gate.Release()
			stop()
			row.Reusable = true
		}
		cancel()
		cases = append(cases, row)
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "..", "docs", "migration", "contracts", "gate.json")
	if *updateGate {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, encoded) {
		t.Fatal("rotation gate contracts differ")
	}
}
