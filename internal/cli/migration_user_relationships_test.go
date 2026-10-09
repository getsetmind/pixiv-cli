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

var updateCLIUserRelationships = flag.Bool("migration-update-cli-user-relationships", false, "capture user works outputs and logical pages")

func TestMigrationUserRelationshipsPreservesOutputsIdentityWindowsAndTextInput(t *testing.T) {
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
	for _, kind := range []string{"following", "followers", "related", "blocked"} {
		key, endpoint := "user_previews", "/v1/user/"+kind
		if kind == "followers" {
			endpoint = "/v1/user/follower"
		}
		if kind == "blocked" {
			key, endpoint = "users", "/v2/user/list"
		}
		item := func(id int, name string) string {
			user := fmt.Sprintf(`{"id":%d,"name":%q,"account":"account","comment":"comment"}`, id, name)
			if kind == "blocked" {
				return user
			}
			return `{"user":` + user + `,"illusts":[],"novels":[],"is_muted":false}`
		}
		first := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, key, item(101, "first<&\nline"), item(102, "second"), endpoint))
		last := json.RawMessage(fmt.Sprintf(`{"%s":[%s,%s],"next_url":null}`, key, item(102, "duplicate"), item(103, "last")))
		variants := [][]string{{"42"}, {}, {"https://www.pixiv.net/users/42"}, {" 42 "}, {"0"}, {"bad"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--limit=0"}, {"42", "--limit=1"}, {"42", "--limit=2"}, {"42", "--limit=3"}, {"42", "--limit=2", "--page=2"}, {"42", "--limit=1", "--page=5"}, {"42", "--page=2"}, {"42", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--limit=3", "--page=4000000000000000000"}, {"0", "--page=0"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=http://fixture"}, {"42", "--no-proxy"}, {"42", "--ndjson", "--json=false"}}
		if kind == "following" || kind == "followers" {
			for _, value := range []string{"public", "private", "", "PUBLIC", " public ", "invalid"} {
				variants = append(variants, []string{"42", "--restrict=" + value})
			}
			variants = append(variants, []string{"0", "--restrict=invalid", "--page=0"})
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
					current.Exit = (app{out: out, errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson" || mode == "auto" && kind != "following", mode != "human" && mode != "auto")
					current.Stdout, current.Stderr, current.Bytes = output.String(), diagnostics.String(), reader.bytes
					rows = append(rows, current)
				}
			}
		}
	}
	recommendedFixture(t, "cli-user-relationships.json", rows, *updateCLIUserRelationships)
}

func TestMigrationUserRelationshipsInvalidUTF8StdinPreservesRestrictValidation(t *testing.T) {
	for _, kind := range []string{"following", "followers", "related", "blocked"} {
		for _, input := range [][]byte{{0xff}, {'4', '2', 0xff, '\n'}} {
			for _, invalidRestrict := range []bool{false, true} {
				if invalidRestrict && kind != "following" && kind != "followers" {
					continue
				}
				command := usercmd.New(usercmd.Dependencies{Input: bytes.NewReader(input), Output: io.Discard, JSONOut: func(*bool) (bool, error) { t.Fatal("invalid stdin reached output resolution"); return false, nil }, Pooled: func(context.Context, usercmd.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
					t.Fatal("invalid stdin reached pool")
					return nil
				}})
				args := []string{kind}
				want := fmt.Sprintf("user_id: pixiv:user %s: invalid_argument: input must be a positive ID or a supported Pixiv URL", kind)
				if invalidRestrict {
					args = append(args, "--restrict=invalid")
					want = "pixiv:user relationships: invalid_argument: restrict must be public or private"
				}
				command.SetArgs(args)
				err := command.Execute()
				if err == nil || err.Error() != want {
					t.Fatalf("invalid UTF-8 stdin: got %v want %s", err, want)
				}
			}
		}
	}
}
