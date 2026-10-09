package sdk_test

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateRefDiagnostics = flag.Bool("migration-update-resource-ref-diagnostics", false, "capture bounded resource reference diagnostics from the fixed Go reference")

func TestMigrationResourceRefDiagnosticsMatchFrozenContract(t *testing.T) {
	type entry struct {
		Name       string        `json:"name"`
		Text       string        `json:"text"`
		Product    string        `json:"product"`
		Operation  string        `json:"operation"`
		Reason     sdk.Reason    `json:"reason"`
		Detail     string        `json:"detail"`
		Cause      string        `json:"cause"`
		Message    string        `json:"message"`
		HTTPStatus int           `json:"http_status"`
		Transport  sdk.Transport `json:"transport"`
		RetrySafe  bool          `json:"retry_safe"`
	}
	var cases []entry
	envelope := func(raw string) string { return base64.RawURLEncoding.EncodeToString([]byte(raw)) }
	for _, input := range []struct{ name, text string }{
		{"direct-invalid-text", "not-a-source?signature=secret"},
		{"direct-ftp", "ftp://i.pximg.net/a/photo.png"},
		{"direct-one-digit", "0"},
		{"direct-negative", "-1"},
		{"outer-invalid-after-crlf", "AAAA\r\n!"},
		{"outer-incomplete-quantum", "AAAAA"},
		{"outer-padded", "YQ=="},
		{"inner-invalid-alphabet", envelope(`{"v":1,"p":"pixiv","d":"!!!"}`)},
		{"inner-incomplete-quantum", envelope(`{"v":1,"p":"pixiv","d":"A"}`)},
		{"inner-invalid-after-crlf", envelope(`{"v":1,"p":"pixiv","d":"AAAA\r\n!"}`)},
		{"json-invalid-leading-letter", envelope("abcd")},
		{"json-empty", "IA"},
		{"future-version", envelope(`{"v":2,"p":"pixiv","d":"AA=="}`)},
		{"missing-product", envelope(`{"v":1,"d":"AA=="}`)},
		{"empty-text", ""},
	} {
		_, err := sdk.ParseResourceRef(input.text)
		var classified *sdk.Error
		if !errors.As(err, &classified) {
			t.Fatalf("%s: expected classified error: %v", input.name, err)
		}
		item := entry{Name: input.name, Text: input.text, Product: classified.Product, Operation: classified.Operation, Reason: classified.Reason, Detail: classified.Detail, Message: err.Error(), HTTPStatus: classified.HTTPStatus, Transport: classified.Transport, RetrySafe: classified.Retry.Safe}
		if cause := errors.Unwrap(classified); cause != nil {
			item.Cause = cause.Error()
		}
		cases = append(cases, item)
	}
	data, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "crates", "pixiv-sdk", "tests", "fixtures", "resource_ref_diagnostics.json")
	if *migrationUpdateRefDiagnostics {
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
		t.Fatal("resource reference diagnostics differ from the frozen Go contract")
	}
}
