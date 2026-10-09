package pixiv_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"testing"

	accountpixiv "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	sdkpixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/require"
)

func loginCredentials(t *testing.T) sdkpixiv.Credentials {
	t.Helper()
	session, err := sdkpixiv.BeginLogin(sdkpixiv.LoginOptions{HTTPClient: &http.Client{Transport: pixivRoundTripper(func(r *http.Request) (*http.Response, error) {
		require.NoError(t, r.ParseForm())
		require.Equal(t, "authorization_code", r.Form.Get("grant_type"))
		return pixivJSONResponse(`{"access_token":"synthetic-access","refresh_token":" synthetic-refresh ","user":{"id":"42","name":"fresh"}}`), nil
	})}})
	require.NoError(t, err)
	credentials, err := session.Complete(context.Background(), "synthetic-code")
	require.NoError(t, err)
	return credentials
}

type loginRepository struct {
	*transferRepository
	getErr error
	cancel context.CancelFunc
}

func (r *loginRepository) SavePixivCredential(ctx context.Context, a accountpixiv.Account) error {
	err := r.transferRepository.SavePixivCredential(ctx, a)
	if r.cancel != nil {
		r.cancel()
	}
	return err
}
func (r *loginRepository) GetPixiv(ctx context.Context, id int64) (accountpixiv.Account, error) {
	if r.getErr != nil {
		*r.calls = append(*r.calls, "get")
		return accountpixiv.Account{}, r.getErr
	}
	return r.transferRepository.GetPixiv(ctx, id)
}

type loginDefaults struct {
	*managementDefaults
	reads      int
	summaryErr bool
}

func (d *loginDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	d.reads++
	if d.summaryErr && d.reads == 2 {
		*d.calls = append(*d.calls, "read")
		return 0, false, errors.New("summary default failed")
	}
	return d.managementDefaults.ReadPixivDefaultUserID()
}

func TestMigrationCompleteLoginSaveDefaultSummaryOrdering(t *testing.T) {
	credentials := loginCredentials(t)
	for _, tc := range []struct {
		name, failure string
		selected      int64
		use           bool
		calls         []string
		want          string
	}{
		{"first", "", 0, false, []string{"save", "read", "set", "get", "read"}, ""},
		{"preserve", "", 7, false, []string{"save", "read", "get", "read"}, ""},
		{"replace", "", 7, true, []string{"save", "read", "set", "get", "read"}, ""},
		{"save", "save", 0, false, []string{"save"}, "save pixiv account: save failed"},
		{"read", "read", 0, true, []string{"save", "read"}, "read pixiv default account: read failed"},
		{"set", "set", 0, false, []string{"save", "read", "set"}, "set pixiv default account: set failed"},
		{"summary-default", "read2", 0, false, []string{"save", "read", "set", "get", "read"}, "summary default failed"},
		{"summary", "get", 0, false, []string{"save", "read", "set", "get"}, "get failed"},
		{"missing-defaults", "nil", 0, false, []string{"save"}, "read pixiv default account: pixiv default account store is not configured"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			calls := []string{}
			repo := &loginRepository{transferRepository: &transferRepository{pixivTestRepository: newPixivTestRepository(), calls: &calls}}
			d := &managementDefaults{pixivTestDefaults: &pixivTestDefaults{userID: tc.selected, ok: tc.selected != 0}, calls: &calls}
			switch tc.failure {
			case "save":
				repo.saveErr = errors.New("save failed")
			case "read":
				d.readErr = errors.New("read failed")
			case "set":
				d.setErr = errors.New("set failed")
			case "get":
				repo.getErr = errors.New("get failed")
			}
			var defaults accountpixiv.DefaultStore = &loginDefaults{managementDefaults: d, summaryErr: tc.failure == "read2"}
			if tc.failure == "nil" {
				defaults = nil
			}
			out, err := accountpixiv.NewService(repo, defaults).CompleteLogin(context.Background(), credentials, tc.use)
			if tc.want == "" {
				require.NoError(t, err)
				require.Equal(t, "fresh", out.Username)
				require.Equal(t, tc.selected == 0 || tc.use, out.Default)
			} else {
				require.EqualError(t, err, tc.want)
			}
			require.Equal(t, tc.calls, calls)
			if tc.failure == "read2" || tc.failure == "get" {
				require.True(t, d.ok)
				require.Equal(t, int64(42), d.userID)
			}
			if tc.failure != "save" {
				require.Equal(t, " synthetic-refresh ", string(repo.accounts[42].RefreshTokenCopy()))
				require.Equal(t, int64(1), repo.accounts[42].CredentialRevision)
			}
			encoded, err := json.Marshal(out)
			require.NoError(t, err)
			require.NotContains(t, string(encoded), "synthetic-refresh")
			require.NotContains(t, fmt.Sprintf("%#v", out), "synthetic-refresh")
		})
	}
	calls := []string{}
	repo := &transferRepository{pixivTestRepository: newPixivTestRepository(), calls: &calls}
	_, err := accountpixiv.NewService(repo, nil).CompleteLogin(context.Background(), sdkpixiv.Credentials{}, false)
	require.EqualError(t, err, "login credentials are incomplete")
	require.Empty(t, calls)
}

func TestMigrationLoginCompletionConsumesOAuthBeforeMissingService(t *testing.T) {
	calls := 0
	start, err := (accountpixiv.LoginService{}).Start(accountpixiv.LoginRequest{Options: sdkpixiv.LoginOptions{HTTPClient: &http.Client{Transport: pixivRoundTripper(func(*http.Request) (*http.Response, error) {
		calls++
		return pixivJSONResponse(`{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","user":{"id":"42","name":"fresh"}}`), nil
	})}}})
	require.NoError(t, err)
	_, err = (accountpixiv.LoginService{}).Complete(context.Background(), accountpixiv.LoginStart{}, accountpixiv.LoginCompleteRequest{})
	require.EqualError(t, err, "login session is not initialized")
	_, err = (accountpixiv.LoginService{}).Complete(context.Background(), start, accountpixiv.LoginCompleteRequest{CallbackOrCode: "synthetic-code"})
	require.EqualError(t, err, "pixiv account service is not configured")
	_, err = (accountpixiv.LoginService{}).Complete(context.Background(), start, accountpixiv.LoginCompleteRequest{CallbackOrCode: "synthetic-code"})
	require.ErrorContains(t, err, "login session was already used")
	require.Equal(t, 1, calls)
}

func TestMigrationCompleteLoginCanceledDatabaseSaveAndPostSaveFailure(t *testing.T) {
	credentials := loginCredentials(t)
	db, err := database.Open(t.TempDir())
	require.NoError(t, err)
	defer db.Close()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err = accountpixiv.NewService(db, &pixivTestDefaults{}).CompleteLogin(ctx, credentials, false)
	require.ErrorIs(t, err, context.Canceled)
	rows, err := db.ListPixiv(context.Background())
	require.NoError(t, err)
	require.Empty(t, rows)
	calls := []string{}
	ctx, cancel = context.WithCancel(context.Background())
	repo := &loginRepository{transferRepository: &transferRepository{pixivTestRepository: newPixivTestRepository(), calls: &calls}, cancel: cancel, getErr: context.Canceled}
	d := &managementDefaults{pixivTestDefaults: &pixivTestDefaults{}, calls: &calls}
	_, err = accountpixiv.NewService(repo, d).CompleteLogin(ctx, credentials, false)
	require.ErrorIs(t, err, context.Canceled)
	require.Equal(t, []string{"save", "read", "set", "get"}, calls)
	require.True(t, d.ok)
	require.Equal(t, int64(42), d.userID)
	require.Contains(t, repo.accounts, int64(42))
}

func TestMigrationLoginCanceledOAuthPreservesSafeDisplayAndCause(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	start, err := (accountpixiv.LoginService{}).Start(accountpixiv.LoginRequest{Options: sdkpixiv.LoginOptions{HTTPClient: &http.Client{Transport: pixivRoundTripper(func(r *http.Request) (*http.Response, error) { cancel(); return nil, r.Context().Err() })}}})
	require.NoError(t, err)
	_, err = (accountpixiv.LoginService{}).Complete(ctx, start, accountpixiv.LoginCompleteRequest{CallbackOrCode: "synthetic-code"})
	require.ErrorIs(t, err, context.Canceled)
	require.EqualError(t, err, "pixiv:Complete: upstream_unavailable: pixiv upstream transport failed")
}
