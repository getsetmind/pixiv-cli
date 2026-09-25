package dic

import (
	"bytes"
	"context"
	"errors"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"golang.org/x/net/html"
)

// SearchRequest selects one page of article search results.
type SearchRequest struct {
	// Query is the search text. It must not be empty.
	Query string
	// Page is the 1-based result page. Zero and negative values select the
	// first page.
	Page int
}

// SearchResult is one article card from a search result page.
type SearchResult struct {
	Title      string
	Summary    string
	Updated    string
	Views      int
	Works      int
	Checklists int
	Related    []string
	URL        string
	Thumbnail  string
}

// errSearchPageUnparsable reports a search page without the result container
// the site always renders.
var errSearchPageUnparsable = errors.New("search page has no result container")

// Search returns the article cards of one search page. A query with no matches
// returns an empty, non-nil slice and a nil error: the upstream 404 for a
// zero-hit query is an empty result, not a failure.
func (c *Client) Search(ctx context.Context, request SearchRequest) ([]SearchResult, error) {
	if c == nil || c.transport == nil {
		return nil, NewError(CodeTransport, "dic transport is not configured", nil)
	}
	if ctx == nil {
		return nil, NewError(CodeInvalidRequest, "dic request context is required", nil)
	}
	if strings.TrimSpace(request.Query) == "" {
		return nil, NewError(CodeInvalidRequest, "search query is required", nil)
	}
	page := request.Page
	if page < 1 {
		page = 1
	}
	values := url.Values{}
	values.Set("query", request.Query)
	values.Set("p", strconv.Itoa(page))

	body, statusCode, err := c.transport.Get(ctx, apiBaseURL+"/search?"+values.Encode(), "text/html")
	if err != nil {
		return nil, NewError(CodeTransport, "dic search request failed", err)
	}
	// The site answers a query that matched nothing with 404 and a valid
	// zero-result page, so 404 here is an empty result, not a failure.
	if statusCode == http.StatusNotFound {
		return []SearchResult{}, nil
	}
	if statusCode < http.StatusOK || statusCode >= http.StatusMultipleChoices {
		return nil, newHTTPStatusError(CodeUpstreamStatus, "dic.pixiv.net search returned an unexpected HTTP status", statusCode)
	}

	results, parseErr := parseSearchPage(body)
	if parseErr != nil {
		return nil, NewError(CodeMalformedResponse, "dic.pixiv.net search page could not be parsed", nil)
	}
	return results, nil
}

// parseSearchPage extracts the article cards from a search result page.
func parseSearchPage(body []byte) ([]SearchResult, error) {
	document, err := html.Parse(bytes.NewReader(body))
	if err != nil {
		return nil, errSearchPageUnparsable
	}
	main, ok := (element{node: document}).first(func(candidate element) bool {
		return candidate.is("div") && candidate.attr("id") == "main"
	})
	if !ok {
		return nil, errSearchPageUnparsable
	}
	results := make([]SearchResult, 0)
	main.each("article", func(card element) {
		if result, ok := readSearchCard(card); ok {
			results = append(results, result)
		}
	})
	return results, nil
}

// readSearchCard maps one <article> card. A card without the title link the
// site always renders is skipped rather than rejected, so unrelated markup
// drift does not discard the rest of the page.
func readSearchCard(card element) (SearchResult, bool) {
	info, ok := card.first(func(candidate element) bool {
		return candidate.is("div") && candidate.hasClass("info")
	})
	if !ok {
		return SearchResult{}, false
	}
	link, ok := info.first(func(candidate element) bool {
		return candidate.is("a") && candidate.attr("href") != ""
	})
	if !ok {
		return SearchResult{}, false
	}
	title := link.text()
	if title == "" {
		return SearchResult{}, false
	}
	result := SearchResult{Title: title, URL: absoluteArticleURL(link.attr("href"))}
	if image, found := card.first(func(candidate element) bool { return candidate.is("img") }); found {
		result.Thumbnail = image.attr("src")
	}
	if summary, found := info.first(func(candidate element) bool {
		return candidate.is("p") && candidate.hasClass("summary")
	}); found {
		result.Summary = summary.textBefore("a")
	}
	if data, found := info.first(func(candidate element) bool {
		return candidate.is("ul") && candidate.hasClass("data")
	}); found {
		readCardCounters(data, &result)
	}
	if relation, found := info.first(func(candidate element) bool {
		return candidate.is("div") && candidate.hasClass("relation")
	}); found {
		relation.each("li", func(item element) {
			if name := item.text(); name != "" {
				result.Related = append(result.Related, name)
			}
		})
	}
	return result, true
}

// readCardCounters reads the labelled counter lines under an article card. The
// labels are the Japanese ones the site renders.
func readCardCounters(data element, result *SearchResult) {
	data.each("li", func(item element) {
		label, value, found := strings.Cut(item.text(), ":")
		if !found {
			return
		}
		switch strings.TrimSpace(label) {
		case "更新":
			result.Updated = strings.TrimSpace(value)
		case "閲覧数":
			result.Views = displayInt(value)
		case "作品数":
			result.Works = displayInt(value)
		case "チェックリスト数":
			result.Checklists = displayInt(value)
		}
	})
}

// absoluteArticleURL turns a site-relative href into an absolute URL.
func absoluteArticleURL(href string) string {
	if href == "" || strings.HasPrefix(href, "http://") || strings.HasPrefix(href, "https://") {
		return href
	}
	return apiBaseURL + "/" + strings.TrimPrefix(href, "/")
}

// displayInt parses a display integer, tolerating thousands separators.
func displayInt(value string) int {
	digits := strings.Map(func(r rune) rune {
		if r >= '0' && r <= '9' {
			return r
		}
		return -1
	}, value)
	parsed, _ := strconv.Atoi(digits)
	return parsed
}

// element is the small DOM query surface the search page needs. Every method
// inspects element nodes only; the encyclopedia cards are element trees.
type element struct{ node *html.Node }

func (e element) is(tag string) bool {
	return e.node != nil && e.node.Type == html.ElementNode && e.node.Data == tag
}

func (e element) attr(key string) string {
	if e.node == nil {
		return ""
	}
	for _, attribute := range e.node.Attr {
		if attribute.Key == key {
			return attribute.Val
		}
	}
	return ""
}

func (e element) hasClass(class string) bool {
	for _, name := range strings.Fields(e.attr("class")) {
		if name == class {
			return true
		}
	}
	return false
}

// first returns the first element in document order, including e itself, for
// which match reports true.
func (e element) first(match func(element) bool) (element, bool) {
	if e.node == nil {
		return element{}, false
	}
	if e.node.Type == html.ElementNode && match(e) {
		return e, true
	}
	for child := e.node.FirstChild; child != nil; child = child.NextSibling {
		if found, ok := (element{node: child}).first(match); ok {
			return found, true
		}
	}
	return element{}, false
}

// each visits every descendant element named tag, without descending into a
// match, so nested containers are visited once.
func (e element) each(tag string, visit func(element)) {
	if e.node == nil {
		return
	}
	if e.is(tag) {
		visit(e)
		return
	}
	for child := e.node.FirstChild; child != nil; child = child.NextSibling {
		(element{node: child}).each(tag, visit)
	}
}

// text is the whitespace-collapsed text of e and its descendants.
func (e element) text() string {
	if e.node == nil {
		return ""
	}
	var builder strings.Builder
	var walk func(*html.Node)
	walk = func(node *html.Node) {
		if node.Type == html.TextNode {
			builder.WriteString(node.Data)
		}
		for child := node.FirstChild; child != nil; child = child.NextSibling {
			walk(child)
		}
	}
	walk(e.node)
	return strings.Join(strings.Fields(builder.String()), " ")
}

// textBefore is text, but stops at the first element named tag. A summary that
// ends in the site's "read more" link therefore keeps only its prose.
func (e element) textBefore(tag string) string {
	if e.node == nil {
		return ""
	}
	var builder strings.Builder
	var walk func(*html.Node) bool
	walk = func(node *html.Node) bool {
		if node.Type == html.ElementNode && node.Data == tag {
			return false
		}
		if node.Type == html.TextNode {
			builder.WriteString(node.Data)
		}
		for child := node.FirstChild; child != nil; child = child.NextSibling {
			if !walk(child) {
				return false
			}
		}
		return true
	}
	walk(e.node)
	return strings.Join(strings.Fields(builder.String()), " ")
}
