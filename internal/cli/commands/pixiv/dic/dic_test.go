package dic

import (
	"bytes"
	"context"
	"encoding/json"
	"strings"
	"testing"

	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
	"github.com/spf13/cobra"
)

type fakeReader struct {
	searchRequest  dicservice.SearchRequest
	searchResults  []dicservice.SearchResult
	searchErr      error
	articleRequest dicservice.ArticleRequest
	article        dicservice.Article
	articleErr     error
}

func (f *fakeReader) Search(_ context.Context, request dicservice.SearchRequest) ([]dicservice.SearchResult, error) {
	f.searchRequest = request
	return f.searchResults, f.searchErr
}

func (f *fakeReader) Article(_ context.Context, request dicservice.ArticleRequest) (dicservice.Article, error) {
	f.articleRequest = request
	return f.article, f.articleErr
}

func newCommand(t *testing.T, reader Reader, args ...string) (*bytes.Buffer, *cobra.Command) {
	t.Helper()
	output := &bytes.Buffer{}
	cmd := New(Dependencies{
		Input:      strings.NewReader(""),
		Output:     output,
		UsageError: func(err error) error { return err },
		JSONOut: func(override *bool) (bool, error) {
			if override != nil {
				return *override, nil
			}
			return false, nil
		},
		Reader: reader,
	})
	cmd.SetOut(output)
	cmd.SetErr(&bytes.Buffer{})
	cmd.SetArgs(args)
	return output, cmd
}

func TestGroupDeclaresSearchAndArticle(t *testing.T) {
	cmd := New(Dependencies{Output: &bytes.Buffer{}})
	if cmd.Use != "dic" {
		t.Fatalf("unexpected dic use: %q", cmd.Use)
	}
	for _, name := range []string{"search", "article"} {
		if _, _, err := cmd.Find([]string{name}); err != nil {
			t.Fatalf("dic command missing %q: %v", name, err)
		}
	}
}

// TestArticleHelpAdvertisesNoCounters 固定 tagger 用来探测 --no-counters 支持的契约。
func TestArticleHelpAdvertisesNoCounters(t *testing.T) {
	output, cmd := newCommand(t, &fakeReader{}, "article", "--help")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if !strings.Contains(output.String(), "--no-counters") {
		t.Fatalf("article help does not advertise --no-counters: %q", output.String())
	}
}

func TestArticleSendsReferenceLanguageAndSkipsCounters(t *testing.T) {
	reader := &fakeReader{article: dicservice.Article{
		ID:          7,
		Title:       "Camie",
		Translation: "カミー",
		Categories:  []string{"キャラクター"},
		URL:         "https://dic.pixiv.net/a/Camie",
		Views:       10,
		Works:       2,
	}}
	output, cmd := newCommand(t, reader, "article", "https://dic.pixiv.net/a/Camie", "--lang", "en", "--no-counters", "--json")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if reader.articleRequest.Ref != "https://dic.pixiv.net/a/Camie" || reader.articleRequest.Language != dicservice.LanguageEnglish || !reader.articleRequest.SkipCounters {
		t.Fatalf("unexpected article request: %+v", reader.articleRequest)
	}
	decoded := map[string]any{}
	if err := json.Unmarshal(output.Bytes(), &decoded); err != nil {
		t.Fatalf("decode article: %v; output=%q", err, output.String())
	}
	if decoded["translation"] != "カミー" || decoded["title"] != "Camie" {
		t.Fatalf("unexpected article payload: %v", decoded)
	}
	if _, ok := decoded["views"]; ok {
		t.Fatalf("views must be absent when counters are skipped: %q", output.String())
	}
}

func TestArticleReportsCountersWhenRequested(t *testing.T) {
	reader := &fakeReader{article: dicservice.Article{Title: "Camie", Views: 10, Works: 2, Comments: 3, Checklists: 4}}
	output, cmd := newCommand(t, reader, "article", "Camie", "--json")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if reader.articleRequest.SkipCounters || reader.articleRequest.Language != dicservice.LanguageJapanese {
		t.Fatalf("unexpected article request: %+v", reader.articleRequest)
	}
	decoded := map[string]any{}
	if err := json.Unmarshal(output.Bytes(), &decoded); err != nil {
		t.Fatalf("decode article: %v; output=%q", err, output.String())
	}
	for field, want := range map[string]float64{"views": 10, "works": 2, "comments": 3, "checklists": 4} {
		if decoded[field] != want {
			t.Fatalf("article %s = %v, want %v", field, decoded[field], want)
		}
	}
}

func TestArticleRejectsUnknownLanguage(t *testing.T) {
	reader := &fakeReader{}
	_, cmd := newCommand(t, reader, "article", "Camie", "--lang", "fr")
	err := cmd.Execute()
	if err == nil || !strings.Contains(err.Error(), "--lang") {
		t.Fatalf("unknown language error = %v, want --lang usage error", err)
	}
	if reader.articleRequest.Ref != "" {
		t.Fatalf("article was fetched for an invalid language: %+v", reader.articleRequest)
	}
}

func TestSearchPassesQueryAndPageAndLimitsNDJSON(t *testing.T) {
	reader := &fakeReader{searchResults: []dicservice.SearchResult{
		{Title: "first", URL: "https://dic.pixiv.net/a/first"},
		{Title: "second", URL: "https://dic.pixiv.net/a/second"},
	}}
	output, cmd := newCommand(t, reader, "search", "camie", "--page", "2", "--limit", "1", "--ndjson")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if reader.searchRequest.Query != "camie" || reader.searchRequest.Page != 2 {
		t.Fatalf("unexpected search request: %+v", reader.searchRequest)
	}
	lines := strings.Split(strings.TrimSpace(output.String()), "\n")
	if len(lines) != 1 {
		t.Fatalf("NDJSON lines = %d, want 1; output=%q", len(lines), output.String())
	}
	decoded := map[string]any{}
	if err := json.Unmarshal([]byte(lines[0]), &decoded); err != nil {
		t.Fatalf("decode record: %v; output=%q", err, output.String())
	}
	if decoded["title"] != "first" {
		t.Fatalf("record title = %v, want first", decoded["title"])
	}
}

func TestSearchJSONPrintsArray(t *testing.T) {
	reader := &fakeReader{searchResults: []dicservice.SearchResult{{Title: "first", Related: []string{"a", "b"}}}}
	output, cmd := newCommand(t, reader, "search", "camie", "--json")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	var decoded []map[string]any
	if err := json.Unmarshal(output.Bytes(), &decoded); err != nil {
		t.Fatalf("decode search: %v; output=%q", err, output.String())
	}
	if len(decoded) != 1 || decoded[0]["title"] != "first" {
		t.Fatalf("unexpected search payload: %v", decoded)
	}
}

func TestSearchRejectsEmptyResultAsJSONArray(t *testing.T) {
	reader := &fakeReader{searchResults: []dicservice.SearchResult{}}
	output, cmd := newCommand(t, reader, "search", "nothing", "--json")
	if err := cmd.Execute(); err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if got := strings.TrimSpace(output.String()); got != "[]" {
		t.Fatalf("empty search JSON = %q, want []", got)
	}
}

func TestSearchRejectsNDJSONWithJSON(t *testing.T) {
	_, cmd := newCommand(t, &fakeReader{}, "search", "camie", "--ndjson", "--json")
	if err := cmd.Execute(); err == nil || !strings.Contains(err.Error(), "--ndjson") {
		t.Fatalf("ndjson+json error = %v, want --ndjson usage error", err)
	}
}
