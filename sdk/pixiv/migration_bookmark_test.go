package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateBookmark = flag.Bool("migration-update-bookmark", false, "capture fixed Go bookmark detail contracts")

func TestMigrationBookmarkDetailsMatchFrozenStateDTOAndReadback(t *testing.T) {
	type row struct {
		Novel    bool              `json:"novel"`
		ID       int64             `json:"id"`
		Status   int               `json:"status"`
		Body     string            `json:"body"`
		Readback bool              `json:"readback"`
		DTOs     []json.RawMessage `json:"dtos"`
		Error    json.RawMessage   `json:"error"`
		Requests []string          `json:"requests"`
	}
	rows := []row{}
	bodies := []string{`{}`, `null`, `{"bookmark_detail":null}`, `{"bookmark_detail":{}}`, `{"bookmark_detail":{"is_bookmarked":false,"restrict":"public","tags":[{"name":"work-tag","is_registered":true}]}}`, `{"bookmark_detail":{"restrict":"private","tags":[{"name":"cat","is_registered":true},{"name":"skip"},{"name":"cat","is_registered":true},{"name":"","is_registered":true},null]}}`, `{"bookmark_detail":{"is_bookmarked":null,"restrict":"friends","tags":null}}`, `{"bookmark_detail":{"is_bookmarked":true,"restrict":null,"tags":[{"name":null,"is_registered":true},{"name":"ignored","is_registered":null}]}}`, `{"bookmark_detail":false}`, `{"bookmark_detail":[]}`, `{"bookmark_detail":{"is_bookmarked":"true"}}`, `{"bookmark_detail":{"restrict":1}}`, `{"bookmark_detail":{"tags":{}}}`, `{"bookmark_detail":{"tags":[1]}}`, `{"bookmark_detail":{"tags":[{"name":1}]}}`, `{"bookmark_detail":{"tags":[{"is_registered":"true"}]}}`, `{"bookmark_detail":{"is_bookmarked":false,"tags":[{"name":1}]}}`, `[]`, `not JSON`}
	for _, novel := range []bool{false, true} {
		for _, body := range bodies {
			rows = append(rows, row{Novel: novel, ID: 42, Status: 200, Body: body})
		}
		for _, status := range []int{204, 400, 401, 403, 404, 410, 429, 500} {
			rows = append(rows, row{Novel: novel, ID: 42, Status: status, Body: "not JSON"})
		}
		rows = append(rows, row{Novel: novel, ID: 0, Status: 200}, row{Novel: novel, ID: -1, Status: 200}, row{Novel: novel, ID: 42, Readback: true, Status: 200})
	}
	for index := range rows {
		row := &rows[index]
		row.DTOs = []json.RawMessage{}
		row.Requests = []string{}
		bookmarked := false
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			request := req.Method + " " + req.URL.RequestURI()
			if req.Method == "POST" {
				if err := req.ParseForm(); err != nil {
					t.Fatal(err)
				}
				request += " " + req.PostForm.Encode()
				bookmarked = strings.HasSuffix(req.URL.Path, "/add")
			}
			row.Requests = append(row.Requests, request)
			body := row.Body
			if row.Readback {
				if req.Method == "POST" {
					body = ""
				} else if bookmarked {
					body = `{"bookmark_detail":{"is_bookmarked":true,"restrict":"private","tags":[{"name":"favorite","is_registered":true}]}}`
				} else {
					body = `{"bookmark_detail":{"is_bookmarked":false,"restrict":"public","tags":[{"name":"work-tag","is_registered":true}]}}`
				}
			}
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		read := func() error {
			var dto any
			if row.Novel {
				detail, readErr := client.NovelBookmark(context.Background(), pixiv.NovelBookmarkRequest{NovelID: row.ID})
				if readErr != nil {
					return readErr
				}
				dto = pixiv.ToNovelBookmarkDetailDTO(detail)
			} else {
				detail, readErr := client.ArtworkBookmark(context.Background(), pixiv.ArtworkBookmarkRequest{ArtworkID: row.ID})
				if readErr != nil {
					return readErr
				}
				dto = pixiv.ToArtworkBookmarkDetailDTO(detail)
			}
			data, marshalErr := json.Marshal(dto)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			row.DTOs = append(row.DTOs, data)
			return nil
		}
		if row.Readback {
			if row.Novel {
				err = client.AddNovelBookmark(context.Background(), pixiv.AddNovelBookmarkRequest{NovelID: row.ID, Restrict: pixiv.RestrictPrivate, Tags: []string{"favorite"}})
			} else {
				err = client.AddArtworkBookmark(context.Background(), pixiv.AddArtworkBookmarkRequest{ArtworkID: row.ID, Restrict: pixiv.RestrictPrivate, Tags: []string{"favorite"}})
			}
			if err != nil {
				t.Fatal(err)
			}
			if err = read(); err != nil {
				t.Fatal(err)
			}
			if row.Novel {
				err = client.RemoveNovelBookmark(context.Background(), pixiv.RemoveNovelBookmarkRequest{NovelID: row.ID})
			} else {
				err = client.RemoveArtworkBookmark(context.Background(), pixiv.RemoveArtworkBookmarkRequest{ArtworkID: row.ID})
			}
			if err != nil {
				t.Fatal(err)
			}
		}
		err = read()
		if err != nil {
			row.Error, err = json.Marshal(err)
			if err != nil {
				t.Fatal(err)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "bookmark-detail.json")
	if *migrationUpdateBookmark {
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
		t.Fatal("bookmark details differ from fixed Go reference")
	}
}
