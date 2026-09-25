// Package ugoira owns the Pixiv ugoira metadata command and terminal presenter.
package ugoira

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"

	requirements "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

// Request 是 ugoira 一次执行解析出的传输覆写值；它不持有 client 或资源。
type Request struct {
	HTTPSProxyOverride *string
}

// Options 是 ugoira 命令自己声明的 flags。
type Options struct {
	Proxy   string
	NoProxy bool
	JSON    bool
}

// Dependencies 是 ugoira owner 的最小执行端口。SDK 资源只经公开 client 和
// composition root 注入的 Pooled 回调进入该 owner。
type Dependencies struct {
	Output              io.Writer
	UsageError          func(error) error
	JSONOut             func(*bool) (bool, error)
	Pooled              func(context.Context, Request, func(context.Context, *pixiv.Client) (bool, error)) error
	FetchArtwork        func(context.Context, *pixiv.Client, int64) (pixiv.Artwork, error)
	FetchUgoiraMetadata func(context.Context, *pixiv.Client, int64) (pixiv.UgoiraMetadata, error)
}

type command struct {
	data Dependencies
}

func (d Dependencies) usage(err error) error {
	if err == nil || d.UsageError == nil {
		return err
	}
	return d.UsageError(err)
}

// New builds the pixiv ugoira command.
func New(data Dependencies) *cobra.Command {
	a := command{data: data}
	opts := Options{}
	cmd := &cobra.Command{
		Use:   "ugoira ID_OR_URL",
		Short: "Show ugoira animation metadata",
		Args: func(_ *cobra.Command, args []string) error {
			if len(args) != 1 {
				return a.data.usage(errors.New("ugoira requires one artwork ID or URL"))
			}
			return nil
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			return a.run(cmd, args, opts)
		},
	}
	cmd.Flags().BoolVarP(&opts.JSON, "json", "j", false, "print JSON")
	cmd.Flags().StringVar(&opts.Proxy, "proxy", "", "proxy URL (http, https, socks5, or socks5h) for this command")
	cmd.Flags().BoolVar(&opts.NoProxy, "no-proxy", false, "clear the configured proxy for this command")
	requirements.Bind(cmd, requirements.PixivData())
	return cmd
}

func (a command) run(cmd *cobra.Command, args []string, opts Options) error {
	request, err := buildRequest(cmd, opts)
	if err != nil {
		return err
	}
	id, err := parseArtworkID(args[0])
	if err != nil {
		return a.data.usage(err)
	}
	jsonOut, err := a.jsonOutput(cmd, opts)
	if err != nil {
		return err
	}
	if a.data.Pooled == nil {
		return errors.New("pixiv ugoira pooled operation is not configured")
	}
	return a.data.Pooled(cmd.Context(), request, func(ctx context.Context, client *pixiv.Client) (bool, error) {
		return false, a.runOne(ctx, client, id, jsonOut)
	})
}

// runOne 先读取作品 kind：非 ugoira 必须在请求元数据前就以 not_ugoira 失败，
// 避免把上游的通用缺字段错误误报成 ugoira 元数据问题。
func (a command) runOne(ctx context.Context, client *pixiv.Client, id int64, jsonOut bool) error {
	if a.data.FetchArtwork == nil || a.data.FetchUgoiraMetadata == nil {
		return errors.New("pixiv ugoira fetchers are not configured")
	}
	artwork, err := a.data.FetchArtwork(ctx, client, id)
	if err != nil {
		return err
	}
	if artwork.Kind != pixiv.ArtworkKindUgoira {
		return sdk.NewError("pixiv", "ugoira", sdk.NotUgoira, sdk.WithDetail("artwork is not a ugoira"))
	}
	metadata, err := a.data.FetchUgoiraMetadata(ctx, client, id)
	if err != nil {
		return err
	}
	metadata.Archives = normalizeArchives(metadata.Archives)
	if jsonOut {
		return a.writeJSON(pixiv.ToUgoiraMetadataDTO(metadata))
	}
	return printUgoira(a.data.Output, metadata)
}

// normalizeArchives 把 original 排在 medium 之前。它只调整顺序，不增删档案；
// SDK 可能按 medium→original 返回，而消费者按顺序读取。
func normalizeArchives(archives []pixiv.UgoiraArchive) []pixiv.UgoiraArchive {
	if len(archives) < 2 {
		return archives
	}
	ordered := make([]pixiv.UgoiraArchive, 0, len(archives))
	for _, archive := range archives {
		if archive.Quality == pixiv.UgoiraQualityOriginal {
			ordered = append(ordered, archive)
		}
	}
	for _, archive := range archives {
		if archive.Quality != pixiv.UgoiraQualityOriginal {
			ordered = append(ordered, archive)
		}
	}
	return ordered
}

func (a command) jsonOutput(cmd *cobra.Command, opts Options) (bool, error) {
	if a.data.JSONOut == nil {
		return false, errors.New("pixiv ugoira JSON output resolver is not configured")
	}
	var override *bool
	if cmd.Flags().Changed("json") {
		override = &opts.JSON
	}
	return a.data.JSONOut(override)
}

func (a command) writeJSON(value any) error {
	body, err := json.Marshal(value)
	if err != nil {
		return err
	}
	var out bytes.Buffer
	if err := json.Indent(&out, body, "", "  "); err != nil {
		return err
	}
	_, err = io.WriteString(a.data.Output, out.String()+"\n")
	return err
}

func buildRequest(cmd *cobra.Command, opts Options) (Request, error) {
	proxyChanged := cmd.Flags().Changed("proxy")
	noProxyChanged := cmd.Flags().Changed("no-proxy")
	if proxyChanged && noProxyChanged {
		return Request{}, errors.New("use either --proxy or --no-proxy, not both")
	}
	request := Request{}
	if noProxyChanged && opts.NoProxy {
		empty := ""
		request.HTTPSProxyOverride = &empty
	} else if proxyChanged {
		request.HTTPSProxyOverride = &opts.Proxy
	}
	return request, nil
}

func parseArtworkID(arg string) (int64, error) {
	value := strings.TrimSpace(arg)
	if id, err := strconv.ParseInt(value, 10, 64); err == nil && id > 0 {
		return id, nil
	}
	ref, err := pixiv.ParseURL(value)
	if err != nil || ref.Kind != pixiv.ReferenceKindArtwork {
		return 0, sdk.NewError("pixiv", "ugoira", sdk.InvalidArgument, sdk.WithDetail("argument must be an artwork ID or a Pixiv artwork URL"))
	}
	return ref.ID, nil
}

func printUgoira(out io.Writer, metadata pixiv.UgoiraMetadata) error {
	if _, err := fmt.Fprintf(out, "artwork: %d\n", metadata.ArtworkID); err != nil {
		return err
	}
	for _, archive := range metadata.Archives {
		if _, err := fmt.Fprintf(out, "archive: %s\n", archive.Quality); err != nil {
			return err
		}
	}
	if _, err := fmt.Fprintf(out, "frames: %d\n", len(metadata.Frames)); err != nil {
		return err
	}
	for _, frame := range metadata.Frames {
		if _, err := fmt.Fprintf(out, "%s %dms\n", frame.Filename, frame.DelayMilliseconds); err != nil {
			return err
		}
	}
	return nil
}
