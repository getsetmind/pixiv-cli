package pixiv

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
	"time"
)

var migrationUpdateOAuthRefresh = flag.Bool("migration-update-oauth-refresh", false, "capture public OAuth refresh contracts from the fixed Go reference")

type migrationOAuthTransport func(*http.Request) (*http.Response, error)

func (f migrationOAuthTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	return f(req)
}

func TestMigrationOAuthRefreshPreservesOpaqueInputAndResponseFallback(t *testing.T) {
	type row struct {
		Name          string `json:"name"`
		Input         string `json:"input"`
		Body          string `json:"body"`
		Calls         int    `json:"calls"`
		Message       string `json:"message"`
		Access        string `json:"access"`
		Refresh       string `json:"refresh"`
		UID           int64  `json:"uid"`
		Username      string `json:"username"`
		ExpirySeconds int64  `json:"expiry_seconds"`
	}
	valid := `{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42,"name":"fixture-name"}}`
	var rows []row
	for _, input := range []string{"", " \t\n", "fixture-refresh", "  fixture-refresh  ", "opaque=value", "opaque token", "opaque;part", "opaque=value;part", "Cookie: fixture=value", "PHPSESSID=value", "refresh_token=value", "x=y; z=w", "session = value", "x=; y=z"} {
		rows = append(rows, row{Name: "input:" + input, Input: input, Body: valid})
	}
	for _, body := range []string{
		`{"access_token":"fixture-access","user":{"id":"42"}}`,
		`{"access_token":"fixture-access","expires_in":-1,"user":{"id":42}}`,
		`{"access_token":"fixture-access","user":{"id":0}}`,
		`{"refresh_token":"fixture-rotated","user":{"id":42}}`,
		`{"access_token":"fixture-access","user":{"id":42},"response":{}}`,
		`{"access_token":"fixture-access","user":{"id":42},"response":{"user":{"id":43}}}`,
		`{"response":{"access_token":"fixture-nested","user":{"id":"43","name":"nested"}}}`,
		`{"access_token":null,"user":{"id":42}}`,
		`{"access_token":"fixture-access","user":{"id":42.5}}`,
		`{"access_token":"fixture-access","user":{"id":"+42"}}`,
		`{"access_token":"fixture-access","user":{"id":" 42"}}`,
		`{"access_token":"fixture-access","user":null}`, "null", "[]", "", "not-json",
	} {
		rows = append(rows, row{Name: "response:" + body, Input: "fixture-refresh", Body: body})
	}
	for i := range rows {
		r := &rows[i]
		transport := migrationOAuthTransport(func(req *http.Request) (*http.Response, error) {
			r.Calls++
			if req.Method != "POST" || req.URL.String() != "https://oauth.secure.pixiv.net/auth/token" {
				t.Fatalf("unexpected OAuth route")
			}
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			if len(req.PostForm) != 5 || req.PostForm.Get("grant_type") != "refresh_token" || req.PostForm.Get("refresh_token") != strings.TrimSpace(r.Input) || req.PostForm.Get("include_policy") != "true" || req.PostForm.Get("client_id") == "" || req.PostForm.Get("client_secret") == "" {
				t.Fatal("unexpected OAuth form")
			}
			if req.Header.Get("Content-Type") != "application/x-www-form-urlencoded" || req.Header.Get("Authorization") != "" {
				t.Fatal("unexpected OAuth headers")
			}
			return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(r.Body)), Request: req}, nil
		})
		before := time.Now()
		client, credentials, err := OpenWith(context.Background(), r.Input, Options{HTTPClient: &http.Client{Transport: transport}})
		after := time.Now()
		if err != nil {
			r.Message = err.Error()
			if client != nil {
				t.Fatal("failed refresh returned a client")
			}
			continue
		}
		client.CloseIdleConnections()
		r.Access, r.Refresh, r.UID, r.Username = credentials.accessToken, credentials.refreshToken, credentials.UserID, credentials.Username
		if !credentials.ExpiresAt.IsZero() {
			if credentials.ExpiresAt.Before(before.Add(3600*time.Second)) || credentials.ExpiresAt.After(after.Add(3600*time.Second)) {
				t.Fatal("unexpected token expiry")
			}
			r.ExpirySeconds = 3600
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "oauth-refresh.json")
	if *migrationUpdateOAuthRefresh {
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
		t.Fatal("OAuth refresh differs from the frozen Go reference")
	}
}
