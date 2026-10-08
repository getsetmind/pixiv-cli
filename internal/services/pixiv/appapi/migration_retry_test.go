package appapi

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"
)

var migrationUpdateRetry = flag.Bool("migration-update-retry", false, "capture Retry-After parsing from the fixed Go reference")

func TestMigrationRetryAfterMatchesFrozenHeaderParsing(t *testing.T) {
	type row struct {
		Header  string `json:"header"`
		Present bool   `json:"present"`
		Nanos   int64  `json:"nanos"`
	}
	now := time.Date(2026, 10, 8, 12, 0, 0, 123456789, time.UTC)
	contract := struct {
		Now   time.Time `json:"now"`
		Cases []row     `json:"cases"`
	}{Now: now}
	for _, header := range []string{"", " ", "0", "-0", "+0", "00120", "+120", "-1", "1.5", "1e2", "120, 240", " 120 ", "9223372036", "9223372037", "18446744073709551616", "Thu, 08 Oct 2026 12:00:01 GMT", "Mon, 01 Jan 2035 00:00:00 GMT", "Monday, 01-Jan-35 00:00:00 GMT", "Mon Jan  1 00:00:00 2035", "Tue, 01 Jan 2035 00:00:00 GMT", "Mon, 01 Jan 2035 00:00:00 UTC", "Mon, 01 Jan 2035 00:00:00 +0000", "Mon, 01 Jan 0001 00:00:00 GMT", "Fri, 31 Dec 9999 23:59:59 GMT"} {
		delay, present := parseRetryAfter(header, now)
		contract.Cases = append(contract.Cases, row{header, present, int64(delay)})
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "docs", "migration", "contracts", "retry-after.json")
	if *migrationUpdateRetry {
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
		t.Fatal("Retry-After parsing differs from the fixed Go reference")
	}
}
