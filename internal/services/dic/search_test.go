package dic_test

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

// searchPageFixture mirrors the card markup of a real result page, including
// the Japanese counter labels and a card that only has the required fields.
const searchPageFixture = `<!DOCTYPE html>
<html lang="ja">
<head><meta charset="utf-8"><title>「初音ミク」の検索結果 - ピクシブ百科事典</title></head>
<body>
<div id="contents">
  <div id="main">
    <div class="search-header">「初音ミク」の検索結果</div>
    <article class="dictionary-entry">
      <a class="thumbnail" href="/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF">
        <img src="https://i.pximg.net/c/240x480/img-master/img/2024/01/01/00/00/00/100_p0_master1200.jpg" alt="初音ミク">
      </a>
      <div class="info">
        <h2><a href="/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF">初音ミク</a></h2>
        <p class="summary">クリプトン・フューチャー・メディアが開発したバーチャルシンガー。<a href="/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF">続きを読む</a></p>
        <ul class="data">
          <li>更新: 2024/05/01 12:34</li>
          <li>閲覧数: 1,234,567</li>
          <li>作品数: 98,765</li>
          <li>チェックリスト数: 432</li>
        </ul>
        <div class="relation">
          <ul><li>VOCALOID</li><li>クリプトン</li></ul>
        </div>
      </div>
    </article>
    <article class="dictionary-entry">
      <div class="info">
        <h2><a href="https://dic.pixiv.net/a/%E3%83%9F%E3%82%AF">ミク</a></h2>
        <p class="summary">曖昧さ回避のためのページ。</p>
      </div>
    </article>
  </div>
</div>
</body>
</html>`

// emptySearchPageFixture is the valid zero-result page the site returns with
// HTTP 404 for a query that matched nothing.
const emptySearchPageFixture = `<!DOCTYPE html>
<html lang="ja"><body><div id="main"><p class="no-result">「zzzz」に一致する記事は見つかりませんでした。</p></div></body></html>`

// missingContainerPageFixture has no result container.
const missingContainerPageFixture = `<!DOCTYPE html>
<html lang="ja"><body><div id="content"><p>本文</p></div></body></html>`

func TestSearchParsesRealShapedResultPage(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(searchPageFixture)}}
	results, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "初音ミク"})
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	wantURL := "https://dic.pixiv.net/search?p=1&query=%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF"
	if len(transport.requests) != 1 || transport.requests[0] != wantURL {
		t.Fatalf("requests = %v, want [%q]", transport.requests, wantURL)
	}
	if len(transport.accepts) != 1 || transport.accepts[0] != "text/html" {
		t.Fatalf("accepts = %v, want [text/html]", transport.accepts)
	}
	if len(results) != 2 {
		t.Fatalf("results = %d, want 2", len(results))
	}
	first := results[0]
	if first.Title != "初音ミク" || first.URL != "https://dic.pixiv.net/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF" {
		t.Fatalf("first title/url = %q %q", first.Title, first.URL)
	}
	if first.Summary != "クリプトン・フューチャー・メディアが開発したバーチャルシンガー。" {
		t.Fatalf("first summary = %q", first.Summary)
	}
	if first.Updated != "2024/05/01 12:34" || first.Views != 1234567 || first.Works != 98765 || first.Checklists != 432 {
		t.Fatalf("first counters = %q %d %d %d", first.Updated, first.Views, first.Works, first.Checklists)
	}
	if strings.Join(first.Related, ",") != "VOCALOID,クリプトン" {
		t.Fatalf("first related = %v", first.Related)
	}
	if first.Thumbnail != "https://i.pximg.net/c/240x480/img-master/img/2024/01/01/00/00/00/100_p0_master1200.jpg" {
		t.Fatalf("first thumbnail = %q", first.Thumbnail)
	}
	second := results[1]
	if second.Title != "ミク" || second.URL != "https://dic.pixiv.net/a/%E3%83%9F%E3%82%AF" || second.Summary != "曖昧さ回避のためのページ。" {
		t.Fatalf("second = %#v", second)
	}
	if second.Views != 0 || second.Works != 0 || second.Checklists != 0 || second.Updated != "" || second.Thumbnail != "" || len(second.Related) != 0 {
		t.Fatalf("second optional fields = %#v", second)
	}
}

func TestSearchNotFoundMeansEmptyResult(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{{body: emptySearchPageFixture, statusCode: 404}}}
	results, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "zzzz"})
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if results == nil || len(results) != 0 {
		t.Fatalf("results = %#v, want non-nil empty slice", results)
	}
	if len(transport.requests) != 1 {
		t.Fatalf("requests = %v, want exactly one request", transport.requests)
	}
}

func TestSearchEmptyResultPageReturnsEmptySlice(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(emptySearchPageFixture)}}
	results, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "zzzz"})
	if err != nil || results == nil || len(results) != 0 {
		t.Fatalf("results = %#v, err = %v", results, err)
	}
}

func TestSearchUsesRequestedPage(t *testing.T) {
	cases := []struct {
		page     int
		wantPage string
	}{
		{page: 0, wantPage: "p=1"},
		{page: 1, wantPage: "p=1"},
		{page: 3, wantPage: "p=3"},
		{page: -2, wantPage: "p=1"},
	}
	for _, testCase := range cases {
		transport := &stubTransport{responses: []stubResponse{ok(emptySearchPageFixture)}}
		if _, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "miku", Page: testCase.page}); err != nil {
			t.Fatalf("page %d: %v", testCase.page, err)
		}
		want := "https://dic.pixiv.net/search?" + testCase.wantPage + "&query=miku"
		if len(transport.requests) != 1 || transport.requests[0] != want {
			t.Fatalf("page %d requests = %v, want [%q]", testCase.page, transport.requests, want)
		}
	}
}

func TestSearchRejectsEmptyQuery(t *testing.T) {
	for _, query := range []string{"", "   "} {
		transport := &stubTransport{}
		_, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: query})
		if dic.CodeOf(err) != dic.CodeInvalidRequest {
			t.Fatalf("query %q error code = %v, want %v", query, dic.CodeOf(err), dic.CodeInvalidRequest)
		}
		if len(transport.requests) != 0 {
			t.Fatalf("query %q issued %v, want no request", query, transport.requests)
		}
	}
}

func TestSearchMalformedPageIsClassified(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(missingContainerPageFixture)}}
	_, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "miku"})
	if dic.CodeOf(err) != dic.CodeMalformedResponse {
		t.Fatalf("error code = %v, want %v", dic.CodeOf(err), dic.CodeMalformedResponse)
	}
}

func TestSearchUnexpectedStatusIsClassified(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{{body: "", statusCode: 503}}}
	_, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "miku"})
	if dic.CodeOf(err) != dic.CodeUpstreamStatus {
		t.Fatalf("error code = %v, want %v", dic.CodeOf(err), dic.CodeUpstreamStatus)
	}
	var classified *dic.Error
	if !errors.As(err, &classified) || classified.StatusCode() != 503 {
		t.Fatalf("error = %v, want status 503", err)
	}
}
