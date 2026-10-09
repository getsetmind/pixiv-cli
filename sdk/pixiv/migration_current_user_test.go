package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateCurrentUser = flag.Bool("migration-update-current-user", false, "capture current user identity and requests")

type migrationCurrentUserRequest struct {
	migrationUserWireRequest
	UserIDHeader string `json:"user_id_header"`
}

func TestMigrationCurrentUserPreservesIdentityDetailAndErrors(t *testing.T) {
	type row struct {
		Name       string                        `json:"name"`
		Source     string                        `json:"source"`
		SourceCase string                        `json:"source_case"`
		Verified   bool                          `json:"verified"`
		Canceled   bool                          `json:"canceled"`
		Status     int                           `json:"status"`
		Username   string                        `json:"username"`
		ID         int64                         `json:"id"`
		DTO        json.RawMessage               `json:"dto"`
		Message    string                        `json:"message"`
		Requests   []migrationCurrentUserRequest `json:"requests"`
	}
	rows := []row{
		{Name: "unknown_identity", Canceled: true},
		{Name: "rich", Verified: true, Source: "user-detail.json", SourceCase: "rich", Status: 200},
		{Name: "uppercase_detail", Verified: true, Source: "user-wire.json", SourceCase: "all_detail_fields_uppercase", Status: 200},
		{Name: "duplicate_workspace_replaces", Verified: true, Source: "user-wire.json", SourceCase: "duplicate_workspace:objects_replace", Status: 200},
		{Name: "invalid_non_dto_field", Verified: true, Source: "user-wire.json", SourceCase: `non_dto_profile_field_still_validated:"birth":7`, Status: 200},
		{Name: "invalid_publicity", Verified: true, Source: "user-detail.json", SourceCase: "publicity:gender:unknown", Status: 200},
		{Name: "missing_profile", Verified: true, Source: "user-detail.json", SourceCase: "missing:profile", Status: 200},
		{Name: "unauthorized", Verified: true, Status: 401},
		{Name: "upstream_failure", Verified: true, Status: 503},
	}
	for index := range rows {
		t.Run(rows[index].Name, func(t *testing.T) {
			current := &rows[index]
			body := `{"error":{"message":"fixture failure"}}`
			if current.Source != "" {
				data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", current.Source))
				if err != nil {
					t.Fatal(err)
				}
				var sources []struct {
					Name      string          `json:"name"`
					Operation string          `json:"operation"`
					Body      json.RawMessage `json:"body"`
				}
				if err := json.Unmarshal(data, &sources); err != nil {
					t.Fatal(err)
				}
				found := false
				for _, source := range sources {
					if source.Name == current.SourceCase && (source.Operation == "" || source.Operation == "User") {
						body = string(source.Body)
						if current.Source == "user-wire.json" {
							if err := json.Unmarshal(source.Body, &body); err != nil {
								t.Fatal(err)
							}
						}
						found = true
						break
					}
				}
				if !found {
					t.Fatalf("missing shared fixture %s/%s", current.Source, current.SourceCase)
				}
			}
			current.Requests = []migrationCurrentUserRequest{}
			transport := migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
				if request.URL.Host == "oauth.secure.pixiv.net" {
					return oauthResponse(), nil
				}
				if !current.Verified {
					t.Fatal("unknown identity reached transport")
				}
				want := url.Values{"filter": {"for_android"}, "user_id": {"42"}}
				if request.Method != "GET" || request.URL.Path != "/v1/user/detail" || request.URL.Query().Encode() != want.Encode() || request.Header.Get("Authorization") != "Bearer new-access-token" {
					t.Fatalf("unexpected current user request: %s %s", request.Method, request.URL)
				}
				current.Requests = append(current.Requests, migrationCurrentUserRequest{migrationUserWireRequest{request.Method, request.URL.Path, request.URL.Query()}, request.Header.Get("X-User-Id")})
				return &http.Response{StatusCode: current.Status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(body)), Request: request}, nil
			})
			options := pixiv.Options{HTTPClient: &http.Client{Transport: transport}}
			client, err := pixiv.NewWith("fixture-access", options)
			if err != nil {
				t.Fatal(err)
			}
			if current.Verified {
				client, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", options)
				if err != nil {
					t.Fatal(err)
				}
			}
			ctx := context.Background()
			if current.Canceled {
				canceled, cancel := context.WithCancel(ctx)
				cancel()
				ctx = canceled
			}
			detail, err := client.CurrentUser(ctx, pixiv.CurrentUserRequest{})
			current.Username, current.ID = client.Username(), client.UserID()
			if err != nil {
				current.Message = err.Error()
				current.DTO = json.RawMessage("null")
				if detail.User.ID != 0 {
					t.Fatal("failed detail exposed partial user")
				}
			} else {
				current.DTO, err = json.Marshal(pixiv.ToUserDetailDTO(detail))
				if err != nil {
					t.Fatal(err)
				}
			}
			if current.Verified && (current.Username != "tester" || current.ID != 42) {
				t.Fatalf("cached identity changed: %d/%s", current.ID, current.Username)
			}
			if !current.Verified && (current.Username != "" || current.ID != 0 || current.Message != "pixiv:CurrentUser: unauthorized: current user identity is unknown") {
				t.Fatalf("unknown identity contract: %#v", current)
			}
		})
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "current-user.json")
	if *migrationUpdateCurrentUser {
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
		t.Fatal("current user differs from frozen Go identity/detail/error contract")
	}
}

func TestMigrationUsernameIsCachedVerifiedIdentitySnapshot(t *testing.T) {
	calls := 0
	options := pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
		calls++
		if request.URL.Host != "oauth.secure.pixiv.net" {
			t.Fatal("cached identity accessor reached content transport")
		}
		return oauthResponse(), nil
	})}}
	unverified, err := pixiv.NewWith("fixture-access", options)
	if err != nil {
		t.Fatal(err)
	}
	if unverified.Username() != "" || unverified.UserID() != 0 || calls != 0 {
		t.Fatal("NewWith inferred an unverified identity or made an HTTP request")
	}
	client, credentials, err := pixiv.OpenWith(context.Background(), "fixture-refresh", options)
	if err != nil {
		t.Fatal(err)
	}
	copied := credentials
	copied.Username = "mutated copy"
	credentials.Username, credentials.UserID = "mutated original", 999
	alias := client
	for index := 0; index < 3; index++ {
		if alias.Username() != "tester" || alias.UserID() != 42 {
			t.Fatal("cached identity changed after credential mutation")
		}
	}
	if calls != 1 {
		t.Fatalf("identity accessor made HTTP requests: %d", calls)
	}
}

func TestMigrationUserIdentityHeaderUsesOnlyVerifiedCredentials(t *testing.T) {
	for _, verified := range []bool{false, true} {
		t.Run(map[bool]string{false: "token_only", true: "verified"}[verified], func(t *testing.T) {
			calls := 0
			options := pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
				if request.URL.Host == "oauth.secure.pixiv.net" {
					return oauthResponse(), nil
				}
				calls++
				want := ""
				if verified {
					want = "42"
				}
				if request.Header.Get("X-User-Id") != want {
					t.Fatalf("identity header = %q, want %q", request.Header.Get("X-User-Id"), want)
				}
				if request.URL.Path == "/v2/illust/follow" {
					return jsonResponse(`{"illusts":[],"next_url":null}`), nil
				}
				if request.URL.Path != "/v1/user/detail" {
					t.Fatalf("unexpected GET %s", request.URL.Path)
				}
				return jsonResponse(`{"user":{"id":31},"profile":{},"profile_publicity":{},"workspace":{}}`), nil
			})}}
			client, err := pixiv.NewWith("fixture-access", options)
			if err != nil {
				t.Fatal(err)
			}
			if verified {
				client, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", options)
				if err != nil {
					t.Fatal(err)
				}
			}
			if _, err = client.User(context.Background(), pixiv.UserRequest{UserID: 31}); err != nil {
				t.Fatal(err)
			}
			if _, err = client.FollowingArtworks(context.Background(), pixiv.FollowingArtworksRequest{}); err != nil {
				t.Fatal(err)
			}
			if calls != 2 {
				t.Fatalf("content requests = %d", calls)
			}
		})
	}
}
