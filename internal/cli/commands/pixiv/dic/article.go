package dic

import (
	"fmt"
	"io"
	"strings"

	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
	"github.com/spf13/cobra"
)

// articleOutput 是 `pixiv dic article` 的机器输出投影。--no-counters 时计数是
// 上游缺失而不是 0，因此用指针加 omitempty 把两者分开。
type articleOutput struct {
	ID          int      `json:"id"`
	Title       string   `json:"title"`
	Yomigana    string   `json:"yomigana"`
	Translation string   `json:"translation"`
	Categories  []string `json:"categories"`
	Abstract    string   `json:"abstract"`
	Related     []string `json:"related"`
	Body        string   `json:"body"`
	URL         string   `json:"url"`
	Views       *int     `json:"views,omitempty"`
	Works       *int     `json:"works,omitempty"`
	Comments    *int     `json:"comments,omitempty"`
	Checklists  *int     `json:"checklists,omitempty"`
}

type articleOptions struct {
	json         bool
	language     string
	skipCounters bool
}

func newArticle(dependencies Dependencies) *cobra.Command {
	options := articleOptions{language: string(dicservice.LanguageJapanese)}
	cmd := &cobra.Command{
		Use:   "article <title|url>",
		Short: "Fetch one Pixiv encyclopedia article",
		Args:  dependencies.exactArgs(1, "pixiv dic article <title|url> [--lang ja|en] [--no-counters] [--json]"),
		RunE: func(cmd *cobra.Command, args []string) error {
			return runArticle(cmd, dependencies, options, args[0])
		},
	}
	flags := cmd.Flags()
	flags.BoolVarP(&options.json, "json", "j", false, "print JSON")
	flags.StringVar(&options.language, "lang", options.language, "article language: ja or en")
	flags.BoolVar(&options.skipCounters, "no-counters", false, "skip the view and work counters")
	return cmd
}

func runArticle(cmd *cobra.Command, dependencies Dependencies, options articleOptions, reference string) error {
	language := dicservice.Language(options.language)
	if language != dicservice.LanguageJapanese && language != dicservice.LanguageEnglish {
		return dependencies.usage(fmt.Errorf("--lang must be one of: %s, %s", dicservice.LanguageJapanese, dicservice.LanguageEnglish))
	}
	reader, err := dependencies.reader()
	if err != nil {
		return err
	}
	article, err := reader.Article(cmd.Context(), dicservice.ArticleRequest{
		Ref:          reference,
		Language:     language,
		SkipCounters: options.skipCounters,
	})
	if err != nil {
		return err
	}
	output := newArticleOutput(article, options.skipCounters)
	jsonOut, err := dependencies.jsonOut(cmd, options.json)
	if err != nil {
		return err
	}
	if jsonOut {
		return dependencies.writeJSON(output)
	}
	return printArticle(dependencies.Output, output)
}

func newArticleOutput(article dicservice.Article, skipCounters bool) articleOutput {
	output := articleOutput{
		ID:          article.ID,
		Title:       article.Title,
		Yomigana:    article.Yomigana,
		Translation: article.Translation,
		Categories:  article.Categories,
		Abstract:    article.Abstract,
		Related:     article.Related,
		Body:        article.Body,
		URL:         article.URL,
	}
	if !skipCounters {
		views, works, comments, checklists := article.Views, article.Works, article.Comments, article.Checklists
		output.Views, output.Works, output.Comments, output.Checklists = &views, &works, &comments, &checklists
	}
	return output
}

func printArticle(out io.Writer, article articleOutput) error {
	heading := article.Title
	if article.Yomigana != "" {
		heading = fmt.Sprintf("%s (%s)", article.Title, article.Yomigana)
	}
	if _, err := fmt.Fprintf(out, "%s\n  %s\n", heading, article.URL); err != nil {
		return err
	}
	if article.Translation != "" {
		if _, err := fmt.Fprintf(out, "  translation: %s\n", article.Translation); err != nil {
			return err
		}
	}
	if len(article.Categories) > 0 {
		if _, err := fmt.Fprintf(out, "  categories: %s\n", strings.Join(article.Categories, ", ")); err != nil {
			return err
		}
	}
	if article.Views != nil {
		if _, err := fmt.Fprintf(out, "  views:%d works:%d comments:%d checklists:%d\n", *article.Views, *article.Works, *article.Comments, *article.Checklists); err != nil {
			return err
		}
	}
	if len(article.Related) > 0 {
		if _, err := fmt.Fprintf(out, "  related: %s\n", strings.Join(article.Related, ", ")); err != nil {
			return err
		}
	}
	for _, section := range []string{article.Abstract, article.Body} {
		if section != "" {
			if _, err := fmt.Fprintf(out, "\n%s\n", section); err != nil {
				return err
			}
		}
	}
	return nil
}
