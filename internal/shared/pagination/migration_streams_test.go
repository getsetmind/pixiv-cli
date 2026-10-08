package pagination

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

var migrationUpdateStreams = flag.Bool("migration-update-streams", false, "capture aggregate state and resumed stream collection")

func TestMigrationStreamsPreserveOrderedStateResumptionAndAtomicFailures(t *testing.T) {
	type state struct {
		Current int               `json:"current"`
		Cursors []migrationCursor `json:"cursors"`
	}
	type outcome struct {
		Items    []int  `json:"items"`
		State    state  `json:"state"`
		Returned int    `json:"returned"`
		More     bool   `json:"more"`
		Error    string `json:"error"`
	}
	type row struct {
		Skip        int      `json:"skip"`
		Limit       int      `json:"limit"`
		One         bool     `json:"one"`
		Odd         bool     `json:"odd"`
		Initial     state    `json:"initial"`
		Fault       string   `json:"fault"`
		First       outcome  `json:"first"`
		Resume      *outcome `json:"resume"`
		Fetches     []string `json:"fetches"`
		Checkpoints []string `json:"checkpoints"`
	}
	states := []state{{}, {Current: 1}, {Current: 2}, {Cursors: []migrationCursor{"0:0:1", ""}}, {Current: 1, Cursors: []migrationCursor{"", "1:0:1"}}, {Current: -1}, {Current: 3}, {Cursors: []migrationCursor{""}}}
	rows := []row{}
	for _, skip := range []int{0, 2, 5, 99} {
		for _, limit := range []int{0, 1, 3, 8} {
			for _, one := range []bool{false, true} {
				for _, odd := range []bool{false, true} {
					for _, initial := range states {
						for _, fault := range []string{"", "fetch", "predicate", "checkpoint", "zero", "cycle"} {
							current := row{Skip: skip, Limit: limit, One: one, Odd: odd, Initial: initial, Fault: fault, Fetches: []string{}, Checkpoints: []string{}}
							streams := []Stream[int, migrationCursor]{}
							for streamIndex, pages := range [][][]int{{{1, 2, 1}, {}, {3, 4}}, {{5, 6, 5}, {7, 8}}} {
								decode := func(cursor migrationCursor) (int, int) {
									if cursor == "" {
										return 0, 0
									}
									parts := strings.Split(cursor.String(), ":")
									page, _ := strconv.Atoi(parts[1])
									offset, _ := strconv.Atoi(parts[2])
									return page, offset
								}
								streams = append(streams, Stream[int, migrationCursor]{Fetch: func(_ context.Context, cursor migrationCursor) ([]int, migrationCursor, error) {
									current.Fetches = append(current.Fetches, fmt.Sprintf("%d|%s", streamIndex, cursor))
									page, offset := decode(cursor)
									if fault == "fetch" && streamIndex == 1 {
										return nil, "", errors.New("fixture stream fetch failed")
									}
									if fault == "cycle" && streamIndex == 0 && page == 1 {
										return []int{}, cursor, nil
									}
									next := migrationCursor("")
									if page+1 < len(pages) {
										next = migrationCursor(fmt.Sprintf("%d:%d:0", streamIndex, page+1))
									}
									return pages[page][offset:], next, nil
								}, Include: func(value int) (bool, error) {
									if fault == "predicate" && value == 6 {
										return false, errors.New("fixture stream predicate failed")
									}
									return !odd || value%2 == 1, nil
								}, Checkpoint: func(cursor migrationCursor, consumed int) (migrationCursor, error) {
									current.Checkpoints = append(current.Checkpoints, fmt.Sprintf("%d|%s|%d", streamIndex, cursor, consumed))
									if fault == "checkpoint" {
										return "", errors.New("fixture stream checkpoint failed")
									}
									if fault == "zero" {
										return "", nil
									}
									page, offset := decode(cursor)
									return migrationCursor(fmt.Sprintf("%d:%d:%d", streamIndex, page, offset+consumed)), nil
								}})
							}
							collect := func(plan PagePlan, initial state) outcome {
								items, next, result, err := CollectStreamsFrom(context.Background(), plan, streams, StreamState[migrationCursor]{Current: initial.Current, Cursors: initial.Cursors})
								value := outcome{Items: items, State: state{Current: next.Current, Cursors: next.Cursors}, Returned: result.Returned, More: result.HasMore}
								if err != nil {
									value.Error = err.Error()
								}
								return value
							}
							current.First = collect(PagePlan{Skip: skip, Limit: limit, OneBatch: one}, initial)
							if current.First.Error == "" && current.First.More {
								next := collect(PagePlan{}, current.First.State)
								current.Resume = &next
							}
							rows = append(rows, current)
						}
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
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "streams.json")
	if *migrationUpdateStreams {
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
		t.Fatal("stream collection changed from frozen Go")
	}
}
