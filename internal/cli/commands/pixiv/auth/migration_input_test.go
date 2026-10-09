package auth

import (
	"errors"
	"net"
	"net/url"
	"strings"
	"testing"
)

func TestMigrationLoginInputClassification(t *testing.T) {
	cases := []struct {
		input, code, message string
		calls                int
	}{
		{" \t\n", "", "sign-in result cannot be empty", 0},
		{" bare-code ", "bare-code", "", 0},
		{"code?state=bad", "code?state=bad", "", 0},
		{"code#fragment", "code#fragment", "", 0},
		{"code&field=value", "code&field=value", "", 0},
		{"pixiv:account/login?code=one", "pixiv:account/login?code=one", "", 0},
		{"https://example.test/callback", "", "sign-in address did not include required details", 0},
		{"https://example.test/callback?", "", "sign-in address did not include required details", 0},
		{"https://example.test/callback#?code=one", "", "sign-in address did not include required details", 0},
		{"https://example.test/callback?code=%zz", "https://example.test/callback?code=%zz", "", 1},
		{"/callback?code=one", "/callback?code=one", "", 1},
		{"//example.test/callback?code=one", "//example.test/callback?code=one", "", 1},
		{"https://example.test/%zz?code=one", "", "invalid sign-in address", 0},
		{"https://example.test/callback?code=one#%zz", "", "invalid sign-in address", 0},
		{"https://example.test/callback?code=one\x00", "", "invalid sign-in address", 0},
	}
	for _, tc := range cases {
		t.Run(tc.input, func(t *testing.T) {
			calls := 0
			got := LoginCodeFromInput(tc.input, func(raw string) bool {
				calls++
				if raw != strings.TrimSpace(tc.input) {
					t.Fatal("untrimmed input")
				}
				return true
			})
			message := ""
			if got.Err != nil {
				message = got.Err.Error()
			}
			if got.Code != tc.code || message != tc.message || calls != tc.calls {
				t.Fatalf("got code=%q error=%q calls=%d", got.Code, message, calls)
			}
		})
	}
	for _, accepter := range []CallbackURLAccepter{nil, func(string) bool { return false }} {
		got := LoginCodeFromInput("pixiv://account/login?code=secret", accepter)
		if got.Err == nil || got.Err.Error() != "sign-in address does not match this login session" {
			t.Fatalf("rejection=%+v", got)
		}
	}
}

func TestMigrationLoginRelayOrderAndQuery(t *testing.T) {
	start := "https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=current"
	bridge := "https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(start)
	cases := []struct {
		input, target, message string
		relay                  bool
	}{
		{" " + bridge + " ", start, "", true},
		{strings.Replace(bridge, "accounts.pixiv.net", "ACCOUNTS.PIXIV.NET", 1), start, "", true},
		{strings.Replace(bridge, "accounts.pixiv.net", "accountſ.pixiv.net", 1), start, "", true},
		{bridge + "&return_to=bad", start, "", true},
		{"https://accounts.pixiv.net/post-redirect?return_to=%zz&return_to=" + url.QueryEscape(start), start, "", true},
		{"https://accounts.pixiv.net/post-redirect?return_to=bad&return_to=" + url.QueryEscape(start), "", "invalid Pixiv authorization relay URL", true},
		{"https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(start) + ";ignored=yes", "", "invalid Pixiv authorization relay URL", true},
		{"https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(strings.Replace(start, "current", "stale", 1)), strings.Replace(start, "current", "stale", 1), "Pixiv authorization relay URL does not match this login attempt", true},
		{"https://accounts.pixiv.net:443/post-redirect?return_to=" + url.QueryEscape(start), "", "", false},
		{"https://user@accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(start), start, "", true},
	}
	for _, tc := range cases {
		t.Run(tc.input, func(t *testing.T) {
			target, ok := PixivPostRedirectReturnTo(tc.input)
			if target != tc.target || ok != (tc.target != "") {
				t.Fatalf("target=%q ok=%v", target, ok)
			}
			accepted := 0
			accept := func(string) bool { accepted++; return true }
			got := classifyLoginInput(tc.input, accept, "current")
			message := ""
			if got.Err != nil {
				message = got.Err.Error()
			}
			if got.Relayed != tc.relay || message != tc.message {
				t.Fatalf("classification=%+v", got)
			}
			if tc.relay && accepted != 0 {
				t.Fatal("relay reached callback accepter")
			}
			opened := 0
			got = LoginInputFromText(tc.input, accept, "current", func(raw string) error {
				opened++
				if raw != strings.TrimSpace(tc.input) {
					t.Fatal("opener received target instead of bridge")
				}
				return nil
			})
			wantOpened := 0
			if tc.relay && tc.message == "" {
				wantOpened = 1
			}
			if opened != wantOpened {
				t.Fatalf("open calls=%d", opened)
			}
		})
	}
	got := LoginInputFromText(bridge, nil, "current", nil)
	if got.Err == nil || got.Err.Error() != "browser opener is not configured" || got.RelayURL != "" {
		t.Fatalf("nil opener=%+v", got)
	}
	openerError := errors.New("opener failure")
	got = LoginInputFromText(bridge, nil, "current", func(string) error { return openerError })
	if got.Err == nil || got.Err.Error() != "could not open Pixiv authorization relay URL: opener failure" || got.RelayURL != "" {
		t.Fatalf("failed opener=%+v", got)
	}
	if !errors.Is(got.Err, openerError) {
		t.Fatal("opener error source must be retained")
	}
	if !PixivAuthStartMatchesChallenge("invalid\x00", "") {
		t.Fatal("empty challenge must bypass parsing")
	}
	if !PixivAuthStartMatchesChallenge(start+"&code_challenge=stale", "current") {
		t.Fatal("first challenge wins")
	}
	if PixivAuthStartMatchesChallenge(start+";bad=x", "current") {
		t.Fatal("semicolon query must be dropped")
	}
	if pixivLoginChallenge(start+"%20") != "current" {
		t.Fatal("login challenge must trim decoded value")
	}
}

func TestMigrationLoginAddressAndSSHHint(t *testing.T) {
	cases := []struct{ addr, validation, hint string }{
		{"", "--addr cannot be empty", "parse login listener address: missing port in address"},
		{"localhoſt:80", "", "ssh -N -L 80:localhoſt:80 USER@SERVER"},
		{"127.0.0.1:0", "", "ssh -N -L 0:127.0.0.1:0 USER@SERVER"},
		{"LOCALHOST:", "", "login listener address is incomplete"},
		{"127.1.2.3:abc", "", "ssh -N -L abc:127.1.2.3:abc USER@SERVER"},
		{"[::1]:41871", "", "ssh -N -L 41871:::1:41871 USER@SERVER"},
		{"[::ffff:127.0.0.1]:80", "", "ssh -N -L 80:::ffff:127.0.0.1:80 USER@SERVER"},
		{"[::1%lo]:80", "--addr must bind to a loopback address, got \"[::1%lo]:80\"", "ssh -N -L 80:::1%lo:80 USER@SERVER"},
		{":80", "--addr must bind to a loopback address, got \":80\"", "login listener address is incomplete"},
		{"0.0.0.0:80", "--addr must bind to a loopback address, got \"0.0.0.0:80\"", "ssh -N -L 80:0.0.0.0:80 USER@SERVER"},
		{"bad", "invalid --addr \"bad\": address bad: missing port in address", "parse login listener address: address bad: missing port in address"},
		{"::1:80", "invalid --addr \"::1:80\": address ::1:80: too many colons in address", "parse login listener address: address ::1:80: too many colons in address"},
		{"[::1:80", "invalid --addr \"[::1:80\": address [::1:80: missing ']' in address", "parse login listener address: address [::1:80: missing ']' in address"},
	}
	for _, tc := range cases {
		t.Run(tc.addr, func(t *testing.T) {
			message := ""
			if err := validateLoginAddr(tc.addr); err != nil {
				message = err.Error()
			}
			if message != tc.validation {
				t.Fatalf("validation=%q", message)
			}
			hint, err := LoginSSHTunnelCommand(tc.addr)
			if err != nil {
				hint = err.Error()
			}
			if hint != tc.hint {
				t.Fatalf("hint=%q", hint)
			}
		})
	}
	_, sshErr := LoginSSHTunnelCommand("bad")
	for _, err := range []error{validateLoginAddr("bad"), sshErr} {
		var source *net.AddrError
		if !errors.As(err, &source) || source.Addr != "bad" || source.Err != "missing port in address" {
			t.Fatalf("address source=%v", err)
		}
	}
	if err := validateLoginAddr(" \t"); err == nil || err.Error() != "--addr cannot be empty" {
		t.Fatal("empty address")
	}
}

func TestMigrationLoginBrowserCallbackPredicate(t *testing.T) {
	for _, input := range []string{"pixiv://account/login?code=one", "PIXIV://ACCOUNT/login", "https://user@app-api.pixiv.net/web/v1/users/auth/pixiv/callback"} {
		parsed, err := url.Parse(input)
		if err != nil || !IsBrowserCallbackURL(parsed) {
			t.Fatalf("valid callback=%q err=%v", input, err)
		}
	}
	for _, input := range []string{"pixiv://account/Login", "https://app-api.pixiv.net:443/web/v1/users/auth/pixiv/callback", "http://app-api.pixiv.net/web/v1/users/auth/pixiv/callback", "http://127.0.0.1:80/callback"} {
		parsed, err := url.Parse(input)
		if err != nil || IsBrowserCallbackURL(parsed) {
			t.Fatalf("invalid callback=%q err=%v", input, err)
		}
	}
}

func TestMigrationLoginInputGoByteStringEvidence(t *testing.T) {
	got := LoginCodeFromInput("code\xff", nil)
	if got.Code != "code\xff" || got.Err != nil {
		t.Fatalf("raw byte code=%+v", got)
	}
	if !PixivAuthStartMatchesChallenge("https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=%FF", "\xff") {
		t.Fatal("Go query retains non-UTF8 decoded byte")
	}
}
