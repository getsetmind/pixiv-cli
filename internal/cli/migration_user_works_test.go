package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	usercmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/user"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"io"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

var updateCLIUserWorks = flag.Bool("migration-update-cli-user-works", false, "capture user works outputs and logical pages")

func TestMigrationUserWorksPreservesOutputsIdentityWindowsAndTextInput(t *testing.T) {
	type row struct {
		Kind     string            `json:"kind"`
		Args     []string          `json:"args"`
		Input    string            `json:"input"`
		Identity int64             `json:"identity"`
		Mode     string            `json:"mode"`
		Writer   string            `json:"writer"`
		Bodies   []json.RawMessage `json:"bodies"`
		Queries  []url.Values      `json:"queries"`
		Error    string            `json:"error"`
		Stdout   string            `json:"stdout"`
		Stderr   string            `json:"stderr"`
		Exit     int               `json:"exit"`
		Bytes    int               `json:"bytes"`
		Proxy    *string           `json:"proxy"`
		JSON     *bool             `json:"json"`
	}
	var rows []row
	for _, kind := range []string{"artworks", "novels"} {
		key, endpoint := "illusts", "/v1/user/illusts"
		if kind == "novels" {
			key, endpoint = "novels", "/v1/user/novels"
		}
		first := json.RawMessage(fmt.Sprintf(`{"%s":[{"id":101,"create_date":"2024-01-02T03:04:05+09:00","title":"first<&\nline","type":"illust","tags":[{"name":"tag\nline"}],"user":{"id":9,"name":"author\nline"}},{"id":102,"create_date":"2024-01-02T03:04:05+09:00","title":"second","type":"manga","user":{"id":9,"name":"author"}}],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, key, endpoint))
		last := json.RawMessage(fmt.Sprintf(`{"%s":[{"id":102,"create_date":"2024-01-02T03:04:05+09:00","title":"duplicate","type":"manga","user":{"id":9,"name":"author"}},{"id":103,"create_date":"2024-01-02T03:04:05+09:00","title":"last","type":"ugoira","user":{"id":9,"name":"author"}}],"next_url":null}`, key))
		variants := [][]string{{"42"}, {}, {"https://www.pixiv.net/users/42"}, {" 42 "}, {"0"}, {"bad"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--limit=0"}, {"42", "--limit=1"}, {"42", "--limit=2"}, {"42", "--limit=3"}, {"42", "--limit=2", "--page=2"}, {"42", "--limit=1", "--page=5"}, {"42", "--page=2"}, {"42", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--limit=3", "--page=4000000000000000000"}, {"0", "--page=0"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=http://fixture"}, {"42", "--no-proxy"}, {"42", "--ndjson", "--json=false"}}
		if kind == "artworks" {
			for _, value := range []string{"illust", "illustration", "manga", "ugoira", "", "invalid", "ILLUST"} {
				variants = append(variants, []string{"42", "--type=" + value})
			}
			variants = append(variants, []string{"0", "--type=invalid", "--page=0"})
		}
		for scenario := 0; scenario < 7; scenario++ {
			candidates := variants
			bodies := []json.RawMessage{first, last}
			input := ""
			identity := int64(42)
			writer := ""
			if scenario > 0 {
				candidates = [][]string{{"42", "--limit=0"}, {"--limit=0"}}
			}
			switch scenario {
			case 1:
				bodies = []json.RawMessage{json.RawMessage(fmt.Sprintf(`{"%s":[],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, key, endpoint)), last}
			case 2:
				bodies = []json.RawMessage{first, json.RawMessage(fmt.Sprintf(`{"%s":null}`, key))}
			case 3:
				bodies = []json.RawMessage{first, first}
			case 4:
				identity = 0
			case 5:
				input = "https://www.pixiv.net/users/43\r\n"
			case 6:
				writer = "short"
			}
			for _, args := range candidates {
				for _, mode := range []string{"human", "json", "ndjson", "false", "auto"} {
					current := row{Kind: kind, Args: append([]string{}, args...), Input: input, Identity: identity, Mode: mode, Writer: writer, Bodies: bodies, Queries: []url.Values{}}
					if mode == "json" || mode == "ndjson" {
						current.Args = append(current.Args, "--"+mode)
					}
					if mode == "false" {
						current.Args = append(current.Args, "--json=false")
					}
					var output, diagnostics bytes.Buffer
					out := io.Writer(&output)
					if writer != "" {
						out = migrationUserSearchWriter{&output, writer}
					}
					if mode == "auto" {
						out = &migrationUserWorksPipeWriter{Buffer: &output}
					}
					reader := &migrationSearchReader{input: strings.NewReader(input)}
					command := usercmd.New(usercmd.Dependencies{Input: reader, Output: out, UsageError: newUsageError, JSONOut: func(value *bool) (bool, error) { current.JSON = value; return mode == "json", nil }, Pooled: func(ctx context.Context, request usercmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
						current.Proxy = request.HTTPSProxyOverride
						transport := migrationDateTransport(func(req *http.Request) (*http.Response, error) {
							payload := []byte(fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d}}`, identity))
							if req.URL.Host != "oauth.secure.pixiv.net" {
								if req.Method != "GET" || req.URL.Path != endpoint || req.Header.Get("Authorization") != "Bearer fixture-access" {
									t.Fatal("unexpected user works request")
								}
								current.Queries = append(current.Queries, req.URL.Query())
								index := 0
								if len(bodies) > 1 && req.URL.Query().Get("offset") != "" {
									index = 1
								}
								payload = bodies[index]
							}
							return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(payload)), Request: req}, nil
						})
						var client *pixiv.Client
						var err error
						if identity > 0 {
							client, _, err = pixiv.OpenWith(ctx, "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
						} else {
							client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
						}
						if err != nil {
							return err
						}
						_, err = invoke(ctx, client)
						return err
					}})
					root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
					root.AddCommand(command)
					root.SetOut(out)
					root.SetErr(&diagnostics)
					root.SetArgs(append([]string{"user", kind}, current.Args...))
					err := root.Execute()
					if err != nil {
						current.Error = err.Error()
					}
					current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson" || mode == "auto", mode != "human" && mode != "auto")
					current.Stdout, current.Stderr, current.Bytes = output.String(), diagnostics.String(), reader.bytes
					rows = append(rows, current)
				}
			}
		}
	}
	recommendedFixture(t, "cli-user-works.json", rows, *updateCLIUserWorks)
}

type migrationUserWorksPipeWriter struct{ *bytes.Buffer }

func (*migrationUserWorksPipeWriter) Fd() uintptr { return ^uintptr(0) }

func TestMigrationUserWorksInvalidUTF8StdinPreservesIDAndTypeValidation(t *testing.T) {
	for _, kind := range []string{"artworks", "novels"} {
		for _, input := range [][]byte{{0xff}, {'4', '2', 0xff, '\n'}} {
			for _, invalidType := range []bool{false, true} {
				if invalidType && kind == "novels" {
					continue
				}
				var out bytes.Buffer
				command := usercmd.New(usercmd.Dependencies{Input: bytes.NewReader(input), Output: &out, JSONOut: func(*bool) (bool, error) { t.Fatal("invalid stdin reached output resolution"); return false, nil }, Pooled: func(context.Context, usercmd.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
					t.Fatal("invalid stdin reached account pool")
					return nil
				}})
				args := []string{kind}
				want := fmt.Sprintf("user_id: pixiv:user %s: invalid_argument: input must be a positive ID or a supported Pixiv URL", kind)
				if invalidType {
					args = append(args, "--type=invalid")
					want = "pixiv:user artworks: invalid_argument: type must be one of illustration, illust, manga, or ugoira"
				}
				command.SetArgs(args)
				err := command.Execute()
				if err == nil || err.Error() != want {
					t.Fatalf("invalid UTF-8 stdin: got %v, want %s", err, want)
				}
				if out.Len() != 0 {
					t.Fatal("invalid stdin emitted output")
				}
			}
		}
	}
}
