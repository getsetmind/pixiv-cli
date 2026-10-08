package pagination

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"strconv"
	"testing"
)

var migrationUpdateTraversal = flag.Bool("migration-update-traversal", false, "capture single stream pagination and checkpoint behavior")

type migrationCursor string

func (c migrationCursor) IsZero() bool   { return c == "" }
func (c migrationCursor) String() string { return string(c) }
func TestMigrationTraversalPreservesLogicalWindowsCheckpointsAndPartialFailure(t *testing.T) {
	type row struct {
		Skip        int      `json:"skip"`
		Limit       int      `json:"limit"`
		One         bool     `json:"one"`
		Filtered    bool     `json:"filtered"`
		Fault       string   `json:"fault"`
		Items       []int    `json:"items"`
		Fetches     []string `json:"fetches"`
		Checkpoints []int    `json:"checkpoints"`
		Next        string   `json:"next"`
		Returned    int      `json:"returned"`
		More        bool     `json:"more"`
		Error       string   `json:"error"`
	}
	rows := []row{}
	for _, skip := range []int{-1, 0, 1, 4, 8, 9} {
		for _, limit := range []int{-1, 0, 1, 2, 5} {
			for _, one := range []bool{false, true} {
				for _, filtered := range []bool{false, true} {
					for _, fault := range []string{"", "fetch", "predicate", "checkpoint", "zero", "cycle", "consume"} {
						current := row{Skip: skip, Limit: limit, One: one, Filtered: filtered, Fault: fault, Items: []int{}, Fetches: []string{}, Checkpoints: []int{}}
						fetch := func(_ context.Context, cursor migrationCursor) ([]int, migrationCursor, error) {
							current.Fetches = append(current.Fetches, cursor.String())
							index := 0
							if cursor != "" {
								index, _ = strconv.Atoi(cursor.String())
							}
							if fault == "fetch" && index == 1 {
								return nil, "", errors.New("fixture fetch failed")
							}
							if fault == "cycle" && index == 1 {
								return []int{}, cursor, nil
							}
							pages := [][]int{{1, 2, 3, 2}, {4, 5}, {6, 7}}
							if index >= len(pages) {
								return nil, "", nil
							}
							next := migrationCursor("")
							if index < 2 {
								next = migrationCursor(strconv.Itoa(index + 1))
							}
							return pages[index], next, nil
						}
						include := func(value int) (bool, error) {
							if fault == "predicate" && value == 4 {
								return false, errors.New("fixture predicate failed")
							}
							return value%2 == 1, nil
						}
						checkpoint := func(cursor migrationCursor, consumed int) (migrationCursor, error) {
							current.Checkpoints = append(current.Checkpoints, consumed)
							if fault == "checkpoint" {
								return "", errors.New("fixture checkpoint failed")
							}
							if fault == "zero" {
								return "", nil
							}
							return migrationCursor(cursor.String() + "/" + strconv.Itoa(consumed)), nil
						}
						plan := PagePlan{Skip: skip, Limit: limit, OneBatch: one}
						var result PageResult
						var err error
						if filtered {
							var next migrationCursor
							current.Items, next, result, err = CollectFilteredPagesFrom(context.Background(), plan, migrationCursor(""), fetch, include, checkpoint)
							current.Next = next.String()
						} else {
							result, err = TraversePages(context.Background(), plan, fetch, func(items []int) error {
								if fault == "consume" {
									return errors.New("fixture consume failed")
								}
								current.Items = append(current.Items, items...)
								return nil
							})
						}
						current.Returned, current.More = result.Returned, result.HasMore
						if err != nil {
							current.Error = err.Error()
						}
						rows = append(rows, current)
					}
				}
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "traversal.json")
	if *migrationUpdateTraversal {
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
		t.Fatal("traversal changed from frozen Go")
	}
}
