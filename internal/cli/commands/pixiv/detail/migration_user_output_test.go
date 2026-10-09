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

var migrationUpdateUserOutput = flag.Bool("migration-update-user-output", false, "capture fixed Go user detail output")

type migrationUserDetailOutputInput struct {
	Name     string          `json:"name"`
	ID       int64           `json:"id"`
	Body     json.RawMessage `json:"body"`
	WireBody string          `json:"wire_body,omitempty"`
}

func TestMigrationUserDetailOutputMatchesFrozenModes(t *testing.T) {
	var inputs []migrationUserDetailOutputInput
	path := filepath.Join("..", "..", "..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "user-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(data, &inputs); err != nil {
		t.Fatal(err)
	}
	selected := inputs[:0]
	for _, input := range inputs {
		if input.Name == "rich" || input.Name == "section:profile:map[]" || strings.HasPrefix(input.Name, "missing:") {
			selected = append(selected, input)
		}
	}
	inputs = selected
	for _, webpage := range []string{"", "https://user:pass@example.test:8443/a?token=secret#fragment", "http://example.test/a", "https://例え.test/a", "javascript:alert(1)", "https://example.test/%zz", "https://example.test/a?", "//example.test/path", "http://bad host/a", "https://%@example.test/a", "https://example.test/a#%zz", "HTTPS://example.test/a", "https://example.test/a?%zz", "https://example.test", "https://example.test:not/a", "https://example.test/a<>\\{}|^`"} {
		var body map[string]any
		if err := json.Unmarshal(inputs[0].Body, &body); err != nil {
			t.Fatal(err)
		}
		body["profile"].(map[string]any)["webpage"] = webpage
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		inputs = append(inputs, migrationUserDetailOutputInput{Name: webpage, ID: 42, Body: encoded})
	}
	wireData, err := os.ReadFile(filepath.Join(path, "user-wire.json"))
	if err != nil {
		t.Fatal(err)
	}
	var wireInputs []struct {
		Name      string `json:"name"`
		Operation string `json:"operation"`
		Body      string `json:"body"`
	}
	if err := json.Unmarshal(wireData, &wireInputs); err != nil {
		t.Fatal(err)
	}
	selectedWire := map[string]bool{
		"uppercase_known_user_fields":                       true,
		"unicode_long_s_matches_known_fields":               true,
		"ordinary_nested_image_objects_merge":               true,
		"later_image_pointer_null_clears_value":             true,
		"later_scalar_null_preserves_value":                 true,
		"invalid_scalar_then_valid_scalar":                  true,
		"unknown_complex_fields_ignored":                    true,
		"duplicate_user_objects_merge_or_replace":           true,
		"duplicate_profile:empty_object_resets":             true,
		"invalid_then_valid_visibility_recovers":            true,
		"invalid_profile_field_then_new_valid_object_fails": true,
	}
	for _, input := range wireInputs {
		if input.Operation == "User" && selectedWire[input.Name] {
			inputs = append(inputs, migrationUserDetailOutputInput{Name: "wire:" + input.Name, ID: 31, WireBody: input.Body})
		}
	}
	type row struct {
		Name     string          `json:"name"`
		ID       int64           `json:"id"`
		Body     json.RawMessage `json:"body"`
		WireBody string          `json:"wire_body,omitempty"`
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
			row := row{Name: input.Name, ID: input.ID, Body: input.Body, WireBody: input.WireBody, Mode: mode}
			body := input.Body
			if input.WireBody != "" {
				body = []byte(input.WireBody)
			}
			var out bytes.Buffer
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationOutputTransport(func(req *http.Request) (*http.Response, error) {
				if req.Method != "GET" || req.URL.Path != "/v1/user/detail" || req.URL.Query().Get("user_id") != strconv.FormatInt(input.ID, 10) || req.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatal("unexpected user detail CLI request")
				}
				row.Requests++
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
			})}})
			if err != nil {
				t.Fatal(err)
			}
			command := New(Dependencies{Input: strings.NewReader(""), Output: &out, OutputIsTTY: func() bool { return false }, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, BuildRequest: func(*cobra.Command, Options) (Request, error) { return Request{}, nil }, Pooled: func(ctx context.Context, _ Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				_, err := invoke(ctx, client)
				return err
			}, FetchUser: func(ctx context.Context, client *pixiv.Client, id int64) (pixiv.UserDetail, error) {
				return client.User(ctx, pixiv.UserRequest{UserID: id})
			}})
			command.SilenceErrors = true
			command.SilenceUsage = true
			args := []string{"--type=user", strconv.FormatInt(input.ID, 10)}
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
	target := filepath.Join(path, "user-output.json")
	if *migrationUpdateUserOutput {
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
		t.Fatal("user detail output differs from fixed Go reference")
	}
}
