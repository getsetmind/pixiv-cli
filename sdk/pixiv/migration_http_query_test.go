package pixiv_test

import (
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var migrationUpdateHTTPQuery = flag.Bool("migration-update-http-query", false, "capture Go query escaping and raw URL query preservation")

func TestMigrationHTTPQueryEncoding(t *testing.T) {
	type row struct {
		Name       string      `json:"name"`
		Path       string      `json:"path"`
		Parameters [][2]string `json:"parameters"`
		Target     string      `json:"target"`
	}
	rows := []row{
		{Name: "unicode-and-punctuation", Path: "/fixture", Parameters: [][2]string{{"tag", "空 白+&/=~!*'():;?@#$,[]"}}},
		{Name: "duplicate-encounter-order", Path: "/fixture", Parameters: [][2]string{{"z", "last"}, {"a", "2"}, {"a", "1"}, {"z", "first"}}},
		{Name: "existing-raw-query", Path: "/fixture?raw=%7e%2f+&dup=old&empty=", Parameters: [][2]string{{"dup", "new"}, {"tag", "~*"}}},
		{Name: "existing-empty-query", Path: "/fixture?", Parameters: [][2]string{{"tag", "~*"}}},
		{Name: "empty-parameters", Path: "/fixture", Parameters: [][2]string{}},
		{Name: "empty-parameters-existing-query", Path: "/fixture?raw=%7e%2f+&empty=", Parameters: [][2]string{}},
		{Name: "empty-parameters-empty-query", Path: "/fixture?", Parameters: [][2]string{}},
		{Name: "empty-and-encoded-keys", Path: "/fixture", Parameters: [][2]string{{"", ""}, {"~* 空", ""}, {"", "value"}}},
	}
	for index := range rows {
		current := &rows[index]
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
			current.Target = request.RequestURI
			_, _ = io.WriteString(w, "{}")
		}))
		parsed, err := url.Parse(server.URL + current.Path)
		if err != nil {
			t.Fatal(err)
		}
		var encoded []string
		for _, pair := range current.Parameters {
			encoded = append(encoded, url.QueryEscape(pair[0])+"="+url.QueryEscape(pair[1]))
		}
		if len(encoded) != 0 {
			if parsed.RawQuery != "" {
				parsed.RawQuery += "&"
			}
			parsed.RawQuery += strings.Join(encoded, "&")
		}
		response, err := server.Client().Get(parsed.String())
		if err != nil {
			server.Close()
			t.Fatal(err)
		}
		_, err = io.Copy(io.Discard, response.Body)
		response.Body.Close()
		server.Close()
		if err != nil {
			t.Fatal(err)
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "docs", "migration", "contracts", "http-query.json")
	if *migrationUpdateHTTPQuery {
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
		t.Fatal("HTTP query bytes differ from frozen Go reference")
	}
}
