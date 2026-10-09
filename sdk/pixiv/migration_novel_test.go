package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateNovel = flag.Bool("migration-update-novel", false, "capture artwork detail contracts from the fixed Go reference")

func TestMigrationNovelMatchesFrozenDTOAndRequests(t *testing.T) {
	type request struct {
		Method  string            `json:"method"`
		Host    string            `json:"host"`
		Path    string            `json:"path"`
		Query   url.Values        `json:"query"`
		Headers map[string]string `json:"headers"`
	}
	type row struct {
		Name     string          `json:"name"`
		ID       int64           `json:"id"`
		Body     json.RawMessage `json:"body"`
		DTO      json.RawMessage `json:"dto"`
		Reason   sdk.Reason      `json:"reason"`
		Message  string          `json:"message"`
		Requests []request       `json:"requests"`
	}
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "novel-ranking.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Name   string            `json:"name"`
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	type input struct {
		name string
		id   int64
		body map[string]any
	}
	inputs := []input{}
	for index, source := range sources {
		if index != 0 && !strings.HasPrefix(source.Name, "field:") && !strings.HasPrefix(source.Name, "body:") {
			continue
		}
		var body map[string]any
		if err := json.Unmarshal(source.Bodies[0], &body); err != nil {
			t.Fatal(err)
		}
		var novel any
		if novels, ok := body["novels"].([]any); ok && len(novels) > 0 {
			novel = novels[0]
		}
		inputs = append(inputs, input{source.Name, 42, map[string]any{"novel": novel}})
	}
	for _, key := range []string{"series_next", "series_prev"} {
		for _, value := range []any{nil, map[string]any{"id": 5, "title": "next"}, map[string]any{"id": 0}, map[string]any{"id": -1}, map[string]any{"id": "5"}, map[string]any{"id": 5, "title": 7}} {
			inputs = append(inputs, input{fmt.Sprintf("%s:%v", key, value), 42, map[string]any{"novel": inputs[0].body["novel"], key: value}})
		}
	}
	inputs = append(inputs, input{"zero-request", 0, inputs[0].body}, input{"negative-request", -1, inputs[0].body})
	rows := make([]row, 0, len(inputs))
	for _, input := range inputs {
		body, err := json.Marshal(input.body)
		if err != nil {
			t.Fatal(err)
		}
		item := row{Name: input.name, ID: input.id, Body: body, Requests: []request{}}
		transport := migrationArtworkTransport(func(r *http.Request) (*http.Response, error) {
			headers := map[string]string{}
			for _, key := range []string{"User-Agent", "App-OS", "App-OS-Version", "App-Version", "Referer", "Authorization"} {
				headers[key] = r.Header.Get(key)
			}
			item.Requests = append(item.Requests, request{r.Method, r.URL.Host, r.URL.Path, r.URL.Query(), headers})
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: r}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.Novel(context.Background(), pixiv.NovelRequest{NovelID: input.id})
		item.Reason = sdk.ReasonOf(err)
		if err != nil {
			item.Message = err.Error()
		} else {
			item.DTO, err = json.Marshal(pixiv.ToNovelDTO(result))
			if err != nil {
				t.Fatal(err)
			}
		}
		rows = append(rows, item)
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "novel-detail.json")
	if *migrationUpdateNovel {
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
		t.Fatal("novel DTO and requests differ from the fixed Go reference")
	}
}
