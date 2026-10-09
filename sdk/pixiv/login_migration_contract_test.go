package pixiv

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/stretchr/testify/require"
)

type loginMigrationTransport struct {
	calls   atomic.Int32
	body    string
	status  int
	inspect func(*http.Request)
}

func (m *loginMigrationTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	m.calls.Add(1)
	if m.inspect != nil {
		m.inspect(r)
	}
	return &http.Response{StatusCode: m.status, Header: http.Header{"Content-Type": []string{"application/json"}}, Body: io.NopCloser(strings.NewReader(m.body)), Request: r}, nil
}
func loginMigrationSession(t *testing.T, body string, status int) (*LoginSession, *loginMigrationTransport) {
	t.Helper()
	m := &loginMigrationTransport{body: body, status: status}
	s, e := BeginLogin(LoginOptions{HTTPClient: &http.Client{Transport: m}})
	require.NoError(t, e)
	return s, m
}

const loginMigrationSuccess = `{"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600,"user":{"id":"42","name":"fixture"}}`

func TestMigrationLoginCallbackContract(t *testing.T) {
	s, m := loginMigrationSession(t, loginMigrationSuccess, 200)
	state := s.state.state
	cases := []struct {
		input, code string
		accepts     bool
	}{
		{"pixiv://account/login?code=x", "x", true},
		{"https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x", "x", true},
		{"https://example.invalid/callback?code=x&state=" + state, "x", true},
		{"custom:opaque?code=x&state=" + state, "x", true},
		{"custom://%C3%A9/cb?code=x&state=" + state, "x", true},
		{"custom://[fe80::1%25eth0]/cb?code=x&state=" + state, "x", true},
		{"custom://%61/cb?code=x&state=" + state, "", false},
		{"custom://[fe80::1%25]/cb?code=x&state=" + state, "", false},
		{"custom://[fe80::1%25é]/cb?code=x&state=" + state, "x", true},
		{"custom://[fe80::1%25%C3%A9]/cb?code=x&state=" + state, "", false},
		{"custom://[bad]/cb?code=x&state=" + state, "", false},
		{"pixiv://account/login?code=x&code=y", "x", true},
		{"pixiv://account/login?code=%20x%20&state=%20" + state + "%20", "x", true},
		{"pixiv://user@account/login?code=x&state=" + state, "x", true},
		{"pixiv://account:443/login?code=x&state=" + state, "x", true},
		{"pixiv://account/%6cogin?code=x", "x", true},
		{"https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x", "", false},
		{"custom://bad\\host/cb?code=x&state=" + state, "", false},
		{"https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback", "https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback", false},
		{"custom://bad\\host/cb", "custom://bad\\host/cb", false},
		{" bare-code ", "bare-code", false}, {"%zz", "%zz", false}, {"https://[bad", "https://[bad", false},
		{"", "", false}, {"relative?code=x", "", false}, {"pixiv://account/login?code=x&state=wrong", "", false},
		{"https://example.invalid/callback?code=x", "", false}, {"pixiv://account/login?code=%zz", "", false},
		{"pixiv://account/login?code=x;y", "", false}, {"pixiv://account/login?code=&code=x", "", false},
	}
	for _, c := range cases {
		t.Run(c.input, func(t *testing.T) {
			code, err := loginCode(c.input, state)
			if c.code == "" {
				require.Error(t, err)
			} else {
				require.NoError(t, err)
				require.Equal(t, c.code, code)
			}
			require.Equal(t, c.accepts, s.AcceptsCallbackURL(c.input))
			require.Equal(t, c.accepts, s.AcceptsCallbackURL(c.input))
		})
	}
	require.Zero(t, m.calls.Load())
	var nilSession *LoginSession
	require.Empty(t, nilSession.AuthorizationURL())
	require.False(t, nilSession.AcceptsCallbackURL("x"))
	_, err := nilSession.Complete(context.Background(), "x")
	require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
	nilSession.CloseIdleConnections()
	zero := LoginSession{}
	require.Empty(t, zero.AuthorizationURL())
	_, err = zero.Complete(context.Background(), "x")
	require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
}
func TestMigrationLoginOneShotRequestAndRedaction(t *testing.T) {
	s, m := loginMigrationSession(t, loginMigrationSuccess, 200)
	u, err := url.Parse(s.AuthorizationURL())
	require.NoError(t, err)
	require.Equal(t, "https://app-api.pixiv.net/web/v1/login", u.Scheme+"://"+u.Host+u.Path)
	require.Len(t, u.Query(), 4)
	require.Len(t, s.state.verifier, 86)
	require.Len(t, s.state.state, 43)
	sum := sha256.Sum256([]byte(s.state.verifier))
	require.Equal(t, base64.RawURLEncoding.EncodeToString(sum[:]), u.Query().Get("code_challenge"))
	require.Equal(t, "S256", u.Query().Get("code_challenge_method"))
	require.Equal(t, "pixiv-android", u.Query().Get("client"))
	m.inspect = func(r *http.Request) {
		require.Equal(t, "POST", r.Method)
		require.Equal(t, "https://oauth.secure.pixiv.net/auth/token", r.URL.String())
		require.NoError(t, r.ParseForm())
		require.Len(t, r.PostForm, 7)
		require.Equal(t, "authorization_code", r.Form.Get("grant_type"))
		require.Equal(t, "bare-code", r.Form.Get("code"))
		require.Equal(t, s.state.verifier, r.Form.Get("code_verifier"))
		require.Equal(t, "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback", r.Form.Get("redirect_uri"))
		require.Equal(t, "true", r.Form.Get("include_policy"))
		require.NotEmpty(t, r.Header.Get("User-Agent"))
		require.Empty(t, r.Header.Get("Authorization"))
	}
	_, err = s.Complete(context.Background(), "bad?input")
	require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
	copy := *s
	c, err := copy.Complete(context.Background(), " bare-code ")
	require.NoError(t, err)
	require.Equal(t, int64(42), c.UserID)
	require.Equal(t, "fixture", c.Username)
	require.WithinDuration(t, time.Now().Add(time.Hour), c.ExpiresAt, time.Second)
	_, err = s.Complete(context.Background(), "bare-code")
	require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
	require.EqualValues(t, 1, m.calls.Load())
	require.True(t, s.AcceptsCallbackURL("pixiv://account/login?code=x"))
	for _, verb := range []string{"%v", "%+v", "%#v", "%s", "%q"} {
		out := fmt.Sprintf(verb, s)
		require.NotContains(t, out, s.state.verifier)
		require.NotContains(t, out, s.state.state)
		require.NotContains(t, out, "bare-code")
	}
}
func TestMigrationLoginResponseAndConsumption(t *testing.T) {
	cases := []struct {
		body   string
		status int
		reason sdk.Reason
	}{
		{`{"refresh_token":"r","user":{"id":42}}`, 200, ""},
		{`{"refresh_token":"r","expires_in":-1,"user":{"id":42}}`, 200, ""},
		{`{"refresh_token":"r","user":{"id":42},"response":{}}`, 200, ""},
		{`{"refresh_token":"r","user":{"id":42},"response":{"user":{"id":7}}}`, 200, sdk.MalformedUpstreamResponse},
		{`{"refresh_token":" ","user":{"id":42}}`, 200, sdk.MalformedUpstreamResponse},
		{`null`, 200, sdk.MalformedUpstreamResponse}, {`{}`, 200, sdk.MalformedUpstreamResponse},
		{`{"secret":"fixture-response-secret"}`, 400, sdk.CredentialsExpired}, {`{}`, 401, sdk.CredentialsExpired}, {`{}`, 403, sdk.Forbidden}, {`{}`, 429, sdk.RateLimited}, {`{}`, 503, sdk.UpstreamError},
	}
	for i, c := range cases {
		t.Run(fmt.Sprint(i), func(t *testing.T) {
			s, m := loginMigrationSession(t, c.body, c.status)
			_, err := s.Complete(context.Background(), "fixture-code-secret")
			if c.reason == "" {
				require.NoError(t, err)
			} else {
				require.Equal(t, c.reason, sdk.ReasonOf(err))
				require.NotContains(t, fmt.Sprintf("%+v", err), "fixture-response-secret")
				require.NotContains(t, fmt.Sprintf("%+v", err), "fixture-code-secret")
			}
			_, err = s.Complete(context.Background(), "x")
			require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
			require.EqualValues(t, 1, m.calls.Load())
		})
	}
}
func TestMigrationLoginConcurrentCopies(t *testing.T) {
	s, m := loginMigrationSession(t, loginMigrationSuccess, 200)
	var wg sync.WaitGroup
	var ok atomic.Int32
	for range 16 {
		wg.Go(func() {
			copy := *s
			_, err := copy.Complete(context.Background(), "x")
			if err == nil {
				ok.Add(1)
			} else {
				require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
			}
		})
	}
	wg.Wait()
	require.EqualValues(t, 1, ok.Load())
	require.EqualValues(t, 1, m.calls.Load())
}

type loginMigrationFailureTransport struct {
	cause error
	calls atomic.Int32
}

func (m *loginMigrationFailureTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	m.calls.Add(1)
	if m.cause != nil {
		return nil, m.cause
	}
	<-r.Context().Done()
	return nil, r.Context().Err()
}
func TestMigrationLoginTransportFailureAndCancellationConsumeSession(t *testing.T) {
	for _, cancel := range []bool{false, true} {
		t.Run(fmt.Sprint(cancel), func(t *testing.T) {
			m := &loginMigrationFailureTransport{}
			if !cancel {
				m.cause = fmt.Errorf("fixture-private-transport-secret")
			}
			s, err := BeginLogin(LoginOptions{HTTPClient: &http.Client{Transport: m}})
			require.NoError(t, err)
			ctx, stop := context.WithCancel(context.Background())
			defer stop()
			if cancel {
				stop()
			}
			_, err = s.Complete(ctx, "fixture-code-secret")
			require.Equal(t, sdk.UpstreamUnavailable, sdk.ReasonOf(err))
			require.NotContains(t, fmt.Sprintf("%+v", err), "fixture-private-transport-secret")
			require.NotContains(t, fmt.Sprintf("%+v", err), "fixture-code-secret")
			if cancel {
				require.ErrorIs(t, err, context.Canceled)
			}
			_, err = s.Complete(context.Background(), "x")
			require.Equal(t, sdk.InvalidArgument, sdk.ReasonOf(err))
			require.EqualValues(t, 1, m.calls.Load())
		})
	}
}
func TestMigrationLoginOfficialURLPredicates(t *testing.T) {
	for _, c := range []struct {
		input    string
		expected bool
	}{
		{"https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback", true},
		{"HTTPS://APP-API.PIXIV.NET/web/v1/users/auth/pixiv/callback", true},
		{"https://user@App-Api.Pixiv.Net/web/v1/users/auth/pixiv/callback", true},
		{"https://app-api.pixiv.net/web/v1/users/auth/pixiv/%63allback", true},
		{"https://app-api.pixiv.net:443/web/v1/users/auth/pixiv/callback", false},
		{"https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback/", false},
		{"http://app-api.pixiv.net/web/v1/users/auth/pixiv/callback", false},
		{"https://example.invalid/web/v1/users/auth/pixiv/callback", false},
		{"https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x", false},
		{"https://bad\\host/web/v1/users/auth/pixiv/callback", false},
	} {
		require.Equal(t, c.expected, IsOfficialOAuthCallbackURL(c.input), c.input)
		start := strings.ReplaceAll(strings.ReplaceAll(c.input, "callback", "start"), "%63allback", "%73tart")
		require.Equal(t, c.expected, IsOfficialOAuthStartURL(start), start)
	}
}
func TestMigrationLoginQueryPreservesNonUTF8Bytes(t *testing.T) {
	code, err := loginCode("pixiv://account/login?code=%ff", "unused")
	require.NoError(t, err)
	require.Equal(t, []byte{255}, []byte(code))
}
