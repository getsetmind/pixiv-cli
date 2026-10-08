package pixiv_test

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateSearch = flag.Bool("migration-update-search", false, "capture fixed Go search pages and checkpoints")

type migrationSearchInput struct {
	Word          string `json:"word"`
	Target        string `json:"target"`
	Sort          string `json:"sort"`
	Duration      string `json:"duration"`
	StartDate     string `json:"start_date"`
	EndDate       string `json:"end_date"`
	ContentType   string `json:"content_type"`
	AIMode        string `json:"ai_mode"`
	AspectRatio   string `json:"aspect_ratio"`
	Resolution    string `json:"resolution"`
	Tool          string `json:"tool"`
	BookmarkMin   *int   `json:"bookmark_min"`
	BookmarkMax   *int   `json:"bookmark_max"`
	CursorContext string `json:"cursor_context"`
}

func (in migrationSearchInput) request() pixiv.SearchArtworksRequest {
	return pixiv.SearchArtworksRequest{Word: in.Word, Target: pixiv.SearchTarget(in.Target), Sort: pixiv.SortMode(in.Sort), Duration: pixiv.DurationFilter(in.Duration), StartDate: in.StartDate, EndDate: in.EndDate, ContentType: pixiv.SearchContentType(in.ContentType), AIMode: pixiv.SearchAIMode(in.AIMode), AspectRatio: pixiv.SearchAspectRatio(in.AspectRatio), Resolution: pixiv.SearchResolution(in.Resolution), Tool: in.Tool, BookmarkMin: in.BookmarkMin, BookmarkMax: in.BookmarkMax, CursorContext: in.CursorContext}
}

type migrationSearchStep struct {
	Action   string `json:"action"`
	Consumed int    `json:"consumed"`
	Mode     string `json:"mode"`
	Word     string `json:"word"`
	Context  string `json:"context"`
}
type migrationSearchResult struct {
	Items   []pixiv.ArtworkDTO `json:"items"`
	Cursor  any                `json:"cursor"`
	Reason  sdk.Reason         `json:"reason"`
	Message string             `json:"message"`
}
type migrationSearchRow struct {
	Name    string                  `json:"name"`
	Input   migrationSearchInput    `json:"input"`
	UserID  int64                   `json:"user_id"`
	Bodies  []json.RawMessage       `json:"bodies"`
	Steps   []migrationSearchStep   `json:"steps"`
	Results []migrationSearchResult `json:"results"`
	Queries []url.Values            `json:"queries"`
}

func migrationSearchCursor(t *testing.T, cursor sdk.Cursor) any {
	t.Helper()
	if cursor.IsZero() {
		return nil
	}
	raw, err := base64.RawURLEncoding.DecodeString(cursor.String())
	if err != nil {
		t.Fatal(err)
	}
	var value map[string]any
	if err = json.Unmarshal(raw, &value); err != nil {
		t.Fatal(err)
	}
	if _, ok := value["i"]; ok {
		value["i"] = "instance"
	}
	return value
}
func TestMigrationSearchArtworksMatchesFrozenFiltersPagesAndCheckpoints(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var artwork []struct {
		Name string          `json:"name"`
		ID   int64           `json:"id"`
		Body json.RawMessage `json:"body"`
	}
	if err = json.Unmarshal(data, &artwork); err != nil {
		t.Fatal(err)
	}
	var complete map[string]json.RawMessage
	if err = json.Unmarshal(artwork[0].Body, &complete); err != nil {
		t.Fatal(err)
	}
	body := json.RawMessage(`{"illusts":[` + string(complete["illust"]) + `],"next_url":"https://app-api.pixiv.net/v1/search/illust?offset=30"}`)
	rows := []migrationSearchRow{}
	add := func(name string, input migrationSearchInput) {
		rows = append(rows, migrationSearchRow{Name: name, Input: input, Bodies: []json.RawMessage{body}, Steps: []migrationSearchStep{{Action: "search"}}})
	}
	add("default", migrationSearchInput{Word: "  日本語 & tag  "})
	for _, field := range []string{"word", "target", "sort", "duration", "content_type", "ai_mode", "aspect_ratio", "resolution", "start_date", "end_date", "tool", "cursor_context"} {
		values := map[string][]string{"word": {"", " \t"}, "target": {"partial_match_for_tags", "exact_match_for_tags", "title_and_caption", "keyword", "bad"}, "sort": {"date_desc", "date_asc", "popular_desc", "bad"}, "duration": {"within_last_day", "within_last_week", "within_last_month", "bad"}, "content_type": {"all", "illust-and-ugoira", "illust", "manga", "ugoira", "bad"}, "ai_mode": {"all", "exclude", "only", "bad"}, "aspect_ratio": {"all", "landscape", "portrait", "square", "bad"}, "resolution": {"all", "high", "medium", "low", "bad"}, "start_date": {"2024-02-29", "2025-02-29", "0000-01-01", "2026-1-01"}, "end_date": {"2026-12-31", "bad"}, "tool": {"unlisted exact tool"}, "cursor_context": {"private filter context"}}[field]
		for _, value := range values {
			in := migrationSearchInput{Word: "fixture"}
			encoded, _ := json.Marshal(in)
			var m map[string]any
			_ = json.Unmarshal(encoded, &m)
			m[field] = value
			encoded, _ = json.Marshal(m)
			_ = json.Unmarshal(encoded, &in)
			add(field+":"+value, in)
		}
	}
	for _, rangeValues := range [][2]int{{0, 0}, {1, 20}, {-1, 20}, {1, -1}, {20, 1}, {9223372036854775807, 9223372036854775807}} {
		min, max := rangeValues[0], rangeValues[1]
		add(fmt.Sprintf("bookmarks:%d:%d", min, max), migrationSearchInput{Word: "fixture", BookmarkMin: &min, BookmarkMax: &max})
	}
	add("reversed-dates", migrationSearchInput{Word: "fixture", StartDate: "2026-01-02", EndDate: "2026-01-01"})
	add("all-filters", migrationSearchInput{Word: "fixture", Target: "keyword", Sort: "popular_desc", Duration: "within_last_week", StartDate: "2026-01-01", EndDate: "2026-01-02", ContentType: "illust-and-ugoira", AIMode: "only", AspectRatio: "square", Resolution: "medium", Tool: "CLIP STUDIO PAINT", CursorContext: "private"})
	for _, in := range artwork {
		if in.ID <= 0 {
			continue
		}
		var envelope map[string]json.RawMessage
		if err = json.Unmarshal(in.Body, &envelope); err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationSearchRow{Name: "response:" + in.Name, Input: migrationSearchInput{Word: "fixture"}, Bodies: []json.RawMessage{json.RawMessage(`{"illusts":[` + string(envelope["illust"]) + `]}`)}, Steps: []migrationSearchStep{{Action: "search"}}})
	}
	for _, raw := range []string{`{}`, `{"illusts":null}`, `{"illusts":[]}`, `{"illusts":[null]}`, `{"illusts":{},"next_url":null}`, `{"illusts":[],"next_url":7}`} {
		rows = append(rows, migrationSearchRow{Name: "envelope:" + raw, Input: migrationSearchInput{Word: "fixture"}, Bodies: []json.RawMessage{json.RawMessage(raw)}, Steps: []migrationSearchStep{{Action: "search"}}})
	}
	for _, next := range []string{"https://app-api.pixiv.net/v1/search/illust?offset=30&word=other", "HTTPS://app-api.pixiv.net/v1/search/illust?offset=+0030", "https://app-api.pixiv.net:/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=30#", "", "http://app-api.pixiv.net/v1/search/illust?offset=30", "https://evil.invalid/v1/search/illust?offset=30", "https://app-api.pixiv.net:443/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=0", "https://app-api.pixiv.net/v1/search/illust?offset=-1", "https://app-api.pixiv.net/v1/search/illust?offset=30&offset=60", "https://app-api.pixiv.net/v1/search/illust?offset=30&unknown=x", "https://app-api.pixiv.net/v1/search/illust?offset=30&word=a&word=b", "https://app-api.pixiv.net/v1/search/illust?offset=30&word=", "https://app-api.pixiv.net/v1/search/illust?offset=30;word=x", "https://app-api.pixiv.net/v1/search/illust?offset=30&word=%zz", "https://app-api.pixiv.net/v1/search/illust?offset=9223372036854775807", "https://app-api.pixiv.net/v1/search/illust?offset=9223372036854775808"} {
		encoded, _ := json.Marshal(map[string]any{"illusts": []any{}, "next_url": next})
		rows = append(rows, migrationSearchRow{Name: "next:" + next, Input: migrationSearchInput{Word: "fixture"}, Bodies: []json.RawMessage{encoded}, Steps: []migrationSearchStep{{Action: "search"}}})
	}
	batch := func(ids []int, next int) json.RawMessage {
		var items []any
		for _, id := range ids {
			ai := 2
			if id == 1 {
				ai = 1
			}
			items = append(items, map[string]any{"id": id, "type": "illust", "illust_ai_type": ai, "create_date": "2026-01-01T00:00:00Z"})
		}
		if items == nil {
			items = []any{}
		}
		m := map[string]any{"illusts": items}
		if next > 0 {
			m["next_url"] = fmt.Sprintf("https://app-api.pixiv.net/v1/search/illust?offset=%d", next)
		}
		encoded, _ := json.Marshal(m)
		return encoded
	}
	for _, userID := range []int64{0, 7} {
		for _, mode := range []string{"keep", "foreign", "same", "legacy", "kind", "identity"} {
			rows = append(rows, migrationSearchRow{Name: fmt.Sprintf("cursor:%d:%s", userID, mode), UserID: userID, Input: migrationSearchInput{Word: "fixture", CursorContext: "private", AIMode: "only"}, Bodies: []json.RawMessage{batch([]int{1, 2, 3}, 30)}, Steps: []migrationSearchStep{{Action: "checkpoint", Consumed: 1}, {Action: "search", Mode: mode}}})
		}
		rows = append(rows, migrationSearchRow{Name: fmt.Sprintf("sequence:%d", userID), UserID: userID, Input: migrationSearchInput{Word: "fixture", AIMode: "only"}, Bodies: []json.RawMessage{batch([]int{1, 2, 3, 4}, 30), batch([]int{1}, 60), batch([]int{5, 6}, 0)}, Steps: []migrationSearchStep{{Action: "search"}, {Action: "checkpoint", Consumed: 1}, {Action: "search"}, {Action: "checkpoint", Consumed: 1}, {Action: "search"}, {Action: "search", Mode: "next"}, {Action: "search", Mode: "next"}, {Action: "checkpoint", Consumed: 9}, {Action: "search"}}})
	}
	for _, consumed := range []int{-1, 0, 1, 3, 9223372036854775807} {
		rows = append(rows, migrationSearchRow{Name: fmt.Sprintf("consumed:%d", consumed), Input: migrationSearchInput{Word: "fixture"}, Bodies: []json.RawMessage{batch([]int{1, 2, 3}, 0)}, Steps: []migrationSearchStep{{Action: "checkpoint", Consumed: consumed}, {Action: "checkpoint", Consumed: 1}, {Action: "search"}}})
	}
	for _, change := range []migrationSearchStep{{Action: "search", Word: "changed"}, {Action: "search", Context: "changed"}} {
		rows = append(rows, migrationSearchRow{Name: "binding:word=" + change.Word + ":context=" + change.Context, Input: migrationSearchInput{Word: "fixture", CursorContext: "private"}, Bodies: []json.RawMessage{body}, Steps: []migrationSearchStep{{Action: "checkpoint", Consumed: 1}, change}})
	}
	for index := range rows {
		row := &rows[index]
		row.Queries = []url.Values{}
		row.Results = []migrationSearchResult{}
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, row.UserID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			row.Queries = append(row.Queries, req.URL.Query())
			bodyIndex := 0
			switch req.URL.Query().Get("offset") {
			case "30":
				bodyIndex = 1
			case "60":
				bodyIndex = 2
			}
			if bodyIndex >= len(row.Bodies) {
				bodyIndex = 0
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})
		makeClient := func(userID int64) *pixiv.Client {
			var client *pixiv.Client
			var err error
			if userID > 0 {
				saved := row.UserID
				row.UserID = userID
				client, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
				row.UserID = saved
			} else {
				client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
			}
			if err != nil {
				t.Fatal(err)
			}
			return client
		}
		client := makeClient(row.UserID)
		var current, next sdk.Cursor
		for _, step := range row.Steps {
			request := row.Input.request()
			request.Cursor = current
			if step.Word != "" {
				request.Word = step.Word
			}
			if step.Context != "" {
				request.CursorContext = step.Context
			}
			target := client
			switch step.Mode {
			case "next":
				request.Cursor = next
				current = next
			case "same":
				target = makeClient(row.UserID)
			case "foreign":
				id := row.UserID
				if id > 0 {
					id++
				}
				target = makeClient(id)
			case "legacy", "kind", "identity":
				raw, _ := base64.RawURLEncoding.DecodeString(current.String())
				var envelope map[string]any
				_ = json.Unmarshal(raw, &envelope)
				switch step.Mode {
				case "legacy":
					envelope["b"] = 1
				case "kind":
					envelope["pl"] = base64.StdEncoding.EncodeToString([]byte(`{"k":"max_bookmark_id","v":30}`))
				case "identity":
					delete(envelope, "i")
					delete(envelope, "e")
					envelope["id"] = "99"
				}
				raw, _ = json.Marshal(envelope)
				request.Cursor, err = sdk.ParseCursor(base64.RawURLEncoding.EncodeToString(raw))
				if err != nil {
					t.Fatal(err)
				}
			}
			result := migrationSearchResult{Items: []pixiv.ArtworkDTO{}}
			if step.Action == "checkpoint" {
				cursor, callErr := target.CheckpointSearchArtworks(request, step.Consumed)
				err = callErr
				if err == nil {
					current = cursor
					result.Cursor = migrationSearchCursor(t, cursor)
				}
			} else {
				page, callErr := target.SearchArtworks(context.Background(), request)
				err = callErr
				if err == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
					}
					next = page.Next
					result.Cursor = migrationSearchCursor(t, page.Next)
				}
			}
			if err != nil {
				result.Reason = sdk.ReasonOf(err)
				result.Message = err.Error()
			}
			row.Results = append(row.Results, result)
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "search-artworks.json")
	if *migrationUpdateSearch {
		if err = os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("search contracts differ from fixed Go reference")
	}
}
