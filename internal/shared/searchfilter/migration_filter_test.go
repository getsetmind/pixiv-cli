package searchfilter_test

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/searchfilter"
)

var migrationUpdateFilter = flag.Bool("migration-update-search-filter", false, "capture frozen local filter normalization, cursor binding and matching")

func TestMigrationLocalFiltersMatchFrozenNormalizationBindingAndUnknownClasses(t *testing.T) {
	type point struct {
		Restrict int    `json:"x_restrict"`
		Kind     string `json:"kind"`
	}
	type row struct {
		Rating            string `json:"rating"`
		Content           string `json:"content_type"`
		NormalizedRating  string `json:"normalized_rating"`
		NormalizedContent string `json:"normalized_content_type"`
		Context           string `json:"context"`
		Error             string `json:"error"`
		Matches           []bool `json:"matches"`
	}
	fixture := struct {
		Points []point `json:"points"`
		Rows   []row   `json:"rows"`
	}{Points: []point{}, Rows: []row{}}
	for _, restrict := range []int{-1, 0, 1, 2, 3, 9223372036854775807} {
		for _, kind := range []string{"", "illust", "illustration", "manga", "ugoira", " ILLUSTRATION ", "unknown"} {
			fixture.Points = append(fixture.Points, point{restrict, kind})
		}
	}
	for _, rating := range []string{"", "all", "sfw", "r18", "r18g", "mature", " R18 ", "SFW", "bad", "explicit", "r18-g", "-bad"} {
		for _, content := range []string{"", "all", "illust", "illustration", "illust-and-ugoira", "manga", "ugoira", " ILLUST ", "İLLUST", "\u3000MANGA\u00a0", "bad", "illust_and_ugoira", "-bad", "\ufeffillust"} {
			current := row{Rating: rating, Content: content, Matches: []bool{}}
			filter, err := searchfilter.NormalizeFilter(rating, content)
			if err != nil {
				current.Error = err.Error()
			} else {
				current.NormalizedRating = string(filter.Rating)
				current.NormalizedContent = string(filter.ContentType)
				current.Context = filter.CursorContext()
				for _, point := range fixture.Points {
					current.Matches = append(current.Matches, filter.Matches(point.Restrict, point.Kind))
				}
			}
			fixture.Rows = append(fixture.Rows, current)
		}
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "search-local-filter.json")
	if *migrationUpdateFilter {
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
		t.Fatal("local filters differ from the frozen Go reference")
	}
}
