package dic_test

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

const articleFixture = `{
  "id": 12345,
  "tagName": "初音ミク",
  "yomigana": "はつねみく",
  "translatedTagName": "Hatsune Miku",
  "categories": ["VOCALOID", "キャラクター"],
  "abstract": "クリプトン・フューチャー・メディアが開発したバーチャルシンガー。",
  "nodes": "[{\"tag\":\"text\",\"text\":\"初音ミクは...\"},{\"tag\":\"br\"},{\"tag\":\"header\",\"children\":[{\"tag\":\"text\",\"text\":\"概要\"}]},{\"tag\":\"p\",\"children\":[{\"tag\":\"text\",\"text\":\"本文です。\"}]},{\"tag\":\"list_item\",\"children\":[{\"tag\":\"text\",\"text\":\"項目\"}]}]",
  "recommendedArticles": [{"tagName": "VOCALOID"}, {"tagName": "クリプトン"}, {"tagName": ""}]
}`

const countersFixture = `{"articleViewCount": 1000, "commentCount": 5, "pixivWorkCount": 200, "checklistCount": 7}`

const articlePath = "https://dic.pixiv.net/_api/get_article/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF"
const countersPath = "https://dic.pixiv.net/_api/get_article_info/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF"

func TestArticleAcceptsBareTitleAndArticleURL(t *testing.T) {
	cases := []struct {
		name string
		ref  string
		want string
	}{
		{name: "bare title", ref: "初音ミク", want: articlePath + "?lang=ja"},
		{name: "japanese article URL", ref: "https://dic.pixiv.net/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF", want: articlePath + "?lang=ja"},
		{name: "english article URL", ref: "https://dic.pixiv.net/en/a/Hatsune_Miku", want: "https://dic.pixiv.net/_api/get_article/Hatsune_Miku?lang=ja"},
		{name: "title with slash", ref: "foo/bar", want: "https://dic.pixiv.net/_api/get_article/foo%2Fbar?lang=ja"},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			transport := &stubTransport{responses: []stubResponse{ok(articleFixture)}}
			if _, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: testCase.ref, SkipCounters: true}); err != nil {
				t.Fatalf("Article: %v", err)
			}
			if len(transport.requests) != 1 || transport.requests[0] != testCase.want {
				t.Fatalf("requests = %v, want [%q]", transport.requests, testCase.want)
			}
		})
	}
}

func TestArticleRejectsInvalidReferences(t *testing.T) {
	for _, ref := range []string{"", "   ", "https://example.com/a/miku", "https://dic.pixiv.net/", "https://dic.pixiv.net/users/1"} {
		transport := &stubTransport{}
		_, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: ref, SkipCounters: true})
		if dic.CodeOf(err) != dic.CodeInvalidReference {
			t.Fatalf("ref %q error code = %v, want %v", ref, dic.CodeOf(err), dic.CodeInvalidReference)
		}
		if len(transport.requests) != 0 {
			t.Fatalf("ref %q issued %v, want no request", ref, transport.requests)
		}
	}
}

func TestArticleDecodesFieldsAndCounters(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(articleFixture), ok(countersFixture)}}
	article, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク"})
	if err != nil {
		t.Fatalf("Article: %v", err)
	}
	if len(transport.requests) != 2 || transport.requests[0] != articlePath+"?lang=ja" || transport.requests[1] != countersPath+"?lang=ja" {
		t.Fatalf("requests = %v", transport.requests)
	}
	if len(transport.accepts) != 2 || transport.accepts[0] != "application/json" || transport.accepts[1] != "application/json" {
		t.Fatalf("accepts = %v", transport.accepts)
	}
	if article.ID != 12345 || article.Title != "初音ミク" || article.Yomigana != "はつねみく" || article.Translation != "Hatsune Miku" {
		t.Fatalf("article identity = %#v", article)
	}
	if strings.Join(article.Categories, ",") != "VOCALOID,キャラクター" {
		t.Fatalf("categories = %v", article.Categories)
	}
	if article.Abstract != "クリプトン・フューチャー・メディアが開発したバーチャルシンガー。" {
		t.Fatalf("abstract = %q", article.Abstract)
	}
	if strings.Join(article.Related, ",") != "VOCALOID,クリプトン" {
		t.Fatalf("related = %v", article.Related)
	}
	wantBody := "初音ミクは...\n\n## 概要\n\n本文です。\n\n- 項目"
	if article.Body != wantBody {
		t.Fatalf("body = %q, want %q", article.Body, wantBody)
	}
	if article.Views != 1000 || article.Works != 200 || article.Comments != 5 || article.Checklists != 7 {
		t.Fatalf("counters = %d %d %d %d", article.Views, article.Works, article.Comments, article.Checklists)
	}
	if article.URL != "https://dic.pixiv.net/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF" {
		t.Fatalf("url = %q", article.URL)
	}
}

func TestArticleUsesRequestedLanguage(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(articleFixture), ok(countersFixture)}}
	article, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク", Language: dic.LanguageEnglish})
	if err != nil {
		t.Fatalf("Article: %v", err)
	}
	if len(transport.requests) != 2 || transport.requests[0] != articlePath+"?lang=en" || transport.requests[1] != countersPath+"?lang=en" {
		t.Fatalf("requests = %v", transport.requests)
	}
	if article.URL != "https://dic.pixiv.net/en/a/%E5%88%9D%E9%9F%B3%E3%83%9F%E3%82%AF" {
		t.Fatalf("english url = %q", article.URL)
	}
}

func TestArticleRejectsUnsupportedLanguage(t *testing.T) {
	transport := &stubTransport{}
	_, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "miku", Language: dic.Language("fr")})
	if dic.CodeOf(err) != dic.CodeInvalidRequest {
		t.Fatalf("error code = %v, want %v", dic.CodeOf(err), dic.CodeInvalidRequest)
	}
	if len(transport.requests) != 0 {
		t.Fatalf("requests = %v, want none", transport.requests)
	}
}

func TestArticleSkipCountersIssuesExactlyOneRequest(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(articleFixture)}}
	article, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク", SkipCounters: true})
	if err != nil {
		t.Fatalf("Article: %v", err)
	}
	if len(transport.requests) != 1 {
		t.Fatalf("requests = %v, want exactly one", transport.requests)
	}
	if article.Views != 0 || article.Works != 0 || article.Comments != 0 || article.Checklists != 0 {
		t.Fatalf("counters = %d %d %d %d, want zero", article.Views, article.Works, article.Comments, article.Checklists)
	}
}

func TestArticleCountersFailureKeepsArticle(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{ok(articleFixture), {body: "", statusCode: 500}}}
	article, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク"})
	if err != nil {
		t.Fatalf("Article: %v", err)
	}
	if len(transport.requests) != 2 {
		t.Fatalf("requests = %v, want two", transport.requests)
	}
	if article.Title != "初音ミク" || article.Views != 0 || article.Works != 0 || article.Comments != 0 || article.Checklists != 0 {
		t.Fatalf("article = %#v, want article with zero counters", article)
	}
}

func TestArticleNotFoundIsClassified(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{{body: "", statusCode: 404}}}
	_, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク", SkipCounters: true})
	if dic.CodeOf(err) != dic.CodeNotFound {
		t.Fatalf("error code = %v, want %v", dic.CodeOf(err), dic.CodeNotFound)
	}
	var classified *dic.Error
	if !errors.As(err, &classified) || classified.StatusCode() != 404 {
		t.Fatalf("error = %v, want status 404", err)
	}
}

func TestArticleMalformedResponseIsClassified(t *testing.T) {
	for _, body := range []string{"{", `{"id":0,"tagName":"x"}`, `{"id":1,"tagName":"  "}`} {
		transport := &stubTransport{responses: []stubResponse{ok(body)}}
		_, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "miku", SkipCounters: true})
		if dic.CodeOf(err) != dic.CodeMalformedResponse {
			t.Fatalf("body %q error code = %v, want %v", body, dic.CodeOf(err), dic.CodeMalformedResponse)
		}
	}
}

func TestArticleTransportFailurePreservesCause(t *testing.T) {
	sentinel := errors.New("dial failed")
	transport := &stubTransport{responses: []stubResponse{{err: sentinel}}}
	_, err := dic.New(transport).Article(context.Background(), dic.ArticleRequest{Ref: "miku", SkipCounters: true})
	if dic.CodeOf(err) != dic.CodeTransport {
		t.Fatalf("error code = %v, want %v", dic.CodeOf(err), dic.CodeTransport)
	}
	if !errors.Is(err, sentinel) {
		t.Fatalf("error = %v, want the transport cause", err)
	}
}
