package pixiv_test

import (
	"context"
	"encoding/json"
	"errors"
	accountpixiv "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	sdkpixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/require"
	"net/http"
	"testing"
)

type transferRepository struct {
	*pixivTestRepository
	calls   *[]string
	saveErr error
}

func (r *transferRepository) SavePixivCredential(ctx context.Context, a accountpixiv.Account) error {
	*r.calls = append(*r.calls, "save")
	if r.saveErr != nil {
		return r.saveErr
	}
	return r.pixivTestRepository.SavePixivCredential(ctx, a)
}
func (r *transferRepository) GetPixiv(ctx context.Context, id int64) (accountpixiv.Account, error) {
	*r.calls = append(*r.calls, "get")
	return r.pixivTestRepository.GetPixiv(ctx, id)
}
func (r *transferRepository) ListPixiv(ctx context.Context) ([]accountpixiv.Account, error) {
	*r.calls = append(*r.calls, "list")
	return r.pixivTestRepository.ListPixiv(ctx)
}

type transferTransport struct{ calls *[]string }

func (t *transferTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	*t.calls = append(*t.calls, "oauth")
	if err := r.ParseForm(); err != nil {
		return nil, err
	}
	if r.Form.Get("refresh_token") != "synthetic-input" {
		return nil, errors.New("unexpected token")
	}
	return pixivJSONResponse(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","user":{"id":"42","name":"fresh"}}`), nil
}
func (t *transferTransport) CloseIdleConnections() { *t.calls = append(*t.calls, "close") }

func TestMigrationImportTransferOrderAndCommittedFailure(t *testing.T) {
	for _, tc := range []struct {
		name, failure string
		expected      []string
		want          string
	}{
		{"success", "", []string{"oauth", "save", "read", "set", "get", "read"}, ""},
		{"save-failure", "save", []string{"oauth", "save"}, "save pixiv account: save failed"},
		{"read-failure", "read", []string{"oauth", "save", "read"}, "read pixiv default account: read failed"},
		{"set-failure", "set", []string{"oauth", "save", "read", "set"}, "set pixiv default account: set failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			calls := []string{}
			repo := &transferRepository{pixivTestRepository: newPixivTestRepository(), calls: &calls}
			d := &managementDefaults{pixivTestDefaults: &pixivTestDefaults{}, calls: &calls}
			switch tc.failure {
			case "save":
				repo.saveErr = errors.New("save failed")
			case "read":
				d.readErr = errors.New("read failed")
			case "set":
				d.setErr = errors.New("set failed")
			}
			s := accountpixiv.NewService(repo, d)
			out, err := s.ImportAccountWith(context.Background(), "  synthetic-input  ", false, sdkpixiv.Options{HTTPClient: &http.Client{Transport: &transferTransport{calls: &calls}}})
			if tc.want == "" {
				require.NoError(t, err)
				require.Equal(t, "fresh", out.Username)
				require.True(t, out.Default)
			} else {
				require.EqualError(t, err, tc.want)
			}
			require.Equal(t, tc.expected, calls)
			if tc.failure != "save" {
				require.Equal(t, "synthetic-rotated", string(repo.accounts[42].RefreshTokenCopy()))
			}
		})
	}
}

func TestMigrationRestoreDefaultFailureLeavesCommittedAccounts(t *testing.T) {
	repo := newPixivTestRepository()
	d := &pixivTestDefaults{setErr: errors.New("set failed")}
	s := accountpixiv.NewService(repo, d)
	_, err := s.RestoreAccounts(context.Background(), []accountpixiv.RestoreAccountInput{{Account: accountpixiv.AccountSummary{UserID: 42, Username: "synthetic"}, RefreshToken: "  synthetic-token  "}})
	require.EqualError(t, err, "set pixiv default account: set failed")
	require.Equal(t, "synthetic-token", string(repo.accounts[42].RefreshTokenCopy()))
	require.False(t, d.ok)
}

func TestMigrationTokenExportReadsDefaultsPerAccountWithoutPoolCleanup(t *testing.T) {
	calls := []string{}
	repo := &transferRepository{pixivTestRepository: newPixivTestRepository(pixivAccountFixture(42, "first", "synthetic-first", 1, true), pixivAccountFixture(7, "second", "synthetic-second", 2, true)), calls: &calls}
	d := &managementDefaults{pixivTestDefaults: &pixivTestDefaults{}, calls: &calls}
	s := accountpixiv.NewService(repo, d)
	out, err := s.AccountsWithTokens(context.Background())
	require.NoError(t, err)
	require.Equal(t, []string{"list", "read", "list", "read", "list"}, calls)
	require.Equal(t, int64(42), out[0].UserID)
	require.True(t, out[0].Default)
	require.False(t, out[1].Default)
	require.Equal(t, "synthetic-second", out[1].RefreshToken())
}

func TestMigrationRestoreDatabaseAtomicityOrderMetadataAndDefaultPolicy(t *testing.T) {
	for _, mode := range []string{"implicit", "explicit", "duplicate", "trigger", "set-failure"} {
		t.Run(mode, func(t *testing.T) {
			ctx := context.Background()
			db, err := database.Open(t.TempDir())
			require.NoError(t, err)
			defer db.Close()
			old := pixivAccountFixture(7, "old", "synthetic-old", 1, false)
			old.CredentialRevision = 4
			require.NoError(t, db.SavePixivCredential(ctx, old))
			_, err = db.DB().Exec("UPDATE pixiv_account SET credential_revision=4,schedulable=0,premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1 WHERE user_id=7")
			require.NoError(t, err)
			d := &pixivTestDefaults{}
			if mode == "explicit" {
				d.userID, d.ok = 999, true
			}
			if mode == "set-failure" {
				d.setErr = errors.New("set failed")
			}
			inputs := []accountpixiv.RestoreAccountInput{{Account: accountpixiv.AccountSummary{UserID: 7, Username: "new"}, RefreshToken: " synthetic-new "}, {Account: accountpixiv.AccountSummary{UserID: 42, Username: "forty-two"}, RefreshToken: "synthetic-42", IsBundleDefault: true}, {Account: accountpixiv.AccountSummary{UserID: 9}, RefreshToken: "synthetic-9", IsBundleDefault: true}}
			if mode == "duplicate" {
				inputs[2].Account.UserID = 7
			}
			if mode == "trigger" {
				_, err = db.DB().Exec("CREATE TRIGGER fail_restore BEFORE INSERT ON pixiv_account WHEN NEW.user_id=42 BEGIN SELECT RAISE(ABORT,'synthetic failure'); END")
				require.NoError(t, err)
			}
			out, err := accountpixiv.NewService(db, d).RestoreAccounts(ctx, inputs)
			rows, listErr := db.ListPixiv(ctx)
			require.NoError(t, listErr)
			if mode == "duplicate" || mode == "trigger" {
				require.Error(t, err)
				require.Len(t, rows, 1)
				require.Equal(t, "synthetic-old", string(rows[0].RefreshTokenCopy()))
				require.False(t, d.ok)
				return
			}
			require.Len(t, rows, 3)
			require.Equal(t, []int64{7, 42, 9}, []int64{rows[0].UserID, rows[1].UserID, rows[2].UserID})
			require.Equal(t, int64(5), rows[0].CredentialRevision)
			require.Equal(t, "synthetic-new", string(rows[0].RefreshTokenCopy()))
			require.False(t, rows[0].Schedulable)
			require.True(t, *rows[0].PremiumStatus)
			require.Equal(t, int64(123), *rows[0].PremiumCheckedAt)
			require.Equal(t, int64(4102444800), *rows[0].PoolFrozenUntil)
			require.True(t, rows[0].PoolLastSelected)
			require.True(t, rows[1].Schedulable)
			if mode == "set-failure" {
				require.EqualError(t, err, "set pixiv default account: set failed")
				require.False(t, d.ok)
				return
			}
			require.NoError(t, err)
			require.True(t, out.Accounts[0].IsReplacement)
			require.False(t, out.Accounts[1].IsReplacement)
			if mode == "explicit" {
				require.Equal(t, int64(999), out.ResultingDefault)
			} else {
				require.Equal(t, int64(42), out.ResultingDefault)
			}
		})
	}
}

type changingTransferDefaults struct {
	calls  *[]string
	reads  int
	values []int64
}

func (d *changingTransferDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	*d.calls = append(*d.calls, "read")
	id := d.values[d.reads]
	d.reads++
	return id, id != 0, nil
}
func (d *changingTransferDefaults) SetPixivDefaultUserID(id int64) error {
	*d.calls = append(*d.calls, "set")
	return nil
}
func (d *changingTransferDefaults) ClearPixivDefaultUserID() error { return nil }
func TestMigrationTransferReadsChangingDefaultAndCurrentUserFreshly(t *testing.T) {
	ctx := context.Background()
	calls := []string{}
	repo := &transferRepository{pixivTestRepository: newPixivTestRepository(pixivAccountFixture(42, "first", "synthetic-first", 1, true), pixivAccountFixture(7, "second", "synthetic-second", 2, true)), calls: &calls}
	d := &changingTransferDefaults{calls: &calls, values: []int64{42, 7, 7}}
	s := accountpixiv.NewService(repo, d)
	out, err := s.AccountsWithTokens(ctx)
	require.NoError(t, err)
	require.True(t, out[0].Default)
	require.True(t, out[1].Default)
	current, err := s.CurrentUser(ctx)
	require.NoError(t, err)
	require.Equal(t, int64(7), current.UserID)
	require.Equal(t, []string{"list", "read", "read", "read", "get", "get"}, calls)
}
func TestMigrationImportKeepsExistingDefaultAndNormalizesOnlySummaryFreeze(t *testing.T) {
	ctx := context.Background()
	db, err := database.Open(t.TempDir())
	require.NoError(t, err)
	defer db.Close()
	require.NoError(t, db.SavePixivCredential(ctx, accountpixiv.New(42, "old", []byte("synthetic-old"))))
	_, err = db.DB().Exec("UPDATE pixiv_account SET pool_frozen_until=1,premium_status=1,premium_checked_at=123,schedulable=0 WHERE user_id=42")
	require.NoError(t, err)
	d := &pixivTestDefaults{userID: 7, ok: true}
	calls := []string{}
	out, err := accountpixiv.NewService(db, d).ImportAccountWith(ctx, "synthetic-input", false, sdkpixiv.Options{HTTPClient: &http.Client{Transport: &transferTransport{calls: &calls}}})
	require.NoError(t, err)
	require.False(t, out.Default)
	require.Nil(t, out.PoolFrozenUntil)
	require.True(t, *out.Premium)
	require.False(t, out.Eligible)
	stored, err := db.GetPixiv(ctx, 42)
	require.NoError(t, err)
	require.Equal(t, int64(1), *stored.PoolFrozenUntil)
	require.Equal(t, int64(123), *stored.PremiumCheckedAt)
	require.Equal(t, int64(7), d.userID)
	require.Equal(t, []string{"oauth"}, calls)
}

func TestMigrationRawTokenExportRetainsInvalidUTF8Bytes(t *testing.T) {
	repo := newPixivTestRepository(pixivAccountFixture(42, "synthetic", "synthetic-\xff-token", 1, true))
	s := accountpixiv.NewService(repo, &pixivTestDefaults{userID: 42, ok: true})
	rows, err := s.AccountsWithTokens(context.Background())
	require.NoError(t, err)
	require.Equal(t, []byte("synthetic-\xff-token"), []byte(rows[0].RefreshToken()))
}

func TestMigrationBundleTokenJSONReplacesEachInvalidByte(t *testing.T) {
	for _, token := range []string{"synthetic-\xff-token", "synthetic-\xe2\x82-token"} {
		repo := newPixivTestRepository(pixivAccountFixture(42, "synthetic", token, 1, true))
		rows, err := accountpixiv.NewService(repo, &pixivTestDefaults{userID: 42, ok: true}).AccountsWithTokens(context.Background())
		require.NoError(t, err)
		body, err := json.Marshal(struct {
			RefreshToken string `json:"refresh_token"`
		}{rows[0].RefreshToken()})
		require.NoError(t, err)
		if token == "synthetic-\xff-token" {
			require.Equal(t, `{"refresh_token":"synthetic-�-token"}`, string(body))
		} else {
			require.Equal(t, `{"refresh_token":"synthetic-��-token"}`, string(body))
		}
	}
}
