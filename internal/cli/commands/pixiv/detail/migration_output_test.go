package detail

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

var migrationUpdateDetailOutput = flag.Bool("migration-update-detail-output", false, "capture fixed Go artwork detail output")

type migrationOutputTransport func(*http.Request) (*http.Response, error)

func (transport migrationOutputTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	return transport(request)
}
func TestMigrationArtworkDetailOutputMatchesFrozenModes(t *testing.T) {
	var inputs []struct {
		Name string          `json:"name"`
		ID   int64           `json:"id"`
		Body json.RawMessage `json:"body"`
	}
	path := filepath.Join("..", "..", "..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(data, &inputs); err != nil {
		t.Fatal(err)
	}
	var base map[string]any
	if err := json.Unmarshal(inputs[0].Body, &base); err != nil {
		t.Fatal(err)
	}
	for _, caption := range []string{"", "<p>a\u0080b\u007fc</p>", "<p>A &amp; B</p><div>C<br>D</div><script>hidden()</script><style>hidden</style>", "<ul><li>one</li><li>two</li></ul><blockquote>quote</blockquote>", "<p>\x1b[31mred\x00\t\r</p>", "<table>before<tr><td>cell</td></tr>after</table>", "<p>\u200d\u2028\u00a0\"\\</p>"} {
		var body map[string]any
		if err = json.Unmarshal(inputs[0].Body, &body); err != nil {
			t.Fatal(err)
		}
		body["illust"].(map[string]any)["caption"] = caption
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		inputs = append(inputs, struct {
			Name string          `json:"name"`
			ID   int64           `json:"id"`
			Body json.RawMessage `json:"body"`
		}{Name: caption, ID: 42, Body: encoded})
	}
	type row struct {
		Name     string          `json:"name"`
		ID       int64           `json:"id"`
		Body     json.RawMessage `json:"body"`
		Mode     string          `json:"mode"`
		Output   string          `json:"output"`
		Error    string          `json:"error"`
		Requests int             `json:"requests"`
	}
	rows := []row{}
	for _, input := range inputs {
		if input.ID <= 0 {
			continue
		}
		for _, mode := range []string{"human", "json", "ndjson"} {
			row := row{Name: input.Name, ID: input.ID, Body: input.Body, Mode: mode}
			var out bytes.Buffer
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationOutputTransport(func(req *http.Request) (*http.Response, error) {
				row.Requests++
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(input.Body)), Request: req}, nil
			})}})
			if err != nil {
				t.Fatal(err)
			}
			command := New(Dependencies{Input: strings.NewReader(""), Output: &out, OutputIsTTY: func() bool { return false }, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, BuildRequest: func(*cobra.Command, Options) (Request, error) { return Request{}, nil }, Pooled: func(ctx context.Context, _ Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				_, err := invoke(ctx, client)
				return err
			}, FetchArtwork: func(ctx context.Context, client *pixiv.Client, id int64) (pixiv.Artwork, error) {
				return client.Artwork(ctx, pixiv.ArtworkRequest{ArtworkID: id})
			}})
			command.SilenceErrors = true
			command.SilenceUsage = true
			args := []string{strconv.FormatInt(input.ID, 10)}
			if mode != "human" {
				args = append(args, "--"+mode)
			}
			command.SetArgs(args)
			if err := command.Execute(); err != nil {
				row.Error = err.Error()
			}
			row.Output = out.String()
			rows = append(rows, row)
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "detail-output.json")
	if *migrationUpdateDetailOutput {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("artwork detail output differs from fixed Go reference")
	}
}
