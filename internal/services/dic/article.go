package dic

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
)

// ArticleRequest selects one encyclopedia article.
type ArticleRequest struct {
	// Ref is a bare article title or a dic.pixiv.net article URL.
	Ref string
	// Language selects the article language. The zero value selects Japanese.
	Language Language
	// SkipCounters skips the optional second request that carries view, work,
	// comment, and checklist counters. The counter fields then stay zero and
	// Article issues exactly one request.
	SkipCounters bool
}

// Article is one encyclopedia article.
type Article struct {
	// ID is the upstream article id. The Japanese and English articles of one
	// concept have different ids.
	ID int
	// Title is the article title in the requested language.
	Title string
	// Yomigana is the Japanese reading of the title, when the article has one.
	Yomigana string
	// Translation is the article title in the other language. It is the only
	// field that bridges the separate Japanese and English articles.
	Translation string
	Categories  []string
	Abstract    string
	// Related lists the titles of recommended articles.
	Related []string
	// Body is the article body rendered from its node tree.
	Body string
	// Views, Works, Comments, and Checklists are the counters. They stay zero
	// when the counters request is skipped or fails.
	Views      int
	Works      int
	Comments   int
	Checklists int
	// URL is the live article page in the requested language.
	URL string
}

// Article fetches one article, optionally with its counters.
func (c *Client) Article(ctx context.Context, request ArticleRequest) (Article, error) {
	if c == nil || c.transport == nil {
		return Article{}, NewError(CodeTransport, "dic transport is not configured", nil)
	}
	if ctx == nil {
		return Article{}, NewError(CodeInvalidRequest, "dic request context is required", nil)
	}
	title, err := parseArticleReference(request.Ref)
	if err != nil {
		return Article{}, err
	}
	language := request.Language
	if language == "" {
		language = LanguageJapanese
	}
	if !validLanguage(language) {
		return Article{}, NewError(CodeInvalidRequest, "article language must be ja or en", nil)
	}

	body, statusCode, err := c.transport.Get(ctx, apiBaseURL+articleAPIPath(title)+"?lang="+string(language), "application/json")
	if err != nil {
		return Article{}, NewError(CodeTransport, "dic article request failed", err)
	}
	switch {
	case statusCode == http.StatusNotFound:
		return Article{}, newHTTPStatusError(CodeNotFound, "dic.pixiv.net article was not found", statusCode)
	case statusCode < http.StatusOK || statusCode >= http.StatusMultipleChoices:
		return Article{}, newHTTPStatusError(CodeUpstreamStatus, "dic.pixiv.net article request returned an unexpected HTTP status", statusCode)
	}

	var wire articleDTO
	if err := json.Unmarshal(body, &wire); err != nil {
		return Article{}, NewError(CodeMalformedResponse, "dic.pixiv.net article response could not be decoded", nil)
	}
	if wire.ID <= 0 || strings.TrimSpace(wire.TagName) == "" {
		return Article{}, NewError(CodeMalformedResponse, "dic.pixiv.net article response is missing required fields", nil)
	}

	article := Article{
		ID:          wire.ID,
		Title:       wire.TagName,
		Yomigana:    wire.Yomigana,
		Translation: wire.TranslatedTagName,
		Categories:  append([]string(nil), wire.Categories...),
		Abstract:    wire.Abstract,
		Related:     relatedTitles(wire.RecommendedArticles),
		Body:        renderNodes(wire.Nodes),
		URL:         articlePageURL(language, wire.TagName),
	}
	if !request.SkipCounters {
		applyCounters(ctx, c.transport, title, language, &article)
	}
	return article, nil
}

// applyCounters fills the optional counters. A failed counters request is not
// fatal: the article is returned with the counter fields at zero.
func applyCounters(ctx context.Context, transport Transport, title string, language Language, article *Article) {
	body, statusCode, err := transport.Get(ctx, apiBaseURL+countersAPIPath(title)+"?lang="+string(language), "application/json")
	if err != nil || statusCode < http.StatusOK || statusCode >= http.StatusMultipleChoices {
		return
	}
	var wire countersDTO
	if err := json.Unmarshal(body, &wire); err != nil {
		return
	}
	article.Views = wire.ArticleViewCount
	article.Works = wire.PixivWorkCount
	article.Comments = wire.CommentCount
	article.Checklists = wire.ChecklistCount
}

// relatedTitles renders the recommended-article list as titles, skipping empty
// entries.
func relatedTitles(related []relatedDTO) []string {
	titles := make([]string, 0, len(related))
	for _, entry := range related {
		if title := strings.TrimSpace(entry.TagName); title != "" {
			titles = append(titles, title)
		}
	}
	return titles
}

// articleDTO is the get_article response. Only the fields the client maps are
// declared.
type articleDTO struct {
	ID                  int          `json:"id"`
	TagName             string       `json:"tagName"`
	Yomigana            string       `json:"yomigana"`
	TranslatedTagName   string       `json:"translatedTagName"`
	Categories          []string     `json:"categories"`
	Abstract            string       `json:"abstract"`
	Nodes               string       `json:"nodes"`
	RecommendedArticles []relatedDTO `json:"recommendedArticles"`
}

// relatedDTO is one recommended-article entry.
type relatedDTO struct {
	TagName string `json:"tagName"`
}

// countersDTO is the get_article_info response.
type countersDTO struct {
	ArticleViewCount int `json:"articleViewCount"`
	CommentCount     int `json:"commentCount"`
	PixivWorkCount   int `json:"pixivWorkCount"`
	ChecklistCount   int `json:"checklistCount"`
}

// nodeDTO is one node of the article body. The article endpoint ships the body
// as a JSON string holding an array of these nodes.
type nodeDTO struct {
	Tag      string     `json:"tag"`
	Text     string     `json:"text"`
	Children []*nodeDTO `json:"children"`
}

// renderNodes renders the article body node tree as plain text. A body that
// cannot be decoded renders as empty rather than failing the article.
func renderNodes(raw string) string {
	if strings.TrimSpace(raw) == "" {
		return ""
	}
	var nodes []*nodeDTO
	if err := json.Unmarshal([]byte(raw), &nodes); err != nil {
		return ""
	}
	renderer := &nodeRenderer{}
	renderer.render(nodes)
	return tidyLines(renderer.builder.String())
}

// nodeRenderer accumulates the rendered body.
type nodeRenderer struct{ builder strings.Builder }

func (r *nodeRenderer) render(nodes []*nodeDTO) {
	for _, node := range nodes {
		if node == nil {
			continue
		}
		switch node.Tag {
		case "text", "article_link", "external_link":
			r.builder.WriteString(node.Text)
		case "br":
			r.builder.WriteString("\n")
		case "header":
			r.builder.WriteString("\n\n## ")
			r.render(node.Children)
			r.builder.WriteString("\n")
		case "sub_header":
			r.builder.WriteString("\n\n### ")
			r.render(node.Children)
			r.builder.WriteString("\n")
		case "p":
			r.builder.WriteString("\n\n")
			r.render(node.Children)
			r.builder.WriteString("\n")
		case "list_item":
			r.builder.WriteString("\n- ")
			r.render(node.Children)
		case "table_row":
			r.builder.WriteString("\n")
			r.render(node.Children)
		case "table_header", "table_cell":
			r.render(node.Children)
			r.builder.WriteString(" | ")
		default:
			r.render(node.Children)
		}
	}
}

// tidyLines trims each line and collapses runs of blank lines.
func tidyLines(text string) string {
	lines := strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n")
	cleaned := make([]string, 0, len(lines))
	blank := true
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			if blank {
				continue
			}
			blank = true
			cleaned = append(cleaned, "")
			continue
		}
		blank = false
		cleaned = append(cleaned, line)
	}
	return strings.TrimSpace(strings.Join(cleaned, "\n"))
}
