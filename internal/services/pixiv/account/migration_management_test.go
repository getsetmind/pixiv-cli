package pixiv_test

import (
	"context"
	"errors"
	"testing"

	accountpixiv "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/stretchr/testify/require"
)

type managementRepository struct {
	*pixivTestRepository
	calls             *[]string
	getErr, removeErr error
}

func (r *managementRepository) GetPixiv(ctx context.Context, id int64) (accountpixiv.Account, error) {
	*r.calls = append(*r.calls, "get")
	if r.getErr != nil {
		return accountpixiv.Account{}, r.getErr
	}
	return r.pixivTestRepository.GetPixiv(ctx, id)
}
func (r *managementRepository) RemovePixiv(ctx context.Context, id int64) error {
	*r.calls = append(*r.calls, "remove")
	if r.removeErr != nil {
		return r.removeErr
	}
	return r.pixivTestRepository.RemovePixiv(ctx, id)
}

type managementDefaults struct {
	*pixivTestDefaults
	calls *[]string
}

func (d *managementDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	*d.calls = append(*d.calls, "read")
	return d.pixivTestDefaults.ReadPixivDefaultUserID()
}
func (d *managementDefaults) SetPixivDefaultUserID(id int64) error {
	*d.calls = append(*d.calls, "set")
	return d.pixivTestDefaults.SetPixivDefaultUserID(id)
}
func (d *managementDefaults) ClearPixivDefaultUserID() error {
	*d.calls = append(*d.calls, "clear")
	return d.pixivTestDefaults.ClearPixivDefaultUserID()
}

func TestMigrationAccountManagementFreezesMutationOrderAndFailureState(t *testing.T) {
	for _, tc := range []struct {
		name, operation string
		explicit        int64
		failure         string
		calls           []string
		want            string
		selected        int64
		remains         bool
	}{
		{"use", "use", 7, "", []string{"get", "set"}, "", 42, true},
		{"use-get-failure", "use", 7, "get", []string{"get"}, "get failed", 7, true},
		{"use-set-failure", "use", 7, "set", []string{"get", "set"}, "set failed", 7, true},
		{"remove-explicit", "remove", 42, "", []string{"read", "clear", "remove"}, "", 0, false},
		{"remove-implicit", "remove", 0, "", []string{"read", "remove"}, "", 0, false},
		{"remove-other", "remove", 7, "", []string{"read", "remove"}, "", 7, false},
		{"remove-read-failure", "remove", 42, "read", []string{"read"}, "read pixiv default account: read failed", 42, true},
		{"remove-clear-failure", "remove", 42, "clear", []string{"read", "clear"}, "clear pixiv default account: clear failed", 42, true},
		{"remove-delete-failure-no-rollback", "remove", 42, "remove", []string{"read", "clear", "remove"}, "remove failed", 0, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			calls := []string{}
			repo := &managementRepository{pixivTestRepository: newPixivTestRepository(pixivAccountFixture(42, "synthetic", "synthetic-token", 1, true)), calls: &calls}
			defaults := &managementDefaults{pixivTestDefaults: &pixivTestDefaults{userID: tc.explicit, ok: tc.explicit != 0}, calls: &calls}
			failure := errors.New(tc.failure + " failed")
			switch tc.failure {
			case "get":
				repo.getErr = failure
			case "remove":
				repo.removeErr = failure
			case "read":
				defaults.readErr = failure
			case "set":
				defaults.setErr = failure
			case "clear":
				defaults.clearErr = failure
			}
			service := accountpixiv.NewService(repo, defaults)
			var err error
			if tc.operation == "use" {
				err = service.UseAccount(context.Background(), 42)
			} else {
				err = service.RemoveAccount(context.Background(), 42)
			}
			if tc.want == "" {
				require.NoError(t, err)
			} else {
				require.EqualError(t, err, tc.want)
			}
			require.Equal(t, tc.calls, calls)
			require.Equal(t, tc.selected, defaults.userID)
			require.Equal(t, tc.selected != 0, defaults.ok)
			_, remains := repo.accounts[42]
			require.Equal(t, tc.remains, remains)
		})
	}
}

func TestMigrationAccountManagementMissingAccountKeepsUseDefaultButClearsMatchingRemove(t *testing.T) {
	repo := newPixivTestRepository(pixivAccountFixture(42, "synthetic", "synthetic-token", 1, true))
	defaults := &pixivTestDefaults{userID: 7, ok: true}
	service := accountpixiv.NewService(repo, defaults)
	require.ErrorIs(t, service.UseAccount(context.Background(), 999), accountpixiv.ErrNotFound)
	require.Equal(t, int64(7), defaults.userID)
	require.ErrorIs(t, service.RemoveAccount(context.Background(), 999), accountpixiv.ErrNotFound)
	require.Equal(t, int64(7), defaults.userID)
	defaults.userID = 999
	require.ErrorIs(t, service.RemoveAccount(context.Background(), 999), accountpixiv.ErrNotFound)
	require.False(t, defaults.ok)
	require.Len(t, repo.accounts, 1)
}
