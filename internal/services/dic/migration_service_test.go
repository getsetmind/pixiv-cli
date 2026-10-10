package dic_test

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

var updateDictionaryService = flag.Bool("migration-update-dictionary-service", false, "capture anonymous dictionary service and transport contracts")

const migrationDictionaryReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

var migrationDictionarySources = map[string]string{
	"internal/services/dic/dic.go":            "2722e246140e3c4c5f93bcf4e46bb3f6d502408244b871fa5b42a276b525db8e",
	"internal/services/dic/errors.go":         "6798eee3c69b135adfbe379d14e09f6a21d4a46f19dd0f2f8a162b6069fcf9b3",
	"internal/services/dic/article.go":        "e6a46d09d7900303addfc6388ecf2b7efb30364da0a43ec3ea870fd03e52970d",
	"internal/services/dic/search.go":         "3997d71cb5d813ea60ed2ccdbf6f4056357609d3ca340b6820ebf09445f2d5bc",
	"internal/services/dic/transport.go":      "8c7839a95f90f570c4ac453e40a0a900584142b92d17aa1dc5a2c80c4450a9d9",
	"internal/services/dic/article_test.go":   "b0c8548dd129228cfe13e46da536aa4e22fc196cd917be56de133da21499d3ec",
	"internal/services/dic/search_test.go":    "fdd0dab02ee4987fc7bcd68b02959ffdd7e76e348276dc771a21de7dbedf0bc6",
	"internal/services/dic/dic_test.go":       "100ee2f559e87f95db75f0502e724eb0f6218afe05fc59dc0fbb26deaa0434ff",
	"internal/services/dic/errors_test.go":    "625d1ef504ec11c4e0755c0cb186d74e3e20c2d4a4b11862b4bd4f07cb86359a",
	"internal/services/dic/transport_test.go": "bf5eba4e4e29b77916190fb2afb10f7c1a07402daee9f4f0b3007ec14dcfb617",
}

type migrationDictionaryError struct {
	Code           string `json:"code"`
	Message        string `json:"message"`
	Status         int    `json:"status"`
	Typed          bool   `json:"typed"`
	Cause          string `json:"cause"`
	IsCanceled     bool   `json:"is_canceled"`
	IsDeadline     bool   `json:"is_deadline"`
	IsFixtureCause bool   `json:"is_fixture_cause"`
}

var migrationDictionaryCause = errors.New("synthetic dictionary transport failure")

func migrationDictionaryObserveError(err error) *migrationDictionaryError {
	if err == nil {
		return nil
	}
	result := &migrationDictionaryError{
		Code: string(dic.CodeOf(err)), Message: err.Error(),
		IsCanceled: errors.Is(err, context.Canceled), IsDeadline: errors.Is(err, context.DeadlineExceeded),
		IsFixtureCause: errors.Is(err, migrationDictionaryCause),
	}
	var classified *dic.Error
	if errors.As(err, &classified) {
		result.Typed, result.Status = true, classified.StatusCode()
	}
	if cause := errors.Unwrap(err); cause != nil {
		result.Cause = cause.Error()
	}
	return result
}

type migrationDictionaryResponse struct {
	Body        string `json:"body"`
	Status      int    `json:"status"`
	Error       string `json:"error"`
	CancelAfter bool   `json:"cancel_after"`
}

type migrationDictionaryRequest struct {
	URL             string `json:"url"`
	Accept          string `json:"accept"`
	ContextCanceled bool   `json:"context_canceled"`
	ContextDeadline bool   `json:"context_deadline"`
}

type migrationDictionaryArticle struct {
	ID          int      `json:"id"`
	Title       string   `json:"title"`
	Yomigana    string   `json:"yomigana"`
	Translation string   `json:"translation"`
	Categories  []string `json:"categories"`
	Abstract    string   `json:"abstract"`
	Related     []string `json:"related"`
	Body        string   `json:"body"`
	Views       int      `json:"views"`
	Works       int      `json:"works"`
	Comments    int      `json:"comments"`
	Checklists  int      `json:"checklists"`
	URL         string   `json:"url"`
}

type migrationDictionarySearchResult struct {
	Title      string   `json:"title"`
	Summary    string   `json:"summary"`
	Updated    string   `json:"updated"`
	Views      int      `json:"views"`
	Works      int      `json:"works"`
	Checklists int      `json:"checklists"`
	Related    []string `json:"related"`
	URL        string   `json:"url"`
	Thumbnail  string   `json:"thumbnail"`
}

type migrationDictionaryServiceRow struct {
	Name         string                            `json:"name"`
	Context      string                            `json:"context"`
	Client       string                            `json:"client"`
	Ref          string                            `json:"ref"`
	Language     string                            `json:"language"`
	SkipCounters bool                              `json:"skip_counters"`
	Query        string                            `json:"query"`
	Page         int                               `json:"page"`
	Responses    []migrationDictionaryResponse     `json:"responses"`
	Requests     []migrationDictionaryRequest      `json:"requests"`
	Article      *migrationDictionaryArticle       `json:"article"`
	Results      []migrationDictionarySearchResult `json:"results"`
	Error        *migrationDictionaryError         `json:"error"`
}

type migrationDictionaryFixture struct {
	Schema     int                               `json:"schema"`
	Reference  string                            `json:"reference"`
	Sources    map[string]string                 `json:"sources"`
	Gaps       []string                          `json:"gaps"`
	Articles   []migrationDictionaryServiceRow   `json:"articles"`
	Searches   []migrationDictionaryServiceRow   `json:"searches"`
	Transports []migrationDictionaryTransportRow `json:"transports"`
}

type migrationDictionaryTransport struct {
	t         *testing.T
	responses []migrationDictionaryResponse
	requests  []migrationDictionaryRequest
	cancel    context.CancelFunc
}

func (s *migrationDictionaryTransport) Get(ctx context.Context, rawURL, accept string) ([]byte, int, error) {
	s.requests = append(s.requests, migrationDictionaryRequest{
		URL: rawURL, Accept: accept,
		ContextCanceled: errors.Is(ctx.Err(), context.Canceled), ContextDeadline: errors.Is(ctx.Err(), context.DeadlineExceeded),
	})
	if len(s.responses) == 0 {
		s.t.Fatal("dictionary exceeded its finite transport responses")
	}
	response := s.responses[0]
	s.responses = s.responses[1:]
	if response.CancelAfter {
		s.cancel()
	}
	var err error
	switch response.Error {
	case "":
	case "fixture":
		err = migrationDictionaryCause
	case "canceled":
		err = context.Canceled
	case "deadline":
		err = context.DeadlineExceeded
	case "context":
		err = ctx.Err()
		if err == nil {
			s.t.Fatal("context response requires an ended context")
		}
	default:
		s.t.Fatalf("unknown response error %q", response.Error)
	}
	return []byte(response.Body), response.Status, err
}

func migrationDictionaryContext(t *testing.T, name string) (context.Context, context.CancelFunc) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	switch name {
	case "background":
	case "nil":
		ctx = nil
	case "canceled":
		cancel()
	case "deadline":
		cancel()
		ctx, cancel = context.WithDeadline(context.Background(), time.Unix(1, 0))
		<-ctx.Done()
	default:
		t.Fatalf("unknown dictionary context %q", name)
	}
	t.Cleanup(cancel)
	return ctx, cancel
}

func migrationDictionaryRunService(t *testing.T, row migrationDictionaryServiceRow, article bool) migrationDictionaryServiceRow {
	t.Helper()
	ctx, cancel := migrationDictionaryContext(t, row.Context)
	transport := &migrationDictionaryTransport{t: t, responses: append([]migrationDictionaryResponse(nil), row.Responses...), requests: []migrationDictionaryRequest{}, cancel: cancel}
	client := dic.New(transport)
	switch row.Client {
	case "normal":
	case "nil":
		client = nil
	case "nil-transport":
		client = dic.New(nil)
	default:
		t.Fatalf("unknown dictionary client %q", row.Client)
	}
	if article {
		value, err := client.Article(ctx, dic.ArticleRequest{Ref: row.Ref, Language: dic.Language(row.Language), SkipCounters: row.SkipCounters})
		row.Error = migrationDictionaryObserveError(err)
		if err == nil {
			row.Article = &migrationDictionaryArticle{
				ID: value.ID, Title: value.Title, Yomigana: value.Yomigana, Translation: value.Translation,
				Categories: value.Categories, Abstract: value.Abstract, Related: value.Related, Body: value.Body,
				Views: value.Views, Works: value.Works, Comments: value.Comments, Checklists: value.Checklists, URL: value.URL,
			}
		}
	} else {
		values, err := client.Search(ctx, dic.SearchRequest{Query: row.Query, Page: row.Page})
		row.Error = migrationDictionaryObserveError(err)
		if values != nil {
			row.Results = make([]migrationDictionarySearchResult, 0, len(values))
			for _, value := range values {
				row.Results = append(row.Results, migrationDictionarySearchResult{
					Title: value.Title, Summary: value.Summary, Updated: value.Updated, Views: value.Views,
					Works: value.Works, Checklists: value.Checklists, Related: value.Related, URL: value.URL, Thumbnail: value.Thumbnail,
				})
			}
		}
	}
	row.Requests = transport.requests
	if len(transport.responses) != 0 {
		t.Fatalf("dictionary left %d finite responses unused", len(transport.responses))
	}
	return row
}

func migrationDictionaryArticleJSON(t *testing.T, fields map[string]any) string {
	t.Helper()
	body, err := json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	return string(body)
}

func migrationDictionaryArticleRows(t *testing.T) []migrationDictionaryServiceRow {
	t.Helper()
	rows := []migrationDictionaryServiceRow{}
	add := func(name, ref, body string) *migrationDictionaryServiceRow {
		rows = append(rows, migrationDictionaryServiceRow{
			Name: name, Context: "background", Client: "normal", Ref: ref, SkipCounters: true,
			Responses: []migrationDictionaryResponse{{Body: body, Status: 200}},
		})
		return &rows[len(rows)-1]
	}
	success := add("existing-article-and-counters", "初音ミク", articleFixture)
	success.SkipCounters = false
	success.Responses = append(success.Responses, migrationDictionaryResponse{Body: countersFixture, Status: 200})
	add("skip-counters", "初音ミク", articleFixture)
	english := add("requested-english", "https://dic.pixiv.net/a/初音ミク", articleFixture)
	english.Language = "en"
	for _, state := range []string{"canceled", "deadline"} {
		row := add("ready-transport-succeeds-despite-"+state, "title", articleFixture)
		row.Context = state
	}
	for _, input := range []struct{ name, ref string }{
		{"trim-go-unicode-whitespace", "\u0085\u00a0\u2003 初音ミク \u3000\n"},
		{"title-slash-punctuation", "foo/bar +&=:@$!?%#'()[]"},
		{"bare-colon", "title:part"},
		{"bare-root-path", "/a/miku/"},
		{"english-url-defaults-japanese", "https://dic.pixiv.net/en/a/Hatsune_Miku"},
		{"host-case-port-userinfo-scheme", "ftp://fixture:password@DIC.PIXIV.NET:1234/prefix/a/title?ignored=yes#fragment"},
		{"scheme-relative-host", "//dic.pixiv.net/a/title"},
		{"hostless-scheme-is-title", "https:dic.pixiv.net/a/title"},
		{"invalid-percent-url-is-title", "https://dic.pixiv.net/a/%zz"},
		{"invalid-port-url-is-title", "https://dic.pixiv.net:abc/a/title"},
		{"inner-control-url-is-title", "https://dic.pixiv.net/a/one\ntwo"},
		{"first-a-segment-joins-rest", "https://dic.pixiv.net/prefix/a/one/a/two/"},
		{"encoded-a-segment", "https://dic.pixiv.net/%61/title"},
		{"decoded-encoded-slash", "https://dic.pixiv.net/a/foo%2Fbar"},
		{"double-unescape-slash", "https://dic.pixiv.net/a/foo%252Fbar"},
		{"double-unescape-unicode", "https://dic.pixiv.net/a/%25E5%2588%259D"},
		{"decoded-empty-path-segment", "https://dic.pixiv.net/a//tail"},
		{"double-unescape-only-slash", "https://dic.pixiv.net/a/%252F"},
		{"title-query-fragment-encoded", "https://dic.pixiv.net/a/title%3Fquery%23fragment?other=x#ignored"},
	} {
		add(input.name, input.ref, articleFixture)
	}
	for _, input := range []struct{ name, ref string }{
		{"empty-reference", ""}, {"whitespace-reference", "\u3000\n\t"},
		{"other-host", "https://example.invalid/a/title"}, {"trailing-dot-host", "https://dic.pixiv.net./a/title"},
		{"url-no-a", "https://dic.pixiv.net/en/title"}, {"url-no-title", "https://dic.pixiv.net/a/"},
		{"double-unescape-invalid-percent", "https://dic.pixiv.net/a/%25zz"},
	} {
		row := add(input.name, input.ref, "")
		row.Responses = []migrationDictionaryResponse{}
	}
	for _, language := range []string{"fr", " JA ", "JA", " ja", "en "} {
		row := add("invalid-language-"+fmt.Sprintf("%x", language), "title", "")
		row.Language, row.Responses = language, []migrationDictionaryResponse{}
	}
	row := add("reference-before-language", "", "")
	row.Language, row.Responses = "fr", []migrationDictionaryResponse{}
	for _, client := range []string{"nil", "nil-transport", "normal"} {
		row := add("nil-context-client-"+client, "", "")
		row.Context, row.Client, row.Responses = "nil", client, []migrationDictionaryResponse{}
	}
	for _, status := range []int{0, 199, 201, 204, 299, 300, 404, 429, 500} {
		row := add(fmt.Sprintf("article-status-%d", status), "title", articleFixture)
		row.Responses[0].Status = status
	}
	for _, kind := range []string{"fixture", "canceled", "deadline"} {
		row := add("article-transport-"+kind, "title", "private synthetic response must not enter the controlled message")
		row.Responses[0].Error = kind
		if kind != "fixture" {
			row.Context = kind
		}
	}
	for _, input := range []struct{ name, body string }{
		{"json-malformed", "{"}, {"json-trailing-document", `{"id":1,"tagName":"x"} {}`},
		{"json-null", "null"}, {"json-array", "[]"}, {"id-zero", `{"id":0,"tagName":"x"}`},
		{"id-negative", `{"id":-1,"tagName":"x"}`}, {"id-float", `{"id":1.5,"tagName":"x"}`},
		{"id-overflow", `{"id":9223372036854775808,"tagName":"x"}`},
		{"title-whitespace", `{"id":1,"tagName":"\u3000\n"}`},
		{"minimal-missing-optionals", `{"id":1,"tagName":"response/title"}`},
		{"nullable-optionals", `{"id":1,"tagName":"x","yomigana":null,"translatedTagName":null,"categories":null,"abstract":null,"nodes":null,"recommendedArticles":null}`},
		{"empty-collections", `{"id":1,"tagName":"x","categories":[],"recommendedArticles":[]}`},
		{"null-category-elements", `{"id":1,"tagName":"x","categories":[null," ","A"],"recommendedArticles":[null,{"tagName":null},{"tagName":" \u3000 Related \u0085"},{"tagName":""}]}`},
		{"categories-wrong-shape", `{"id":1,"tagName":"x","categories":"wrong"}`},
		{"recommended-wrong-shape", `{"id":1,"tagName":"x","recommendedArticles":["wrong"]}`},
		{"nodes-array-fails-wire", `{"id":1,"tagName":"x","nodes":[]}`},
		{"duplicate-case-insensitive-json-fields", `{"ID":3,"id":4,"TagName":"before","tagName":" last ","unknown":{"secret":"ignored"}}`},
		{"duplicate-scalars-followed-by-null", `{"id":3,"id":null,"tagName":"before","tagName":null,"yomigana":"reading","yomigana":null,"translatedTagName":"translation","translatedTagName":null,"abstract":"abstract","abstract":null,"nodes":"[{\"tag\":\"text\",\"text\":\"body\"}]","nodes":null}`},
		{"duplicate-collections-followed-by-null", `{"id":3,"tagName":"x","categories":["before"],"categories":null,"recommendedArticles":[{"tagName":"before"}],"recommendedArticles":null}`},
	} {
		add(input.name, "request/title", input.body)
	}
	nodes := `[null,{"tag":"text","text":" leading \r\n inner \n\n\n","children":[{"tag":"text","text":"ignored-text-child"}]},{"tag":"article_link","text":"ArticleLink","children":[{"tag":"text","text":"ignored-link-child"}]},{"tag":"external_link","text":" ExternalLink"},{"tag":"br","text":"ignored-br"},{"tag":"header","text":"ignored-header","children":[{"tag":"text","text":" Heading "},null]},{"tag":"sub_header","children":[{"tag":"text","text":" Subheading "}]},{"tag":"p","children":[{"tag":"text","text":" paragraph "},{"tag":"br"},{"tag":"text","text":" second \u3000"}]},{"tag":"list_item","children":[{"tag":"text","text":" item "}]},{"tag":"table_row","children":[{"tag":"table_header","children":[{"tag":"text","text":"H"}]},{"tag":"table_cell","children":[{"tag":"text","text":"C"}]}]},{"tag":"unknown","text":"ignored-unknown-text","children":[{"tag":"text","text":" nested "}]},{"children":[{"tag":"text","text":"end"}]}]`
	for _, input := range []struct{ name, nodes string }{
		{"body-all-renderer-branches", nodes}, {"body-empty", ""}, {"body-whitespace", " \u3000\n"},
		{"body-null", "null"}, {"body-empty-array", "[]"}, {"body-malformed", "["},
		{"body-wrong-shape", "{}"}, {"body-wrong-node-field", `[{"tag":1,"text":"x"}]`},
		{"body-null-string-fields", `[{"tag":"text","text":null},{"tag":null,"children":[{"tag":"text","text":"child"}]}]`},
	} {
		add(input.name, "body", migrationDictionaryArticleJSON(t, map[string]any{"id": 1, "tagName": "body", "nodes": input.nodes}))
	}
	for _, response := range []struct {
		name  string
		value migrationDictionaryResponse
	}{
		{"status", migrationDictionaryResponse{Body: "private counters failure", Status: 500}},
		{"not-found", migrationDictionaryResponse{Status: 404}},
		{"malformed", migrationDictionaryResponse{Body: "{", Status: 200}},
		{"null", migrationDictionaryResponse{Body: "null", Status: 200}},
		{"negative-values", migrationDictionaryResponse{Body: `{"articleViewCount":-1,"commentCount":-2,"pixivWorkCount":-3,"checklistCount":-4}`, Status: 200}},
		{"overflow", migrationDictionaryResponse{Body: `{"articleViewCount":1,"commentCount":9223372036854775808}`, Status: 200}},
		{"wrong-field-type", migrationDictionaryResponse{Body: `{"articleViewCount":1,"commentCount":"wrong"}`, Status: 200}},
		{"fractional-int", migrationDictionaryResponse{Body: `{"articleViewCount":1,"commentCount":1.5}`, Status: 200}},
		{"duplicate-scalar-then-null", migrationDictionaryResponse{Body: `{"articleViewCount":7,"articleViewCount":null,"commentCount":3,"commentCount":null,"unknown":{"ignored":"value"}}`, Status: 200}},
		{"transport", migrationDictionaryResponse{Error: "fixture"}},
		{"canceled", migrationDictionaryResponse{Error: "canceled"}},
		{"deadline", migrationDictionaryResponse{Error: "deadline"}},
	} {
		row := add("counters-best-effort-"+response.name, "request/title", articleFixture)
		row.SkipCounters = false
		row.Responses = append(row.Responses, response.value)
	}
	row = add("counters-after-context-canceled", "request/title", articleFixture)
	row.SkipCounters, row.Responses[0].CancelAfter = false, true
	row.Responses = append(row.Responses, migrationDictionaryResponse{Error: "context"})
	return rows
}

func migrationDictionarySearchRows() []migrationDictionaryServiceRow {
	rows := []migrationDictionaryServiceRow{}
	add := func(name, body string) *migrationDictionaryServiceRow {
		rows = append(rows, migrationDictionaryServiceRow{Name: name, Context: "background", Client: "normal", Query: "初音ミク", Page: 1, Responses: []migrationDictionaryResponse{{Body: body, Status: 200}}})
		return &rows[len(rows)-1]
	}
	add("existing-real-shaped-page", searchPageFixture)
	add("existing-empty-page", emptySearchPageFixture)
	add("existing-missing-main", missingContainerPageFixture)
	for _, state := range []string{"canceled", "deadline"} {
		row := add("ready-transport-succeeds-despite-"+state, searchPageFixture)
		row.Context = state
	}
	for _, page := range []int{-2, 0, 1, 3, 9223372036854775807} {
		row := add(fmt.Sprintf("page-%d-one-request", page), emptySearchPageFixture)
		row.Page = page
	}
	row := add("query-preserved-and-go-query-escape", emptySearchPageFixture)
	row.Query = " \t日本語 +/&?=#@:$'()[]~\u3000\n"
	for _, query := range []string{"", "\u0085\u00a0\u2003\u3000\n\t"} {
		row := add("empty-query-"+fmt.Sprintf("%x", query), "")
		row.Query, row.Responses = query, []migrationDictionaryResponse{}
	}
	for _, client := range []string{"nil", "nil-transport", "normal"} {
		row := add("nil-context-client-"+client, "")
		row.Context, row.Client, row.Query, row.Responses = "nil", client, "", []migrationDictionaryResponse{}
	}
	for _, status := range []int{0, 199, 201, 204, 299, 300, 404, 429, 503} {
		row := add(fmt.Sprintf("search-status-%d", status), "not even HTML; private body")
		row.Responses[0].Status = status
	}
	for _, kind := range []string{"fixture", "canceled", "deadline"} {
		row := add("search-transport-"+kind, "private synthetic response")
		row.Responses[0].Error = kind
		if kind != "fixture" {
			row.Context = kind
		}
	}
	add("main-first-only-document-order-and-nested-article", `<div id="main"><article><div class="info"><a href="/a/first">First</a></div><article><div class="info"><a href="/a/nested">Nested skipped</a></div></article></article><article><div class="info"><a href="/a/second">Second</a></div></article><article><div class="info"><a href="/a/first">First</a></div></article></div><div id="main"><article><div class="info"><a href="/a/ignored">Ignored second main</a></div></article></div>`)
	add("invalid-cards-and-first-info-first-href", `<div id="main"><article><a href="/a/no-info">Skip</a></article><article><div class="info">No anchor</div></article><article><div class="info"><a href="">Skip empty href</a></div></article><article><div class="info"><a href="/a/empty"> 　 </a><a href="/a/later">Later must not rescue first href</a></div></article><article><div class="info"><a>No href ignored</a><a href="/a/valid">  Valid <span>nested</span> title </a></div><div class="info"><a href="/a/ignored">Ignored</a></div></article></div>`)
	add("whitespace-entity-summary-cutoff-related-thumbnail", "<div id='main'><article><img src='../raw?x=1&amp;y=2'><img src='ignored'><div class='other info more'><a href='/a/title'> \u3000 A&amp;B\n <span>C</span><b>D</b> </a><p class='summary'> Before <b>bold <a href='x'>cutoff</a> lost</b> tail </p><p class='summary'>ignored</p><ul class='data'><li>更新: 2024/01/01 01:02:03</li><li>閲覧数: -1,234.5abc６７</li><li>作品数: no ASCII digits １２３</li><li>チェックリスト数: 99999999999999999999999999999999999999</li><li>Unknown: 99</li><li>更新: last:colon</li></ul><div class='relation'><ul><li> \u3000 </li><li> A <span>B</span> </li><li>same</li><li>same</li></ul></div></div></article></div>")
	add("counter-duplicate-label-last-value-and-first-data", `<div id="main"><article><div class="info"><a href="/a/x">X</a><ul class="data"><li>閲覧数:1</li><li>閲覧数:2</li><li>作品数:9223372036854775807</li><li>チェックリスト数:9223372036854775808</li><li>更新 no colon</li></ul><ul class="data"><li>閲覧数:99</li></ul></div></article></div>`)
	for _, href := range []string{"http://other.invalid/a/x", "https://other.invalid/a/x", "HTTP://other.invalid/a/x", "//other.invalid/a/x", "/a/x", "a/x", "../a/x", "?query=x", "#fragment", "javascript:fixture", " /a/x", "///a/x"} {
		add("href-"+fmt.Sprintf("%x", href), `<div id="main"><article><div class="info"><a href="`+href+`">Title</a></div></article></div>`)
	}
	add("html-parser-repairs-unclosed-tags", `<div id=main><article><div class=info><a href=/a/x> X &amp; Y</a><p class=summary>First<br>second`)
	add("empty-main-precedes-later-results", `<div id="main"></div><div id="main"><article><div class="info"><a href="/a/x">Ignored</a></div></article></div>`)
	return rows
}

func migrationDictionaryGuardSources(t *testing.T) {
	t.Helper()
	for path, want := range migrationDictionarySources {
		body, err := os.ReadFile(filepath.Join("../../..", path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(body)); got != want {
			t.Fatalf("worktree %s differs from frozen Go reference: %s", path, got)
		}
		command := exec.Command("git", "show", migrationDictionaryReference+":"+path)
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read frozen source %s: %v", path, err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(frozen)); got != want {
			t.Fatalf("frozen %s differs from source guard: %s", path, got)
		}
	}
}

func TestMigrationDictionaryService(t *testing.T) {
	migrationDictionaryGuardSources(t)
	fixture := migrationDictionaryFixture{
		Schema: 1, Reference: migrationDictionaryReference, Sources: migrationDictionarySources,
		Gaps: []string{
			"Anonymous internal service and actual HTTPTransport only; no frozen public SDK or MCP operation exists for dictionary.",
			"Article and search use finite synthetic Transport responses; HTTPTransport uses a caller-owned RoundTripper or owned HTTP loopback. No external dictionary, account, auth, media, browser, or native effect occurs.",
			"Loopback origin alone is normalized to <ORIGIN>; no other result, request, error, or body is normalized.",
			"Integer overflow observations are the frozen Go linux/amd64 int width; native Windows and all other platforms remain separate evidence.",
			"Live HTTPS, default environment proxy behavior, all URL/JSON/HTML grammar, full caller-owned client lifecycle, and upstream drift remain outside this bounded fixture.",
		},
	}
	for _, row := range migrationDictionaryArticleRows(t) {
		t.Run("article/"+row.Name, func(t *testing.T) {
			fixture.Articles = append(fixture.Articles, migrationDictionaryRunService(t, row, true))
		})
	}
	for _, row := range migrationDictionarySearchRows() {
		t.Run("search/"+row.Name, func(t *testing.T) {
			fixture.Searches = append(fixture.Searches, migrationDictionaryRunService(t, row, false))
		})
	}
	for _, row := range migrationDictionaryTransportRows() {
		t.Run("transport/"+row.Name, func(t *testing.T) {
			fixture.Transports = append(fixture.Transports, migrationDictionaryRunHTTPTransport(t, row))
		})
	}
	if t.Failed() {
		return
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	const path = "../../../crates/pixiv-cli/tests/fixtures/dictionary-service.json"
	if *updateDictionaryService {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
	} else {
		expected, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var want migrationDictionaryFixture
		if err := json.Unmarshal(expected, &want); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(want, fixture) {
			t.Fatalf("dictionary service contract changed\ngot: %s\nwant: %s", body, expected)
		}
	}
	t.Logf("frozen dictionary rows: articles=%d searches=%d transports=%d", len(fixture.Articles), len(fixture.Searches), len(fixture.Transports))
}

func TestMigrationDictionaryErrorClassification(t *testing.T) {
	if dic.CodeOf(nil) != dic.CodeUnknown || dic.CodeOf(migrationDictionaryCause) != dic.CodeUnknown {
		t.Fatal("nil and plain errors must retain unknown classification")
	}
	cause := dic.NewError(dic.CodeTransport, "controlled", context.Canceled)
	wrapped := fmt.Errorf("outer: %w", cause)
	if dic.CodeOf(wrapped) != dic.CodeTransport || !errors.Is(wrapped, context.Canceled) {
		t.Fatal("wrapped dictionary errors must retain classification and source")
	}
	var typed *dic.Error
	if !errors.As(wrapped, &typed) || typed != cause || typed.StatusCode() != 0 || typed.Unwrap() != context.Canceled {
		t.Fatal("wrapped dictionary errors must preserve the original typed error")
	}
	if strings.Contains(cause.Error(), "context canceled") {
		t.Fatal("dictionary error messages must remain controlled even with a source")
	}
}
