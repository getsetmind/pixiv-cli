package sdk_test

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateCursors = flag.Bool("migration-update-cursors", false, "capture cursor contracts from the fixed Go reference")

type migrationCursorCase struct {
	Name           string     `json:"name"`
	Product        string     `json:"product"`
	Operation      string     `json:"operation"`
	Binding        int        `json:"binding"`
	Query          string     `json:"query"`
	Payload        []byte     `json:"payload"`
	Identity       string     `json:"identity"`
	Ephemeral      bool       `json:"ephemeral"`
	Instance       *string    `json:"instance"`
	Text           string     `json:"text"`
	JSON           string     `json:"json"`
	Reason         sdk.Reason `json:"reason"`
	ValidateReason sdk.Reason `json:"validate_reason"`
	InstanceReason sdk.Reason `json:"instance_reason"`
}

func TestMigrationCursorsMatchFrozenEncodingAndBindings(t *testing.T) {
	var contract struct{ Construct, Parse, Validate, Instances, JSON []migrationCursorCase }
	instance := "client-instance-1"
	for _, input := range []migrationCursorCase{
		{Name: "plain", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "digest", Payload: []byte(`{"k":"offset","v":30}`)},
		{Name: "identity", Product: "pixiv", Operation: "UserArtworkBookmarks", Binding: 1, Query: "digest", Payload: []byte("state"), Identity: "42"},
		{Name: "ephemeral", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "digest", Payload: []byte("state"), Ephemeral: true},
		{Name: "instance", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "digest", Payload: []byte("state"), Instance: &instance},
		{Name: "both-bindings", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "digest", Payload: []byte("state"), Identity: "42", Instance: &instance},
		{Name: "binary", Product: "fanbox", Operation: "Posts", Binding: 1, Query: "digest", Payload: []byte{0, 255, 128, 1}},
		{Name: "html-unicode", Product: "<>&\u2028", Operation: "検索", Binding: 2, Query: "<&>", Payload: []byte("state"), Identity: "日本語"},
		{Name: "empty-product", Operation: "Op", Binding: 1, Query: "q", Payload: []byte("p")},
		{Name: "empty-operation", Product: "pixiv", Binding: 1, Query: "q", Payload: []byte("p")},
		{Name: "zero-binding", Product: "pixiv", Operation: "Op", Query: "q", Payload: []byte("p")},
		{Name: "negative-binding", Product: "pixiv", Operation: "Op", Binding: -1, Query: "q", Payload: []byte("p")},
		{Name: "empty-query", Product: "pixiv", Operation: "Op", Binding: 1, Payload: []byte("p")},
		{Name: "empty-payload", Product: "pixiv", Operation: "Op", Binding: 1, Query: "q"},
		{Name: "empty-instance", Product: "pixiv", Operation: "Op", Binding: 1, Query: "q", Payload: []byte("p"), Instance: new(string)},
	} {
		opts := []sdk.CursorOption{}
		if input.Identity != "" {
			opts = append(opts, sdk.WithCursorIdentity(input.Identity))
		}
		if input.Ephemeral {
			opts = append(opts, sdk.WithCursorEphemeral())
		}
		if input.Instance != nil {
			opts = append(opts, sdk.WithCursorEphemeralInstance(*input.Instance))
		}
		cursor, err := sdk.NewCursor(input.Product, input.Operation, input.Binding, input.Query, input.Payload, opts...)
		input.Reason = sdk.ReasonOf(err)
		input.Text = cursor.String()
		if err == nil {
			data, err := json.Marshal(cursor)
			if err != nil {
				t.Fatal(err)
			}
			input.JSON = string(data)
			input.Ephemeral = sdk.CursorEphemeral(cursor)
		}
		contract.Construct = append(contract.Construct, input)
	}
	valid := contract.Construct[0].Text
	addParse := func(name, text string) {
		cursor, err := sdk.ParseCursor(text)
		input := migrationCursorCase{Name: name, Text: text, Reason: sdk.ReasonOf(err)}
		if err == nil {
			input.Payload, err = sdk.CursorPayload(cursor)
			if err != nil {
				t.Fatal(err)
			}
			input.Identity, _ = sdk.CursorIdentity(cursor)
			input.Ephemeral = sdk.CursorEphemeral(cursor)
			data, err := json.Marshal(cursor)
			if err != nil {
				t.Fatal(err)
			}
			input.JSON = string(data)
		}
		contract.Parse = append(contract.Parse, input)
	}
	for _, input := range []struct{ name, text string }{{"valid", valid}, {"empty", ""}, {"raw-url", "https://example.invalid/next"}, {"alphabet", "%%%"}, {"not-json", "YWJjZA"}, {"padded", valid + "="}, {"line-breaks", valid[:4] + "\r\n" + valid[4:]}} {
		addParse(input.name, input.text)
	}
	for _, input := range []struct{ name, raw string }{
		{"minimal", `{"v":1}`}, {"future-version", `{"v":2}`}, {"null-version", `{"v":null}`},
		{"uppercase", `{"V":1,"ID":"42","E":true,"PL":"AA=="}`},
		{"payload-array", `{"v":1,"pl":[0,255,1]}`}, {"duplicate-payload", `{"v":1,"pl":[17],"pl":[null]}`},
		{"null-payload", `{"v":1,"pl":null}`}, {"empty-payload", `{"v":1,"pl":""}`},
		{"negative-binding", `{"v":1,"b":-1}`}, {"null-ephemeral", `{"v":1,"e":true,"e":null}`},
		{"invalid-ephemeral", `{"v":1,"e":1}`}, {"unknown-field", `{"v":1,"extra":{"x":1e999}}`},
		{"fraction-version", `{"v":1.0}`}, {"duplicate-null-version", `{"v":1,"v":null}`},
		{"instance-without-ephemeral", `{"v":1,"i":"client-instance-1"}`},
	} {
		addParse(input.name, base64.RawURLEncoding.EncodeToString([]byte(input.raw)))
	}
	for _, input := range []migrationCursorCase{
		{Name: "match", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "digest"},
		{Name: "product", Product: "fanbox", Operation: "SearchArtworks", Binding: 2, Query: "digest"},
		{Name: "operation", Product: "pixiv", Operation: "SearchNovels", Binding: 2, Query: "digest"},
		{Name: "binding", Product: "pixiv", Operation: "SearchArtworks", Binding: 1, Query: "digest"},
		{Name: "query", Product: "pixiv", Operation: "SearchArtworks", Binding: 2, Query: "other"},
	} {
		cursor, err := sdk.ParseCursor(valid)
		if err != nil {
			t.Fatal(err)
		}
		input.Text = valid
		input.ValidateReason = sdk.ReasonOf(sdk.ValidateCursor(cursor, input.Product, input.Operation, input.Binding, input.Query))
		contract.Validate = append(contract.Validate, input)
	}
	for _, input := range []struct{ name, text, instance string }{
		{"match", contract.Construct[3].Text, instance}, {"different", contract.Construct[3].Text, "other"}, {"empty-instance", contract.Construct[3].Text, ""}, {"not-ephemeral", valid, instance}, {"unbound-ephemeral", contract.Construct[2].Text, instance}, {"zero", "", instance},
	} {
		cursor := sdk.Cursor{}
		if input.text != "" {
			var err error
			cursor, err = sdk.ParseCursor(input.text)
			if err != nil {
				t.Fatal(err)
			}
		}
		instance := input.instance
		contract.Instances = append(contract.Instances, migrationCursorCase{Name: input.name, Text: input.text, Instance: &instance, InstanceReason: sdk.ReasonOf(sdk.ValidateCursorInstance(cursor, instance))})
	}
	for _, input := range []struct{ name, raw string }{{"string", `"` + valid + `"`}, {"null", `null`}, {"number", `42`}, {"object", `{}`}, {"empty", `""`}} {
		var cursor sdk.Cursor
		err := json.Unmarshal([]byte(input.raw), &cursor)
		contract.JSON = append(contract.JSON, migrationCursorCase{Name: input.name, JSON: input.raw, Text: cursor.String(), Reason: sdk.ReasonOf(err)})
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "docs", "migration", "contracts", "cursors.json")
	if *migrationUpdateCursors {
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
		t.Fatal("cursor wire and binding contracts differ from the frozen Go reference")
	}
}
