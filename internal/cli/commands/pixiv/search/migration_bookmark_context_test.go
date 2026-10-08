package search

import (
	"bytes"
	"encoding/json"
	"flag"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/searchfilter"
	"os"
	"path/filepath"
	"testing"
)

var migrationUpdateBookmarkContext = flag.Bool("migration-update-bookmark-context", false, "capture bookmark and combined local continuation contexts")

func TestMigrationBookmarkContextsPreserveNilZeroStrategyAndOrderedCombination(t *testing.T) {
	type row struct {
		Min      *int   `json:"min"`
		Max      *int   `json:"max"`
		Strategy string `json:"strategy"`
		Context  string `json:"context"`
		Bookmark string `json:"bookmark"`
		Combined string `json:"combined"`
	}
	zero, ten, maximum := 0, 10, int(^uint(0)>>1)
	filter, err := searchfilter.NormalizeFilter("r18", "illust")
	if err != nil {
		t.Fatal(err)
	}
	rows := []row{}
	for _, min := range []*int{nil, &zero, &ten, &maximum} {
		for _, max := range []*int{nil, &zero, &ten, &maximum} {
			for _, strategy := range []string{"auto", "local", "best_effort", "server"} {
				for _, context := range []string{"", filter.CursorContext(), "other-local-context"} {
					bookmark := searchfilter.BookmarkContext(min, max, strategy)
					rows = append(rows, row{Min: min, Max: max, Strategy: strategy, Context: context, Bookmark: bookmark, Combined: combineCursorContexts(context, bookmark)})
				}
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "..", "docs", "migration", "contracts", "bookmark-context.json")
	if *migrationUpdateBookmarkContext {
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
		t.Fatal("bookmark continuation contexts differ from frozen Go")
	}
}
