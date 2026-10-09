package pixiv_test

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"reflect"
	"strings"
	"testing"
	"time"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type validationRepository struct {
	account.Repository
	calls   *[]string
	failure string
	gets    int
}

func (r *validationRepository) GetPixiv(ctx context.Context, id int64) (account.Account, error) {
	*r.calls = append(*r.calls, "get")
	r.gets++
	if r.failure == fmt.Sprintf("get%d", r.gets) {
		return account.Account{}, errors.New("get failed")
	}
	if r.failure == "record" {
		id = 7
	}
	return r.Repository.GetPixiv(ctx, id)
}
func (r *validationRepository) RotatePixivCredentials(ctx context.Context, id, revision int64, token []byte) error {
	*r.calls = append(*r.calls, "rotate")
	if r.failure == "rotate" {
		return account.ErrCredentialConflict
	}
	return r.Repository.RotatePixivCredentials(ctx, id, revision, token)
}
func (r *validationRepository) UpdatePixivMetadata(ctx context.Context, id int64, name string, premium *bool, checked *int64) error {
	*r.calls = append(*r.calls, "metadata")
	if r.failure == "metadata" {
		return errors.New("metadata failed")
	}
	return r.Repository.UpdatePixivMetadata(ctx, id, name, premium, checked)
}
func TestMigrationAccountValidationRotationAndRefreshOrdering(t *testing.T) {
	for _, tc := range []struct {
		name      string
		refresh   bool
		failure   string
		want      []string
		committed bool
		metadata  bool
	}{
		{"check", false, "", []string{"get", "oauth", "rotate"}, true, false},
		{"check_identity", false, "identity", []string{"get", "oauth"}, false, false},
		{"check_revision", false, "rotate", []string{"get", "oauth", "rotate"}, false, false},
		{"check_missing", false, "get1", []string{"get"}, false, false},
		{"refresh_missing", true, "get1", []string{"get"}, false, false},
		{"check_oauth_cancel", false, "oauth_cancel", []string{"get", "oauth"}, false, false},
		{"refresh_oauth_cancel", true, "oauth_cancel", []string{"get", "oauth"}, false, false},
		{"refresh_profile_cancel", true, "profile_cancel", []string{"get", "oauth", "rotate", "profile"}, true, false},
		{"refresh", true, "", []string{"get", "oauth", "rotate", "profile", "get", "metadata", "get"}, true, true},
		{"refresh_identity", true, "identity", []string{"get", "oauth"}, false, false},
		{"refresh_revision", true, "rotate", []string{"get", "oauth", "rotate"}, false, false},
		{"refresh_profile", true, "profile", []string{"get", "oauth", "rotate", "profile"}, true, false},
		{"refresh_metadata_read", true, "get2", []string{"get", "oauth", "rotate", "profile", "get"}, true, false},
		{"refresh_metadata", true, "metadata", []string{"get", "oauth", "rotate", "profile", "get", "metadata"}, true, false},
		{"refresh_summary_read", true, "get3", []string{"get", "oauth", "rotate", "profile", "get", "metadata", "get"}, true, true},
		{"refresh_default", true, "default", []string{"get", "oauth", "rotate", "profile", "get", "metadata", "get"}, true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			db, err := database.Open(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			defer db.Close()
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			if err = db.SavePixivCredential(ctx, account.New(42, "stored", []byte("synthetic-input"))); err != nil {
				t.Fatal(err)
			}
			calls := []string{}
			repo := &validationRepository{Repository: db, calls: &calls, failure: tc.failure}
			defaults := &pixivTestDefaults{userID: 42, ok: true}
			if tc.failure == "default" {
				defaults.readErr = errors.New("default failed")
			}
			client := &http.Client{Transport: pixivRoundTripper(func(req *http.Request) (*http.Response, error) {
				if req.URL.Host == "oauth.secure.pixiv.net" {
					calls = append(calls, "oauth")
					if tc.failure == "oauth_cancel" {
						cancel()
						return nil, context.Canceled
					}
					id := 42
					if tc.failure == "identity" {
						id = 43
					}
					return pixivJSONResponse(fmt.Sprintf(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":"%d","name":"oauth-name"}}`, id)), nil
				}
				calls = append(calls, "profile")
				if tc.failure == "profile_cancel" {
					cancel()
					return nil, context.Canceled
				}
				if req.Method != "GET" || req.URL.Path != "/v1/user/detail" || req.URL.Query().Get("user_id") != "42" || req.URL.Query().Get("filter") != "for_android" {
					t.Fatalf("profile request %s", req.URL)
				}
				stored, _ := db.GetPixiv(ctx, 42)
				if stored.CredentialRevision != 2 {
					t.Fatal("profile before commit")
				}
				if tc.failure == "profile" {
					return pixivJSONResponse(`{}`), nil
				}
				return pixivJSONResponse(`{"user":{"id":42,"name":"profile-name"},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}}`), nil
			})}
			before := time.Now().Unix()
			service := account.NewService(repo, defaults)
			var result account.AccountSummary
			if tc.refresh {
				result, err = service.RefreshAccountWith(ctx, 42, pixiv.Options{HTTPClient: client})
			} else {
				result, err = service.CheckAccountWith(ctx, 42, pixiv.Options{HTTPClient: client})
			}
			if (err != nil) != (tc.failure != "") {
				t.Fatalf("error %v", err)
			}
			expectedError := map[string]string{
				"identity": "persist rotated pixiv credentials: pixiv:OpenAccountClient: local_state_error: credential identity does not match selected account",
				"rotate":   "persist rotated pixiv credentials: pixiv account credential revision conflict",
				"profile":  "pixiv:CurrentUser: malformed_upstream_response",
				"get1":     "get failed", "get2": "get failed", "get3": "get failed",
				"metadata": "metadata failed", "default": "default failed",
				"oauth_cancel":   "pixiv:Open: upstream_unavailable: pixiv upstream transport failed",
				"profile_cancel": "pixiv:CurrentUser: upstream_unavailable: pixiv upstream transport failed",
			}[tc.failure]
			if tc.refresh && tc.failure == "get1" {
				expectedError = "select pixiv account: get failed"
			}
			if err != nil && err.Error() != expectedError {
				t.Fatalf("error %q want %q", err.Error(), expectedError)
			}
			if !reflect.DeepEqual(calls, tc.want) {
				t.Fatalf("calls %v want %v", calls, tc.want)
			}
			stored, _ := db.GetPixiv(context.Background(), 42)
			revision := int64(1)
			if tc.committed {
				revision = 2
			}
			if stored.CredentialRevision != revision {
				t.Fatalf("revision %d", stored.CredentialRevision)
			}
			if strings.HasSuffix(tc.failure, "cancel") && !errors.Is(err, context.Canceled) {
				t.Fatalf("cancellation %v", err)
			}
			if stored.Username != "stored" {
				t.Fatal("OAuth/profile name replaced stored metadata")
			}
			if tc.metadata {
				if stored.PremiumStatus == nil || !*stored.PremiumStatus || stored.PremiumCheckedAt == nil || *stored.PremiumCheckedAt < before || *stored.PremiumCheckedAt > time.Now().Unix() {
					t.Fatal("premium metadata")
				}
			} else if stored.PremiumStatus != nil || stored.PremiumCheckedAt != nil {
				t.Fatal("unexpected metadata")
			}
			if err == nil {
				if tc.refresh {
					if result.Username != "stored" || !result.Default || result.Premium == nil || !*result.Premium || !result.PoolStatusKnown || !result.Eligible {
						t.Fatalf("summary %+v", result)
					}
				} else {
					if result.Username != "oauth-name" || result.Default || result.Premium != nil || result.PoolStatusKnown {
						t.Fatalf("check summary %+v", result)
					}
				}
			}
		})
	}
}

func TestMigrationAccountRefreshPreservesSchedulingAndReadsFinalDefault(t *testing.T) {
	for _, premium := range []bool{false, true} {
		t.Run(fmt.Sprint(premium), func(t *testing.T) {
			db, err := database.Open(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			defer db.Close()
			ctx := context.Background()
			for _, id := range []int64{42, 7} {
				if err = db.SavePixivCredential(ctx, account.New(id, "stored", []byte("synthetic-input"))); err != nil {
					t.Fatal(err)
				}
			}
			if _, err = db.DB().Exec("UPDATE pixiv_account SET schedulable=0,pool_frozen_until=1,pool_last_selected=1,premium_status=1,premium_checked_at=123 WHERE user_id=42"); err != nil {
				t.Fatal(err)
			}
			defaults := &pixivTestDefaults{}
			client := &http.Client{Transport: pixivRoundTripper(func(req *http.Request) (*http.Response, error) {
				if req.URL.Host == "oauth.secure.pixiv.net" {
					return pixivJSONResponse(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"oauth-name"}}`), nil
				}
				defaults.userID, defaults.ok = 7, true
				if err = db.UpdatePixivMetadata(ctx, 42, "fresh-stored", nil, nil); err != nil {
					t.Fatal(err)
				}
				return pixivJSONResponse(fmt.Sprintf(`{"user":{"id":42,"name":"profile-name"},"profile":{"is_premium":%t},"profile_publicity":{},"workspace":{}}`, premium)), nil
			})}
			result, err := account.NewService(db, defaults).RefreshAccountWith(ctx, 42, pixiv.Options{HTTPClient: client})
			if err != nil {
				t.Fatal(err)
			}
			if result.Default || result.Username != "fresh-stored" || result.Premium == nil || *result.Premium != premium || result.Schedulable || result.Eligible || result.PoolFrozenUntil != nil || !result.PoolLastSelected || !result.PoolStatusKnown {
				t.Fatalf("fresh summary %+v", result)
			}
			stored, err := db.GetPixiv(ctx, 42)
			if err != nil {
				t.Fatal(err)
			}
			if stored.CredentialRevision != 2 || stored.Schedulable || stored.PoolFrozenUntil == nil || *stored.PoolFrozenUntil != 1 || !stored.PoolLastSelected || stored.Username != "fresh-stored" {
				t.Fatal("metadata write altered credential or pool fields")
			}
		})
	}
}

func TestMigrationAccountValidationDistinguishesRequestedAndReturnedIdentity(t *testing.T) {
	for _, check := range []bool{false, true} {
		t.Run(fmt.Sprint(check), func(t *testing.T) {
			db, err := database.Open(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			defer db.Close()
			ctx := context.Background()
			for _, id := range []int64{42, 7} {
				if err = db.SavePixivCredential(ctx, account.New(id, "stored", []byte("synthetic-input"))); err != nil {
					t.Fatal(err)
				}
			}
			calls := []string{}
			repo := &validationRepository{Repository: db, calls: &calls, failure: "record"}
			client := &http.Client{Transport: pixivRoundTripper(func(req *http.Request) (*http.Response, error) {
				if req.URL.Host != "oauth.secure.pixiv.net" {
					t.Fatal("identity mismatch reached content")
				}
				return pixivJSONResponse(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":7,"name":"oauth-name"}}`), nil
			})}
			service := account.NewService(repo, &pixivTestDefaults{})
			if check {
				result, e := service.CheckAccountWith(ctx, 42, pixiv.Options{HTTPClient: client})
				if e != nil || result.UserID != 7 {
					t.Fatalf("check %v %v", result, e)
				}
			} else {
				_, err = service.OpenAccountClientWith(ctx, 42, pixiv.Options{HTTPClient: client})
				if err == nil || err.Error() != "persist rotated pixiv credentials: pixiv:OpenAccountClient: local_state_error: credential identity does not match selected account" {
					t.Fatal(err)
				}
			}
			original, _ := db.GetPixiv(ctx, 42)
			returned, _ := db.GetPixiv(ctx, 7)
			if original.CredentialRevision != 1 {
				t.Fatal("requested account changed")
			}
			want := int64(1)
			if check {
				want = 2
			}
			if returned.CredentialRevision != want {
				t.Fatal("returned account rotation")
			}
		})
	}
}
