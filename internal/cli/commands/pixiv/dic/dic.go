// Package dic owns the Pixiv encyclopedia (dic.pixiv.net) command group.
package dic

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"

	requirements "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands"
	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
	"github.com/spf13/cobra"
)

// Reader 是 dic 命令消费的百科读取面。composition root 注入真实 service
// client；测试注入 fake。它只覆盖本命令组真正使用的两个读取。
type Reader interface {
	Search(ctx context.Context, request dicservice.SearchRequest) ([]dicservice.SearchResult, error)
	Article(ctx context.Context, request dicservice.ArticleRequest) (dicservice.Article, error)
}

// Dependencies 是一次 `pixiv dic` 执行所需的窄端口。百科是匿名只读资源，
// 因此这里不出现账号、client 或内容调用链端口。
type Dependencies struct {
	Input      io.Reader
	Output     io.Writer
	UsageError func(error) error
	// JSONOut 返回 JSON 输出开关；nil override 时读取 runtime config。
	JSONOut func(*bool) (bool, error)
	Reader  Reader
}

func (d Dependencies) usage(err error) error {
	if err == nil || d.UsageError == nil {
		return err
	}
	return d.UsageError(err)
}

// exactArgs 返回普通错误：位置参数数量错误按命令失败报告为 exit code 1；
// usage exit code 2 只保留给 unknown option 与显式输入契约违规。
func (d Dependencies) exactArgs(count int, usage string) cobra.PositionalArgs {
	return func(_ *cobra.Command, args []string) error {
		if len(args) != count {
			return fmt.Errorf("usage: %s", usage)
		}
		return nil
	}
}

func (d Dependencies) reader() (Reader, error) {
	if d.Reader == nil {
		return nil, errors.New("dic reader is not configured")
	}
	return d.Reader, nil
}

// jsonOut 只在 --json flag 显式出现时覆盖 runtime config 的 JSON 输出开关。
func (d Dependencies) jsonOut(cmd *cobra.Command, flag bool) (bool, error) {
	var override *bool
	if cmd.Flags().Changed("json") {
		override = &flag
	}
	if d.JSONOut == nil {
		if override == nil {
			return false, nil
		}
		return *override, nil
	}
	return d.JSONOut(override)
}

func (d Dependencies) writeJSON(value any) error {
	body, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	_, err = io.WriteString(d.Output, string(body)+"\n")
	return err
}

func (d Dependencies) writeRecord(value any) error {
	body, err := json.Marshal(value)
	if err != nil {
		return err
	}
	_, err = io.WriteString(d.Output, string(body)+"\n")
	return err
}

// New 构造 `pixiv dic` 命令组。
func New(dependencies Dependencies) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "dic",
		Short: "Read the Pixiv encyclopedia (dic.pixiv.net)",
		Args:  dependencies.exactArgs(0, "pixiv dic <search|article> [options]"),
		RunE: func(cmd *cobra.Command, _ []string) error {
			return cmd.Help()
		},
	}
	cmd.AddCommand(newSearch(dependencies), newArticle(dependencies))
	requirements.Bind(cmd, requirements.Normal())
	return cmd
}
