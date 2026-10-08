package cli

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	pixivdeps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/utils/uri"
)

var updateConnectionOptions = flag.Bool("migration-update-connection-options", false, "capture command connection selection contracts")

type migrationConnectionInput struct {
	Name     string  `json:"name"`
	Global   string  `json:"global"`
	Service  *string `json:"service"`
	Override *string `json:"override"`
	Interval int64   `json:"interval"`
}

type migrationConnectionRow struct {
	Input        migrationConnectionInput `json:"input"`
	Message      string                   `json:"message"`
	InvalidProxy bool                     `json:"invalid_proxy"`
	Proxy        string                   `json:"proxy"`
	Pacing       int64                    `json:"pacing"`
	Timeout      int64                    `json:"timeout"`
	HTTPClient   bool                     `json:"http_client"`
}

func TestMigrationConnectionOptionsPreserveProxyPresenceAndPacing(t *testing.T) {
	text := func(value string) *string { return &value }
	inputs := []migrationConnectionInput{
		{Name: "direct"},
		{Name: "global", Global: "http://global.invalid:80"},
		{Name: "service", Global: "http://global.invalid", Service: text("http://service.invalid")},
		{Name: "service_direct", Global: "http://global.invalid", Service: text("")},
		{Name: "override", Global: "http://global.invalid", Service: text("http://service.invalid"), Override: text("http://override.invalid")},
		{Name: "override_direct", Global: "http://global.invalid", Service: text("http://service.invalid"), Override: text("")},
		{Name: "override_ignores_invalid_lower_sources", Global: "%zz", Service: text("%zz"), Override: text("")},
		{Name: "service_ignores_invalid_global", Global: "%zz", Service: text("http://service.invalid")},
		{Name: "pacing", Interval: int64(2 * time.Second)},
		{Name: "pacing_and_proxy", Global: "socks5h://proxy.invalid:1080", Interval: 123456789},
	}
	for _, value := range []string{"http://proxy.invalid", "https://proxy.invalid:8443", "socks5://proxy.invalid:1080", "socks5h://proxy.invalid:1080", "http://[::1]:8080", "http://synthetic-user:synthetic-password@proxy.invalid:7890", "http://proxy.invalid/path?key=synthetic", "http://proxy.invalid/%zz", "ftp://proxy.invalid", "proxy.invalid:8080", "//proxy.invalid", "http:///path", "http://", " http://proxy.invalid", "http://proxy.invalid:bad"} {
		inputs = append(inputs, migrationConnectionInput{Name: "proxy_" + value, Global: value})
	}
	var rows []migrationConnectionRow
	for _, input := range inputs {
		row := migrationConnectionRow{Input: input}
		options, err := pixivOptionsFromRequest(pixivdeps.Request{HTTPSProxyOverride: input.Override}, func() (config.RuntimeConfig, error) {
			cfg := config.RuntimeConfig{HTTPSProxy: input.Global, RequestInterval: time.Duration(input.Interval)}
			if input.Service != nil {
				cfg.PixivNetwork.ProxyURL = config.OptionalString{Present: true, Value: *input.Service}
			}
			return cfg, nil
		})
		if err != nil {
			row.Message = err.Error()
			row.InvalidProxy = errors.Is(err, uri.ErrInvalidProxy)
		} else {
			row.Pacing = int64(options.Pacing.MinInterval)
			row.HTTPClient = options.HTTPClient != nil
			if options.HTTPClient == nil {
				t.Fatal("command did not construct an HTTP client")
			}
			row.Timeout = int64(options.HTTPClient.Timeout)
			transport, ok := options.HTTPClient.Transport.(*http.Transport)
			if !ok {
				t.Fatalf("unexpected transport %T", options.HTTPClient.Transport)
			}
			if transport.Proxy != nil {
				request, err := http.NewRequest(http.MethodGet, "https://app-api.pixiv.net/v1/illust/detail", nil)
				if err != nil {
					t.Fatal(err)
				}
				proxy, err := transport.Proxy(request)
				if err != nil {
					t.Fatal(err)
				}
				if proxy != nil {
					row.Proxy = proxy.String()
				}
			}
			transport.CloseIdleConnections()
		}
		rows = append(rows, row)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "connection-options.json")
	if *updateConnectionOptions {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("command connection options differ from fixed Go reference")
	}
}
