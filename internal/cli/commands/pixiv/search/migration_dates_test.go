package search

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"
)

var migrationUpdateDates = flag.Bool("migration-update-search-dates", false, "capture frozen CLI quick date ranges at fixed instants")

func TestMigrationQuickDateRangesMatchFrozenTokyoBoundaries(t *testing.T) {
	path := filepath.Join("..", "..", "..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "search-date-inputs.json"))
	if err != nil {
		t.Fatal(err)
	}
	var rows []struct {
		Now      string `json:"now"`
		Duration string `json:"duration"`
		Start    string `json:"start"`
		End      string `json:"end"`
		OK       bool   `json:"ok"`
	}
	if err = json.Unmarshal(data, &rows); err != nil {
		t.Fatal(err)
	}
	for index := range rows {
		row := &rows[index]
		now, err := time.Parse(time.RFC3339Nano, row.Now)
		if err != nil {
			t.Fatal(err)
		}
		row.Start, row.End, row.OK = quickDateRange(row.Duration, now)
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "search-dates-cli.json")
	if *migrationUpdateDates {
		if err = os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("CLI quick date ranges differ from the fixed Go reference")
	}
}
