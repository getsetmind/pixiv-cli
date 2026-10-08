package database

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	sdkpixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var updateAccountOpen = flag.Bool("migration-update-account-open", false, "capture account refresh and persistence contracts")

type migrationAccountDefaults struct{ id int64 }

func (d migrationAccountDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	return d.id, d.id != 0, nil
}
func (migrationAccountDefaults) SetPixivDefaultUserID(int64) error { panic("unexpected default write") }
func (migrationAccountDefaults) ClearPixivDefaultUserID() error    { panic("unexpected default clear") }

type migrationAccountOpenTransport func(*http.Request) (*http.Response, error)

func (f migrationAccountOpenTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	return f(r)
}

func TestMigrationAccountOpenPersistsRotationBeforeContent(t *testing.T) {
	type row struct {
		Name         string `json:"name"`
		Message      string `json:"message"`
		OAuthCalls   int    `json:"oauth_calls"`
		ContentCalls int    `json:"content_calls"`
		Token        string `json:"token"`
		Revision     int64  `json:"revision"`
		Username     string `json:"username"`
		Canceled     bool   `json:"canceled"`
	}
	var rows []row
	for _, name := range []string{"explicit", "fallback", "configured", "empty", "missing", "configured_missing", "identity_mismatch", "revision_conflict", "no_rotated_token", "zero_expiry", "refresh_canceled"} {
		db, err := Open(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		ctx, cancel := context.WithCancel(context.Background())
		if name != "empty" {
			if err := db.SavePixivCredential(ctx, account.New(42, "stored-name", []byte("fixture-refresh"))); err != nil {
				t.Fatal(err)
			}
		}
		r := row{Name: name}
		transport := migrationAccountOpenTransport(func(req *http.Request) (*http.Response, error) {
			body := `{}`
			if req.URL.Host == "oauth.secure.pixiv.net" {
				r.OAuthCalls++
				if name == "refresh_canceled" {
					cancel()
					<-req.Context().Done()
					return nil, req.Context().Err()
				}
				uid := int64(42)
				if name == "identity_mismatch" {
					uid = 43
				}
				if name == "revision_conflict" {
					if err := db.RotatePixivCredentials(ctx, 42, 1, []byte("fixture-concurrent")); err != nil {
						t.Fatal(err)
					}
				}
				token := "fixture-rotated"
				if name == "no_rotated_token" {
					token = ""
				}
				expiry := 3600
				if name == "zero_expiry" {
					expiry = 0
				}
				payload, _ := json.Marshal(map[string]any{"access_token": "fixture-access", "refresh_token": token, "expires_in": expiry, "user": map[string]any{"id": uid, "name": "oauth-name"}})
				body = string(payload)
			} else {
				r.ContentCalls++
				stored, err := db.GetPixiv(ctx, 42)
				if err != nil {
					t.Fatal(err)
				}
				if stored.CredentialRevision != 2 || req.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatal("content requested before rotated credentials were persisted")
				}
			}
			return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
		})
		defaultID := int64(0)
		if name == "configured" {
			defaultID = 42
		}
		if name == "configured_missing" {
			defaultID = 99
		}
		service := account.NewService(db, migrationAccountDefaults{defaultID})
		options := sdkpixiv.Options{HTTPClient: &http.Client{Transport: transport}}
		var client *sdkpixiv.Client
		if name == "fallback" || name == "configured" || name == "empty" || name == "configured_missing" {
			client, err = service.OpenClientWith(ctx, options)
		} else {
			id := int64(42)
			if name == "missing" {
				id = 99
			}
			client, err = service.OpenAccountClientWith(ctx, id, options)
		}
		if err != nil {
			r.Canceled = errors.Is(err, context.Canceled)
			r.Message = err.Error()
			if client != nil {
				t.Fatal("failed persistence returned client")
			}
		} else {
			_, _ = client.Artwork(ctx, sdkpixiv.ArtworkRequest{ArtworkID: 1})
			client.CloseIdleConnections()
		}
		stored, getErr := db.GetPixiv(context.Background(), 42)
		if getErr == nil {
			r.Token, r.Revision, r.Username = string(stored.RefreshTokenCopy()), stored.CredentialRevision, stored.Username
		}
		rows = append(rows, r)
		db.Close()
		cancel()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "account-open.json")
	if *updateAccountOpen {
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
		t.Fatal("account open differs from fixed Go reference")
	}
}
