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

var migrationUpdateMutation = flag.Bool("migration-update-mutation", false, "capture fixed Go form mutation contracts")

func TestMigrationFormMutationsMatchFrozenValidationWireAndStatus(t *testing.T) {
	type row struct {
		Operation string          `json:"operation"`
		ID        int64           `json:"id"`
		Restrict  string          `json:"restrict"`
		Tags      []string        `json:"tags"`
		Status    int             `json:"status"`
		Body      string          `json:"body"`
		Error     json.RawMessage `json:"error"`
		Requests  []string        `json:"requests"`
	}
	rows := []row{}
	for _, operation := range []string{"AddArtworkBookmark", "AddBookmark", "RemoveArtworkBookmark", "RemoveBookmark", "AddNovelBookmark", "RemoveNovelBookmark", "FollowUser", "UnfollowUser"} {
		for _, restrict := range []string{"", "public", "private", "friends", "PUBLIC", " public "} {
			rows = append(rows, row{Operation: operation, ID: 42, Restrict: restrict, Tags: []string{"tag", "", "日本語", "tag", "a&b +", "line\nfeed"}, Status: 200, Body: "not JSON"})
		}
		for _, id := range []int64{0, -1, 9223372036854775807} {
			rows = append(rows, row{Operation: operation, ID: id, Restrict: "friends", Status: 204})
		}
		for _, status := range []int{200, 201, 204, 299, 300, 400, 401, 403, 404, 410, 429, 500, 503} {
			rows = append(rows, row{Operation: operation, ID: 42, Status: status, Body: "fixture private body"})
		}
	}
	rows = append(rows, row{Operation: "SetAIArtworkVisibility", ID: 0, Status: 200}, row{Operation: "SetAIArtworkVisibility", ID: 1, Status: 200})
	for index := range rows {
		row := &rows[index]
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "POST" || req.URL.Host != "app-api.pixiv.net" || req.URL.RawQuery != "" || req.Header.Get("Authorization") != "Bearer fixture-access" || req.Header.Get("Content-Type") != "application/x-www-form-urlencoded" {
				t.Fatal("unexpected mutation request")
			}
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			row.Requests = append(row.Requests, req.Method+" "+req.URL.Path+" "+req.PostForm.Encode())
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"text/plain"}, "Retry-After": {"0"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		restrict := pixiv.Restrict(row.Restrict)
		switch row.Operation {
		case "AddArtworkBookmark":
			err = client.AddArtworkBookmark(context.Background(), pixiv.AddArtworkBookmarkRequest{ArtworkID: row.ID, Restrict: restrict, Tags: row.Tags})
		case "AddBookmark":
			err = client.AddBookmark(context.Background(), pixiv.AddBookmarkRequest{ArtworkID: row.ID, Restrict: restrict, Tags: row.Tags})
		case "RemoveArtworkBookmark":
			err = client.RemoveArtworkBookmark(context.Background(), pixiv.RemoveArtworkBookmarkRequest{ArtworkID: row.ID})
		case "RemoveBookmark":
			err = client.RemoveBookmark(context.Background(), pixiv.RemoveBookmarkRequest{ArtworkID: row.ID})
		case "AddNovelBookmark":
			err = client.AddNovelBookmark(context.Background(), pixiv.AddNovelBookmarkRequest{NovelID: row.ID, Restrict: restrict, Tags: row.Tags})
		case "RemoveNovelBookmark":
			err = client.RemoveNovelBookmark(context.Background(), pixiv.RemoveNovelBookmarkRequest{NovelID: row.ID})
		case "FollowUser":
			err = client.FollowUser(context.Background(), pixiv.FollowUserRequest{UserID: row.ID, Restrict: restrict})
		case "UnfollowUser":
			err = client.UnfollowUser(context.Background(), pixiv.UnfollowUserRequest{UserID: row.ID})
		case "SetAIArtworkVisibility":
			err = client.SetAIArtworkVisibility(context.Background(), pixiv.SetAIArtworkVisibilityRequest{Visible: row.ID != 0})
		}
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "form-mutations.json")
	if *migrationUpdateMutation {
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
		t.Fatal("mutations differ from fixed Go reference")
	}
}
