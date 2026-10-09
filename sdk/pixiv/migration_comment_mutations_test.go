package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"io"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"
)

var migrationUpdateCommentMutations = flag.Bool("migration-update-comment-mutations", false, "capture fixed Go comment mutation contracts")

type migrationCommentMutationRow struct {
	Name      string          `json:"name"`
	Operation string          `json:"operation"`
	ID        int64           `json:"id"`
	Comment   string          `json:"comment"`
	ParentID  int64           `json:"parent_id"`
	StampID   int64           `json:"stamp_id"`
	Status    int             `json:"status"`
	Body      string          `json:"body"`
	CommentID int64           `json:"comment_id"`
	Error     json.RawMessage `json:"error"`
	Requests  []string        `json:"requests"`
}

func TestMigrationCommentMutations(t *testing.T) {
	rows := []migrationCommentMutationRow{}
	add := func(op, name, body string, id int64, comment string, parent, stamp int64, status int) {
		rows = append(rows, migrationCommentMutationRow{Name: op + ":" + name, Operation: op, ID: id, Comment: comment, ParentID: parent, StampID: stamp, Status: status, Body: body})
	}
	for _, kind := range []string{"Artwork", "Novel"} {
		for _, verb := range []string{"Post", "Reply", "Stamp", "Delete"} {
			op := verb + kind + "Comment"
			add(op, "wire", `{"comment_id":71,"comment":{"id":72}}`, 42, "日本語 &+\n", 6, 9, 200)
			add(op, "invalid-target", `{"comment_id":71}`, 0, "", 0, 0, 200)
			add(op, "negative-target", `{"comment_id":71}`, -1, "valid", 6, 9, 200)
			if verb == "Post" || verb == "Reply" {
				add(op, "empty-before-parent", `{"comment_id":71}`, 42, "", 0, 9, 200)
				add(op, "whitespace", `{"comment":{"id":73}}`, 42, " \t\n", 6, 9, 201)
			}
			if verb == "Reply" {
				add(op, "invalid-parent", `{"comment_id":71}`, 42, "valid", 0, 9, 200)
			}
			if verb == "Stamp" {
				add(op, "empty-sticker", `{"comment":{"id":74}}`, 42, "", 6, 9, 200)
				add(op, "invalid-stamp", `{"comment_id":71}`, 42, "", 6, 0, 200)
			}
			if verb == "Delete" {
				for _, body := range []string{"", "not JSON", `{"error":true}`} {
					add(op, "ignored:"+body, body, 42, "", 0, 0, 204)
				}
			}
		}
	}
	bodies := []string{
		`{"comment_id":75}`, `{"comment":{"id":76}}`, `{"comment_id":null,"comment":{"id":77}}`,
		`{"comment_id":0,"comment":{"id":78}}`, `{"comment_id":-1,"comment":{"id":78}}`,
		`{}`, `null`, `[]`, "", "not JSON", `{"comment_id":79}{}`,
		`{"comment_id":"79","comment":{"id":80}}`, `{"comment_id":79,"comment":{"id":"80"}}`,
		`{"comment_id":79,"comment":false}`, `{"comment_id":79,"unknown":1e999}`,
		`{"comment_id":9223372036854775807}`, `{"comment_id":9223372036854775808}`,
		`{"comment_id":79.0}`, `{"comment_id":79e0}`,
		`{"comment_id":79,"COMMENT_ID":80}`, `{"COMMENT_ID":79,"comment_id":null,"comment":{"id":80}}`,
		`{"comment":{"id":81},"COMMENT":{"unknown":0}}`, `{"comment":{"id":81},"comment":null}`,
		`{"comment":{"id":81},"comment":{"id":null}}`,
		`{"comment":{"id":81},"comment":{"id":82}}`,
		`{"comment_id":"bad","comment_id":83}`,
	}
	for _, kind := range []string{"Artwork", "Novel"} {
		for _, body := range bodies {
			add("Post"+kind+"Comment", "decode:"+body, body, 42, "body", 0, 0, 200)
		}
		for _, verb := range []string{"Post", "Delete"} {
			for _, status := range []int{299, 300, 400, 401, 403, 404, 410, 429, 500, 503} {
				add(verb+kind+"Comment", "status:"+http.StatusText(status), `{"comment_id":84}`, 42, "body", 0, 0, status)
			}
		}
	}
	for index := range rows {
		row := &rows[index]
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "POST" || req.URL.Host != "app-api.pixiv.net" || req.URL.RawQuery != "" || req.Header.Get("Authorization") != "Bearer fixture-access" || req.Header.Get("Content-Type") != "application/x-www-form-urlencoded" {
				t.Fatal("unexpected comment mutation request")
			}
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			row.Requests = append(row.Requests, req.Method+" "+req.URL.Path+" "+req.PostForm.Encode())
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"text/plain"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		var result pixiv.CommentMutationResult
		switch row.Operation {
		case "PostArtworkComment":
			result, err = client.PostArtworkComment(context.Background(), pixiv.PostArtworkCommentRequest{ArtworkID: row.ID, Comment: row.Comment})
		case "ReplyArtworkComment":
			result, err = client.ReplyArtworkComment(context.Background(), pixiv.ReplyArtworkCommentRequest{ArtworkID: row.ID, Comment: row.Comment, ParentCommentID: row.ParentID})
		case "StampArtworkComment":
			result, err = client.StampArtworkComment(context.Background(), pixiv.StampArtworkCommentRequest{ArtworkID: row.ID, Comment: row.Comment, StampID: row.StampID})
		case "DeleteArtworkComment":
			err = client.DeleteArtworkComment(context.Background(), pixiv.DeleteArtworkCommentRequest{CommentID: row.ID})
		case "PostNovelComment":
			result, err = client.PostNovelComment(context.Background(), pixiv.PostNovelCommentRequest{NovelID: row.ID, Comment: row.Comment})
		case "ReplyNovelComment":
			result, err = client.ReplyNovelComment(context.Background(), pixiv.ReplyNovelCommentRequest{NovelID: row.ID, Comment: row.Comment, ParentCommentID: row.ParentID})
		case "StampNovelComment":
			result, err = client.StampNovelComment(context.Background(), pixiv.StampNovelCommentRequest{NovelID: row.ID, Comment: row.Comment, StampID: row.StampID})
		case "DeleteNovelComment":
			err = client.DeleteNovelComment(context.Background(), pixiv.DeleteNovelCommentRequest{CommentID: row.ID})
		}
		row.CommentID = result.CommentID
		if err != nil {
			data, marshalErr := json.Marshal(err)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			row.Error = data
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := "../../docs/migration/contracts/comment-mutations.json"
	if *migrationUpdateCommentMutations {
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
		t.Fatal("comment mutations differ from fixed Go reference")
	}
}

func TestMigrationCommentMutationHeadersAndRetry(t *testing.T) {
	for _, verified := range []bool{false, true} {
		for _, language := range []string{"", "  ", "  ja-JP  "} {
			for _, deletion := range []bool{false, true} {
				calls := 0
				rt := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
					if req.URL.Host == "oauth.secure.pixiv.net" {
						return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600,"user":{"id":"31","name":"writer"}}`)), Request: req}, nil
					}
					calls++
					wantID := ""
					if verified {
						wantID = "31"
					}
					if req.Header.Get("X-User-Id") != wantID || req.Header.Get("Accept-Language") != strings.TrimSpace(language) {
						t.Fatalf("headers = %v", req.Header)
					}
					if req.URL.Path == "/v1/stamps" {
						calls--
						if req.Method != "GET" {
							t.Fatal("stamps must use GET")
						}
						return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"stamps":[]}`)), Request: req}, nil
					}
					return &http.Response{StatusCode: 429, Header: http.Header{"Retry-After": {"60"}}, Body: io.NopCloser(strings.NewReader("private sentinel")), Request: req}, nil
				})
				options := pixiv.Options{HTTPClient: &http.Client{Transport: rt}, AcceptLanguage: language}
				var client *pixiv.Client
				var err error
				if verified {
					client, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", options)
				} else {
					client, err = pixiv.NewWith("fixture-access", options)
				}
				if err != nil {
					t.Fatal(err)
				}
				if _, err := client.Stamps(context.Background(), pixiv.StampsRequest{}); err != nil {
					t.Fatal(err)
				}
				before := time.Now()
				if deletion {
					err = client.DeleteArtworkComment(context.Background(), pixiv.DeleteArtworkCommentRequest{CommentID: 42})
				} else {
					_, err = client.PostArtworkComment(context.Background(), pixiv.PostArtworkCommentRequest{ArtworkID: 42, Comment: "body"})
				}
				var classified *sdk.Error
				if !errors.As(err, &classified) || classified.Reason != sdk.RateLimited || classified.HTTPStatus != 429 || calls != 1 {
					t.Fatalf("error=%v calls=%d", err, calls)
				}
				if deletion {
					if classified.Retry.Safe || classified.Retry.HasAfter {
						t.Fatal("delete must discard retry header")
					}
				} else {
					if !classified.Retry.Safe || !classified.Retry.HasAfter || classified.Retry.After.Before(before.Add(60*time.Second)) || classified.Retry.After.After(time.Now().Add(60*time.Second)) {
						t.Fatalf("retry=%v", classified.Retry)
					}
				}
			}
		}
	}
}
