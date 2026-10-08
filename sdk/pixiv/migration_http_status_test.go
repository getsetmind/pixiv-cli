package pixiv

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/protocol"
)

var migrationUpdateHTTPStatus = flag.Bool("migration-update-http-status", false, "capture HTTP status contracts from the fixed Go reference")

func TestMigrationHTTPStatusMatchesFrozenClassification(t *testing.T) {
	type row struct {
		OAuth      bool   `json:"oauth"`
		Status     int    `json:"status"`
		RetryAfter bool   `json:"retry_after"`
		Reason     string `json:"reason"`
		Safe       bool   `json:"safe"`
		HasAfter   bool   `json:"has_after"`
		Message    string `json:"message"`
	}
	var rows []row
	for _, oauth := range []bool{false, true} {
		for _, status := range []int{199, 300, 400, 401, 403, 404, 408, 410, 418, 429, 500, 501, 502, 503, 504, 599} {
			for _, retry := range []bool{false, true} {
				failure := protocol.HTTPStatusWithRetryAfter(status, 120*time.Second, retry)
				err := classifyAppError(failure, "Artwork")
				if oauth {
					err = classifyOAuthError(failure, "Open")
				}
				rows = append(rows, row{oauth, status, retry, string(err.Reason), err.Retry.Safe, err.Retry.HasAfter, err.Error()})
				if err.HTTPStatus != status || err.Product != "pixiv" || err.Detail != "" || err.Transport != "" || err.Unwrap() != nil {
					t.Fatalf("unexpected status metadata: %#v", err)
				}
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "http-status.json")
	if *migrationUpdateHTTPStatus {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("HTTP status classifications differ from the frozen Go reference")
	}
}
