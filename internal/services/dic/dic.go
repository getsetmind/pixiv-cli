// Package dic implements the Pixiv encyclopedia (dic.pixiv.net) read client.
//
// It reads the three upstream shapes the encyclopedia exposes: the search
// result HTML, the article JSON, and the article counter JSON. The search
// answers a query with no matches using HTTP 404 and a valid zero-result page,
// so a search 404 means an empty result rather than a failure. The counters are
// an optional second request; a caller may skip them, and a failed counters
// request never fails the article fetch.
package dic

import (
	"context"
	"net/url"
	"strings"
)

// apiBaseURL is the origin of every encyclopedia request. It is a constant so a
// request never follows a caller-supplied host.
const apiBaseURL = "https://dic.pixiv.net"

// Language selects the article language. The Japanese and English articles of
// one concept are separate articles with different ids; the article's
// Translation field is the only bridge between them.
type Language string

const (
	// LanguageJapanese is the default article language.
	LanguageJapanese Language = "ja"
	// LanguageEnglish is the English article language.
	LanguageEnglish Language = "en"
)

// Transport performs one HTTP GET against the encyclopedia. It is the narrow
// seam between the client and net/http; HTTPTransport is the production
// implementation and tests may substitute another.
//
// Get returns the response body and its HTTP status code. A non-2xx status is
// not an error: the client interprets the status. A transport-level failure
// returns an error, in which case body and statusCode are unspecified. The
// returned body is fully read and owned by the caller.
type Transport interface {
	Get(ctx context.Context, rawURL string, accept string) (body []byte, statusCode int, err error)
}

// Client reads the Pixiv encyclopedia through a Transport.
type Client struct{ transport Transport }

// New returns a Client that reads through transport. A nil transport is
// accepted; every operation then fails with CodeTransport.
func New(transport Transport) *Client { return &Client{transport: transport} }

// parseArticleReference accepts a bare article title or a dic.pixiv.net
// article URL and returns the article title. A bare title is returned
// unchanged, including non-ASCII text and "/" characters; a URL is decoded
// from its path.
func parseArticleReference(reference string) (string, error) {
	trimmed := strings.TrimSpace(reference)
	if trimmed == "" {
		return "", NewError(CodeInvalidReference, "article reference is empty", nil)
	}
	parsed, err := url.Parse(trimmed)
	if err != nil || parsed.Host == "" {
		return trimmed, nil
	}
	if !strings.EqualFold(parsed.Hostname(), "dic.pixiv.net") {
		return "", NewError(CodeInvalidReference, "article reference must be a dic.pixiv.net URL or a bare title", nil)
	}
	segments := strings.Split(strings.Trim(parsed.Path, "/"), "/")
	for index, segment := range segments {
		if segment != "a" {
			continue
		}
		title := strings.Join(segments[index+1:], "/")
		if title == "" {
			break
		}
		decoded, decodeErr := url.PathUnescape(title)
		if decodeErr != nil || decoded == "" {
			break
		}
		return decoded, nil
	}
	return "", NewError(CodeInvalidReference, "article URL has no article title", nil)
}

// articleAPIPath is the article endpoint path for one title.
func articleAPIPath(title string) string {
	return "/_api/get_article/" + url.PathEscape(title)
}

// countersAPIPath is the article counter endpoint path for one title.
func countersAPIPath(title string) string {
	return "/_api/get_article_info/" + url.PathEscape(title)
}

// articlePageURL is the live page of an article in one language.
func articlePageURL(language Language, title string) string {
	prefix := "/a/"
	if language == LanguageEnglish {
		prefix = "/en/a/"
	}
	return apiBaseURL + prefix + url.PathEscape(title)
}

// validLanguage reports whether language is an article language the
// encyclopedia serves.
func validLanguage(language Language) bool {
	return language == LanguageJapanese || language == LanguageEnglish
}
