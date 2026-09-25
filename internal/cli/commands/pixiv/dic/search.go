package dic

import (
	"errors"
	"fmt"
	"io"
	"strings"

	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
	"github.com/spf13/cobra"
)

// searchOutput 是 `pixiv dic search` 的机器输出投影。百科搜索页没有公开的
// 分页游标，一条结果只携带搜索卡片上的字段。
type searchOutput struct {
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

type searchOptions struct {
	json   bool
	ndjson bool
	page   int
	limit  int
}

func newSearch(dependencies Dependencies) *cobra.Command {
	options := searchOptions{page: 1}
	cmd := &cobra.Command{
		Use:   "search <query>",
		Short: "Search Pixiv encyclopedia articles",
		Args:  dependencies.exactArgs(1, "pixiv dic search <query> [--page N] [--limit N] [--json] [--ndjson]"),
		RunE: func(cmd *cobra.Command, args []string) error {
			return runSearch(cmd, dependencies, options, args[0])
		},
	}
	flags := cmd.Flags()
	flags.BoolVarP(&options.json, "json", "j", false, "print JSON")
	flags.BoolVar(&options.ndjson, "ndjson", false, "print one encyclopedia article as JSON per line")
	flags.IntVar(&options.page, "page", options.page, "1-based result page")
	flags.IntVarP(&options.limit, "limit", "n", 0, "maximum results; 0 returns every article on the page")
	return cmd
}

func runSearch(cmd *cobra.Command, dependencies Dependencies, options searchOptions, query string) error {
	if options.page < 1 {
		return dependencies.usage(errors.New("--page must be greater than 0"))
	}
	if options.limit < 0 {
		return dependencies.usage(errors.New("--limit must not be negative"))
	}
	if options.ndjson && cmd.Flags().Changed("json") {
		return dependencies.usage(errors.New("--ndjson cannot be used with --json"))
	}
	reader, err := dependencies.reader()
	if err != nil {
		return err
	}
	results, err := reader.Search(cmd.Context(), dicservice.SearchRequest{Query: query, Page: options.page})
	if err != nil {
		return err
	}
	if options.limit > 0 && len(results) > options.limit {
		results = results[:options.limit]
	}
	records := make([]searchOutput, 0, len(results))
	for _, result := range results {
		records = append(records, searchOutput{
			Title:      result.Title,
			Summary:    result.Summary,
			Updated:    result.Updated,
			Views:      result.Views,
			Works:      result.Works,
			Checklists: result.Checklists,
			Related:    result.Related,
			URL:        result.URL,
			Thumbnail:  result.Thumbnail,
		})
	}
	if options.ndjson {
		for _, record := range records {
			if err := dependencies.writeRecord(record); err != nil {
				return err
			}
		}
		return nil
	}
	jsonOut, err := dependencies.jsonOut(cmd, options.json)
	if err != nil {
		return err
	}
	if jsonOut {
		return dependencies.writeJSON(records)
	}
	return printSearchResults(dependencies.Output, records)
}

func printSearchResults(out io.Writer, records []searchOutput) error {
	if len(records) == 0 {
		_, err := io.WriteString(out, "no encyclopedia articles matched\n")
		return err
	}
	for _, record := range records {
		if _, err := fmt.Fprintf(out, "%s\n%s\n", record.URL, record.Title); err != nil {
			return err
		}
		if record.Summary != "" {
			if _, err := fmt.Fprintf(out, "  %s\n", record.Summary); err != nil {
				return err
			}
		}
		if _, err := fmt.Fprintf(out, "  updated:%s views:%d works:%d checklists:%d\n", record.Updated, record.Views, record.Works, record.Checklists); err != nil {
			return err
		}
		if len(record.Related) > 0 {
			if _, err := fmt.Fprintf(out, "  related: %s\n", strings.Join(record.Related, ", ")); err != nil {
				return err
			}
		}
	}
	return nil
}
