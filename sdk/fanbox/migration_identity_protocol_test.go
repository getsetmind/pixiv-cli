package fanbox_test

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"html"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/protocol"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxIdentity = flag.Bool("migration-capture-fanbox-identity-protocol", false, "capture frozen Go FANBOX identity/options/protocol behavior")

const migrationFanboxSessionValue = "synthetic-contract-session"
const migrationFanboxExternalCanary = "synthetic-external-detail"

type migrationFanboxStep struct {
	Status         int         `json:"status"`
	Body           string      `json:"body"`
	Headers        http.Header `json:"headers,omitempty"`
	Location       string      `json:"location,omitempty"`
	ReadError      string      `json:"read_error,omitempty"`
	CloseError     string      `json:"close_error,omitempty"`
	Chunk          int         `json:"chunk,omitempty"`
	NilBody        bool        `json:"nil_body,omitempty"`
	ContentLength  int64       `json:"content_length,omitempty"`
	TransportError string      `json:"transport_error,omitempty"`
	CancelOnRead   bool        `json:"cancel_on_read,omitempty"`
	CancelOnClose  bool        `json:"cancel_on_close,omitempty"`
}

type migrationFanboxInput struct {
	Operation string                      `json:"operation"`
	Cookie    string                      `json:"cookie,omitempty"`
	Proxy     string                      `json:"proxy,omitempty"`
	UserAgent string                      `json:"user_agent,omitempty"`
	Solver    *fanbox.FlareSolverrOptions `json:"solver,omitempty"`
	Client    string                      `json:"client,omitempty"`
	URL       string                      `json:"url,omitempty"`
	Document  string                      `json:"document,omitempty"`
	Context   string                      `json:"context,omitempty"`
	Repeat    int                         `json:"repeat,omitempty"`
	Steps     []migrationFanboxStep       `json:"steps,omitempty"`
	Format    string                      `json:"format,omitempty"`
	User      *fanbox.User                `json:"user,omitempty"`
}

type migrationFanboxCase struct {
	Name        string               `json:"name"`
	Input       migrationFanboxInput `json:"input"`
	Observation map[string]any       `json:"observation"`
}

type migrationFanboxBody struct {
	reader   *strings.Reader
	step     migrationFanboxStep
	cancel   context.CancelFunc
	Injected bool     `json:"injected_body"`
	Reads    int      `json:"go_only_read_calls"`
	Bytes    int      `json:"bytes_read"`
	Closes   int      `json:"close_calls"`
	Events   []string `json:"go_only_events"`
}

func (b *migrationFanboxBody) Read(p []byte) (int, error) {
	b.Reads++
	b.Events = append(b.Events, "read")
	if b.step.CancelOnRead {
		b.cancel()
	}
	if b.step.Chunk > 0 && len(p) > b.step.Chunk {
		p = p[:b.step.Chunk]
	}
	n, err := b.reader.Read(p)
	b.Bytes += n
	if b.step.ReadError != "" && b.reader.Len() == 0 {
		return n, migrationFanboxInjectedError(b.step.ReadError)
	}
	return n, err
}

func (b *migrationFanboxBody) Close() error {
	b.Closes++
	b.Events = append(b.Events, "close")
	if b.step.CancelOnClose {
		b.cancel()
	}
	return migrationFanboxInjectedError(b.step.CloseError)
}

func migrationFanboxInjectedError(kind string) error {
	switch kind {
	case "":
		return nil
	case "eof":
		return io.EOF
	case "canceled":
		return fmt.Errorf("%s: %w", migrationFanboxExternalCanary, context.Canceled)
	case "deadline":
		return fmt.Errorf("%s: %w", migrationFanboxExternalCanary, context.DeadlineExceeded)
	default:
		return errors.New(migrationFanboxExternalCanary + " https://example.invalid/private?token=synthetic-secret")
	}
}

type migrationFanboxTransport struct {
	t         *testing.T
	steps     []migrationFanboxStep
	requests  []map[string]any
	bodies    []*migrationFanboxBody
	cancel    context.CancelFunc
	idleCalls int
}

func (r *migrationFanboxTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	index := len(r.requests)
	headers := req.Header.Clone()
	headers.Set("Cookie", migrationFanboxCookieProjection(headers.Get("Cookie")))
	if headers.Get("Cookie") == "" {
		headers.Del("Cookie")
	}
	_, deadline := req.Context().Deadline()
	r.requests = append(r.requests, map[string]any{
		"method": req.Method, "url": req.URL.String(), "headers": headers,
		"context_value":    req.Context().Value(migrationFanboxContextKey{}),
		"context_canceled": req.Context().Err() != nil, "context_has_deadline": deadline,
		"request_has_body": req.Body != nil,
	})
	if index >= len(r.steps) {
		r.t.Fatalf("unexpected transport call %d to %s", index, req.URL)
	}
	step := r.steps[index]
	if step.TransportError == "nil_response" {
		return nil, nil
	}
	if step.TransportError != "" {
		return nil, migrationFanboxInjectedError(step.TransportError)
	}
	body := &migrationFanboxBody{reader: strings.NewReader(step.Body), step: step, cancel: r.cancel, Events: []string{}, Injected: !step.NilBody}
	r.bodies = append(r.bodies, body)
	header := step.Headers.Clone()
	if header == nil {
		header = make(http.Header)
	}
	if step.Location != "" {
		header.Set("Location", step.Location)
	}
	response := &http.Response{StatusCode: step.Status, Header: header, Body: body, Request: req, ContentLength: step.ContentLength}
	if step.NilBody {
		response.Body = nil
	}
	return response, nil
}

func (r *migrationFanboxTransport) CloseIdleConnections() { r.idleCalls++ }

type migrationFanboxJar struct{ calls int }

func (j *migrationFanboxJar) Cookies(*url.URL) []*http.Cookie {
	j.calls++
	return []*http.Cookie{{Name: "injected_jar", Value: "synthetic-jar"}}
}
func (j *migrationFanboxJar) SetCookies(*url.URL, []*http.Cookie) { j.calls++ }

type migrationFanboxContextKey struct{}

func migrationFanboxCookieProjection(value string) string {
	return strings.ReplaceAll(value, migrationFanboxSessionValue, "[SYNTHETIC_SESSION]")
}

func migrationFanboxErrorObservation(err error) map[string]any {
	result := map[string]any{"message": "", "reason": "", "canceled": false, "deadline": false,
		"not_authenticated": false, "forbidden": false, "challenge": false, "invalid_option": false,
		"go_only_error_tree": []string{}}
	if err == nil {
		return result
	}
	result["message"] = err.Error()
	result["reason"] = string(sdk.ReasonOf(err))
	result["canceled"] = errors.Is(err, context.Canceled)
	result["deadline"] = errors.Is(err, context.DeadlineExceeded)
	result["not_authenticated"] = errors.Is(err, protocol.ErrNotAuthenticated)
	result["forbidden"] = errors.Is(err, protocol.ErrForbidden)
	result["challenge"] = errors.Is(err, protocol.ErrChallenge)
	result["invalid_option"] = errors.Is(err, protocol.ErrInvalidOption)
	var classified *sdk.Error
	if errors.As(err, &classified) {
		result["sdk"] = map[string]any{"product": classified.Product, "operation": classified.Operation, "reason": classified.Reason,
			"detail": classified.Detail, "http_status": classified.HTTPStatus, "transport": classified.Transport,
			"retry_safe": classified.Retry.Safe, "retry_has_after": classified.Retry.HasAfter,
			"go_only_matches_reason": errors.Is(err, sdk.NewError("ignored", "ignored", classified.Reason))}
	}
	var tree []string
	var visit func(error)
	visit = func(current error) {
		if current == nil {
			return
		}
		tree = append(tree, fmt.Sprintf("%T: %s", current, current.Error()))
		if multiple, ok := current.(interface{ Unwrap() []error }); ok {
			for _, child := range multiple.Unwrap() {
				visit(child)
			}
		} else {
			visit(errors.Unwrap(current))
		}
	}
	visit(err)
	result["go_only_error_tree"] = tree
	if strings.Contains(fmt.Sprint(result), migrationFanboxExternalCanary) {
		panic("external error details reached a frozen observation")
	}
	return result
}

func migrationFanboxIdentityDTO(value protocol.Identity) fanbox.UserDTO {
	return fanbox.ToUserDTO(fanbox.User{UserID: value.UserID, DisplayName: value.DisplayName, CreatorID: value.CreatorID, CreatorStatus: value.CreatorStatus, IsCreator: value.IsCreator})
}

func migrationFanboxObserve(t *testing.T, input migrationFanboxInput) map[string]any {
	t.Helper()
	result := map[string]any{}
	if input.Operation == "parse_identity" {
		identity, err := protocol.ParseIdentityMetadataHTML([]byte(input.Document))
		result["dto"] = migrationFanboxIdentityDTO(identity)
		result["error"] = migrationFanboxErrorObservation(err)
		return result
	}
	if input.Operation == "normalize_cookie" {
		cookie, err := protocol.NormalizeCookieHeader(input.Cookie)
		result["cookie_projection"] = migrationFanboxCookieProjection(cookie)
		result["redacted"] = protocol.RedactCookieHeader(input.Cookie)
		result["error"] = migrationFanboxErrorObservation(err)
		return result
	}
	if input.Operation == "user_dto" {
		result["dto"] = fanbox.ToUserDTO(*input.User)
		return result
	}
	if input.Operation == "credentials_format" {
		credentials := fanbox.SessionCredentials{FANBOXSESSID: input.Cookie}
		result["go_only_format"] = fmt.Sprintf(input.Format, credentials)
		result["string"] = credentials.String()
		result["go_string"] = credentials.GoString()
		encoded, err := json.Marshal(credentials)
		if err != nil {
			t.Fatal(err)
		}
		result["json"] = json.RawMessage(encoded)
		if strings.Contains(result["go_only_format"].(string), input.Cookie) {
			t.Fatal("SessionCredentials formatting exposed synthetic credential")
		}
		return result
	}
	if input.Operation == "validate_api_url" || input.Operation == "validate_media_url" {
		var err error
		if input.Operation == "validate_api_url" {
			err = protocol.ValidateAPIURL(input.URL)
		} else {
			err = protocol.ValidateMediaURL(input.URL)
		}
		result["error"] = migrationFanboxErrorObservation(err)
		return result
	}
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationFanboxContextKey{}, "synthetic-context"))
	defer cancel()
	if input.Context == "canceled" {
		cancel()
	}
	if input.Context == "deadline" {
		var stop context.CancelFunc
		ctx, stop = context.WithDeadline(ctx, time.Unix(1, 0))
		defer stop()
	}
	if input.Context == "future_deadline" {
		var stop context.CancelFunc
		ctx, stop = context.WithDeadline(ctx, time.Date(2099, 1, 1, 0, 0, 0, 0, time.UTC))
		defer stop()
	}
	transport := &migrationFanboxTransport{t: t, steps: input.Steps, requests: []map[string]any{}, bodies: []*migrationFanboxBody{}, cancel: cancel}
	jar := &migrationFanboxJar{}
	redirectCalls := 0
	client := &http.Client{Transport: transport, Jar: jar, CheckRedirect: func(*http.Request, []*http.Request) error {
		redirectCalls++
		return errors.New("synthetic caller redirect rejection")
	}}
	if input.Client == "implicit" {
		client = &http.Client{}
	}
	if input.Client == "native" {
		client = nil
	}
	options := fanbox.Options{HTTPClient: client, ProxyURL: input.Proxy, UserAgent: input.UserAgent, FlareSolverr: input.Solver}
	var sdkClient *fanbox.Client
	var session *protocol.Session
	var err error
	cookie := input.Cookie
	if cookie == "" && input.Operation != "sdk_open_empty" && input.Operation != "session_empty" {
		cookie = migrationFanboxSessionValue
	}
	isSDK := strings.HasPrefix(input.Operation, "sdk_")
	if isSDK {
		if input.Operation == "sdk_open" || input.Operation == "sdk_open_empty" {
			sdkClient, err = fanbox.Open(fanbox.SessionCredentials{FANBOXSESSID: cookie})
		} else {
			sdkClient, err = fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: cookie}, options)
		}
	} else {
		header := cookie
		if input.Operation != "session_empty" && !strings.Contains(header, "=") {
			header = "FANBOXSESSID=" + header
		}
		protocolOptions := protocol.SessionOptions{HTTPClient: client, ProxyURL: input.Proxy, UserAgent: input.UserAgent}
		if input.Solver != nil {
			protocolOptions.FlareSolverr = &protocol.FlareSolverrOptions{URL: input.Solver.URL, ProxyURL: input.Solver.ProxyURL}
		}
		if input.Operation == "session_functional" {
			functional := []protocol.Option{nil, protocol.WithHTTPClient(client), protocol.WithProxyURL(input.Proxy), protocol.WithUserAgent("overridden-agent"), protocol.WithUserAgent(input.UserAgent)}
			if protocolOptions.FlareSolverr != nil {
				functional = append(functional, protocol.WithFlareSolverr(*protocolOptions.FlareSolverr))
			}
			session, err = protocol.NewSession(header, functional...)
		} else if input.Operation == "session_http_constructor" {
			session, err = protocol.NewSessionWithHTTPClient(header, client)
		} else {
			session, err = protocol.NewSessionWithOptions(header, protocolOptions)
		}
	}
	result["constructed"] = sdkClient != nil || session != nil
	result["constructor_error"] = migrationFanboxErrorObservation(err)
	var outcomes []map[string]any
	if err == nil {
		repeat := input.Repeat
		if repeat == 0 {
			repeat = 1
		}
		for n := 0; n < repeat; n++ {
			outcome := map[string]any{}
			switch input.Operation {
			case "sdk_current_user":
				var user fanbox.User
				user, err = sdkClient.CurrentUser(ctx, fanbox.CurrentUserRequest{})
				outcome["dto"] = fanbox.ToUserDTO(user)
			case "sdk_validate_session":
				err = sdkClient.ValidateSession(ctx)
			case "session_current_user":
				var identity protocol.Identity
				identity, err = session.CurrentUser(ctx)
				outcome["dto"] = migrationFanboxIdentityDTO(identity)
			case "session_get_json":
				var target any
				err = session.GetJSON(ctx, input.URL, &target)
				outcome["json"] = target
			case "session_format":
				outcome["go_only_format"] = fmt.Sprintf(input.Format, session)
				outcome["synthetic_cookie_visible"] = strings.Contains(outcome["go_only_format"].(string), migrationFanboxSessionValue)
			}
			outcome["error"] = migrationFanboxErrorObservation(err)
			outcomes = append(outcomes, outcome)
		}
		if sdkClient != nil {
			sdkClient.CloseIdleConnections()
			sdkClient.CloseIdleConnections()
		} else {
			session.CloseIdleConnections()
			session.CloseIdleConnections()
		}
	}
	result["outcomes"] = outcomes
	result["requests"] = transport.requests
	result["bodies"] = transport.bodies
	result["close_idle_calls"] = transport.idleCalls
	result["jar_calls"] = jar.calls
	result["redirect_callback_calls"] = redirectCalls
	result["go_only_injected_client_unchanged"] = client == nil || input.Client == "implicit" || (client.Transport == transport && client.Jar == jar && client.CheckRedirect != nil && client.Timeout == 0)
	return result
}

func migrationFanboxMetadata(user string) string {
	return `<meta name="metadata" content="` + html.EscapeString(`{"context":{"user":`+user+`}}`) + `">`
}

func migrationFanboxCases() []migrationFanboxCase {
	var rows []migrationFanboxCase
	add := func(name string, in migrationFanboxInput) {
		rows = append(rows, migrationFanboxCase{Name: name, Input: in})
	}
	good := migrationFanboxMetadata(`{"userId":42,"name":" synthetic user ","creatorId":" artist ","creatorStatus":" active ","isCreator":true}`)
	ok := migrationFanboxStep{Status: 200, Body: good}
	for _, verb := range []string{"%v", "%+v", "%#v", "%s", "%q", "%x", "%d", "%20.3s", "%-20v", "%T"} {
		add("credentials_format/"+verb, migrationFanboxInput{Operation: "credentials_format", Cookie: migrationFanboxSessionValue, Format: verb})
	}
	for _, verb := range []string{"%v", "%+v", "%#v", "%s", "%q"} {
		add("session_format/"+verb, migrationFanboxInput{Operation: "session_format", Format: verb})
	}
	add("dto/zero", migrationFanboxInput{Operation: "user_dto", User: &fanbox.User{}})
	add("dto/all_fields", migrationFanboxInput{Operation: "user_dto", User: &fanbox.User{UserID: 9223372036854775807, DisplayName: "名前 & <>", CreatorID: "id", CreatorStatus: "status", IsCreator: true}})
	for i, cookie := range []string{
		"", " ", "FANBOXSESSID=" + migrationFanboxSessionValue, " FANBOXSESSID = " + migrationFanboxSessionValue + " ; other = value ",
		"FANBOXSESSID=a=b", "FANBOXSESSID=\"quoted\"", "FANBOXSESSID=a,b", "FANBOXSESSID=a\\b", "FANBOXSESSID=a b",
		"FANBOXSESSID=日本語", "FANBOXSESSID=a\x00b", "FANBOXSESSID=a\tb", "FANBOXSESSID=a\nb", "FANBOXSESSID=a\rb",
		"FANBOXSESSID=a;", "FANBOXSESSID=a;;other=b", "FANBOXSESSID=a; other=", "FANBOXSESSID=a; bad name=b",
		"FANBOXSESSID=a; other=b; other=c", "FANBOXSESSID=a; FANBOXSESSID=b", "fanboxsessid=a", "other=b", "FANBOXSESSID",
		"FANBOXSESSID=a; fanboxsessid=b", "FANBOXSESSID=a; !#$%&'*+-.^_`|~=b", "FANBOXSESSID=a; other=a:b/c?d=e",
	} {
		add(fmt.Sprintf("normalize_cookie/%02d", i), migrationFanboxInput{Operation: "normalize_cookie", Cookie: cookie})
	}
	constructors := []migrationFanboxInput{
		{Operation: "sdk_open", Client: "native"}, {Operation: "sdk_open_empty", Client: "native"}, {Operation: "sdk_open_with", Client: "native"},
		{Operation: "sdk_open_with", Client: "implicit"}, {Operation: "sdk_open_with"}, {Operation: "sdk_open_with", Cookie: " " + migrationFanboxSessionValue + " "},
		{Operation: "sdk_open_with", Cookie: migrationFanboxSessionValue + "; extra=value"}, {Operation: "sdk_open_with", Cookie: "FANBOXSESSID=" + migrationFanboxSessionValue},
		{Operation: "sdk_open_with", Cookie: migrationFanboxSessionValue + "; FANBOXSESSID=duplicate"}, {Operation: "sdk_open_with", Cookie: "bad\nvalue"},
		{Operation: "session_empty"}, {Operation: "session_http_constructor"}, {Operation: "session_functional", UserAgent: "functional-agent"},
		{Operation: "sdk_open_with", Cookie: "bad\nvalue", UserAgent: "bad\nagent", Proxy: "invalid", Client: "implicit"},
		{Operation: "sdk_open_with", Cookie: "bad\nvalue", Proxy: "invalid", Client: "implicit"},
		{Operation: "sdk_open_with", Cookie: "bad\nvalue", Client: "implicit"},
	}
	for i, in := range constructors {
		add(fmt.Sprintf("constructor/%02d", i), in)
	}
	for i, agent := range []string{"", " custom-agent ", "\t", "agent日本語", "agent\x7f", "bad\ragent", "bad\nagent", "bad\x00agent"} {
		add(fmt.Sprintf("user_agent/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", UserAgent: agent, Steps: []migrationFanboxStep{ok}})
	}
	for i, proxy := range []string{"", " \t ", "http://proxy.example", "https://proxy.example:8443", "HTTP://proxy.example", "http://proxy.example/path?x=1#fragment", "http://[::1]:8080", "http://proxy.example:", "socks5://proxy.example", "http://user:pass@proxy.example", "http:///no-host", "http://proxy.example/%zz", " http://proxy.example", "https://proxy.example?", "http://proxy.example:bad"} {
		add(fmt.Sprintf("native_proxy/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", Proxy: proxy, Steps: []migrationFanboxStep{ok}})
	}
	for i, solver := range []fanbox.FlareSolverrOptions{
		{URL: "http://solver.example"}, {URL: "https://solver.example/"}, {URL: "http://solver.example:8191", ProxyURL: "http://proxy.example/"},
		{URL: "http://solver.example", ProxyURL: "socks4://proxy.example:1080"}, {URL: "http://solver.example", ProxyURL: "socks5://proxy.example"},
		{URL: ""}, {URL: "http://solver.example/prefix"}, {URL: "http://user:pass@solver.example"}, {URL: "http://solver.example?"},
		{URL: "http://solver.example?x=1"}, {URL: "http://solver.example#fragment"}, {URL: "ftp://solver.example"}, {URL: "http:///nohost"},
		{URL: "http://solver.example", ProxyURL: "https://proxy.example"}, {URL: "http://solver.example", ProxyURL: "socks5://user:pass@proxy.example"},
		{URL: "http://solver.example", ProxyURL: "http://proxy.example/prefix"}, {URL: "http://solver.example", ProxyURL: "http://proxy.example?"},
		{URL: "http://solver.example", ProxyURL: "http://proxy.example#fragment"}, {URL: "http://solver.example", ProxyURL: " "},
	} {
		add(fmt.Sprintf("solver_options/%02d", i), migrationFanboxInput{Operation: "sdk_open_with", Solver: &solver})
	}
	for _, sample := range []struct{ name, service, upstream string }{
		{"service_encoded_root_slash", "http://solver.example/%2F", ""},
		{"service_double_slash", "http://solver.example//", ""},
		{"service_encoded_path_separator", "http://solver.example/%2fnext", ""},
		{"service_empty_fragment", "http://solver.example#", ""},
		{"service_encoded_question_path", "http://solver.example/%3F", ""},
		{"service_encoded_nul_path", "http://solver.example/%00", ""},
		{"service_raw_control", "http://solver.example/\n", ""},
		{"service_uppercase_scheme", "HTTP://solver.example/", ""},
		{"upstream_encoded_root_slash", "http://solver.example", "http://proxy.example/%2F"},
		{"upstream_double_slash", "http://solver.example", "http://proxy.example//"},
		{"upstream_encoded_nul_path", "http://solver.example", "http://proxy.example/%00"},
		{"upstream_raw_control", "http://solver.example", "http://proxy.example/\t"},
		{"upstream_empty_fragment", "http://solver.example", "http://proxy.example#"},
	} {
		add("solver_options/"+sample.name, migrationFanboxInput{Operation: "sdk_open_with", Solver: &fanbox.FlareSolverrOptions{URL: sample.service, ProxyURL: sample.upstream}})
	}
	for _, sample := range []struct{ name, proxy string }{
		{"encoded_root_slash", "http://proxy.example/%2F"},
		{"encoded_nul_path", "http://proxy.example/%00"},
		{"raw_control", "http://proxy.example/\n"},
		{"double_slash", "http://proxy.example//"},
	} {
		add("native_proxy/"+sample.name, migrationFanboxInput{Operation: "sdk_current_user", Proxy: sample.proxy, Steps: []migrationFanboxStep{ok}})
	}
	add("constructor/session_functional_native", migrationFanboxInput{Operation: "session_functional", Client: "native"})
	add("current_user/malformed_page", migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{{Status: 200, Body: "<html>no metadata</html>"}}})
	for _, contextKind := range []string{"canceled", "deadline"} {
		add("current_user/context_failure/"+contextKind, migrationFanboxInput{Operation: "sdk_current_user", Context: contextKind, Steps: []migrationFanboxStep{{TransportError: "raw"}}})
	}
	for _, sample := range []struct {
		name string
		step migrationFanboxStep
	}{
		{"malformed_page", migrationFanboxStep{Status: 200, Body: "<html>no metadata</html>"}},
		{"challenge", migrationFanboxStep{Status: 403, Body: "cf-chl"}},
		{"read_and_close_failure", migrationFanboxStep{Status: 200, Body: good, ReadError: "raw", CloseError: "raw"}},
		{"transport_canceled", migrationFanboxStep{TransportError: "canceled"}},
	} {
		add("validate_session/"+sample.name, migrationFanboxInput{Operation: "sdk_validate_session", Steps: []migrationFanboxStep{sample.step}})
	}
	for _, contextKind := range []string{"", "canceled", "deadline", "future_deadline"} {
		add("current_user/context/"+contextKind, migrationFanboxInput{Operation: "sdk_current_user", Context: contextKind, Steps: []migrationFanboxStep{ok}})
	}
	add("current_user/not_cached", migrationFanboxInput{Operation: "sdk_current_user", Repeat: 2, Steps: []migrationFanboxStep{ok, {Status: 200, Body: migrationFanboxMetadata(`{"userId":43,"name":"second"}`)}}})
	add("validate_session/not_cached", migrationFanboxInput{Operation: "sdk_validate_session", Repeat: 2, Steps: []migrationFanboxStep{ok, {Status: 401, Body: "synthetic denied"}}})
	add("current_user/full_cookie_normalization", migrationFanboxInput{Operation: "sdk_current_user", Cookie: " " + migrationFanboxSessionValue + " ; extra = value ", Steps: []migrationFanboxStep{ok}})
	add("protocol/current_user", migrationFanboxInput{Operation: "session_current_user", Steps: []migrationFanboxStep{ok}})
	baseUser := `{"userId":42,"name":"n"}`
	users := []string{
		baseUser, `null`, `{}`, `{"userId":" +42 ","name":" n "}`, `{"userId":"042","name":"n"}`, `{"userId":9223372036854775807,"name":"n"}`,
		`{"userId":9223372036854775808,"name":"n"}`, `{"userId":0,"name":"n"}`, `{"userId":-1,"name":"n"}`, `{"userId":42.0,"name":"n"}`, `{"userId":4.2e1,"name":"n"}`,
		`{"userId":true,"name":"n"}`, `{"userId":{},"name":"n"}`, `{"userId":[],"name":"n"}`, `{"userId":null,"name":"n"}`,
		`{"userId":"0x2a","name":"n"}`, `{"userId":"４２","name":"n"}`, `{"userId":"1_000","name":"n"}`,
		`{"userId":42}`, `{"userId":42,"name":null}`, `{"userId":42,"name":"　 \t "}`, `{"userId":42,"name":7}`, `{"userId":42,"name":true}`,
		`{"userId":42,"name":"<&\" 日本語","creatorId":" artist "}`, `{"userId":42,"name":"n","creatorId":null}`,
		`{"userId":42,"name":"n","creatorId":123}`, `{"userId":42,"name":"n","creatorId":1.5}`, `{"userId":42,"name":"n","creatorId":true}`,
		`{"userId":42,"name":"n","creatorId":{}}`, `{"userId":42,"name":"n","creatorId":[]}`,
		`{"userId":42,"name":"n","creatorStatus":true}`, `{"userId":42,"name":"n","creatorStatus":false}`,
		`{"userId":42,"name":"n","creatorStatus":null}`, `{"userId":42,"name":"n","creatorStatus":7}`, `{"userId":42,"name":"n","creatorStatus":{}}`,
		`{"userId":42,"name":"n","creatorStatus":" active ","isCreator":true}`, `{"userId":42,"name":"n","creatorId":"artist","isCreator":false}`,
		`{"userId":42,"name":"n","creatorStatus":true,"isCreator":false}`, `{"userId":42,"name":"n","creatorStatus":false,"isCreator":true}`,
		`{"userId":42,"name":"n","isCreator":null}`, `{"userId":42,"name":"n","isCreator":"true"}`, `{"userId":42,"name":"n","isCreator":1}`,
		`{"USERID":42,"NAME":"n","CREATORID":"artist"}`, `{"userId":1,"userId":2,"name":"first","name":"second"}`,
		`{"userId":42,"name":"n","unknown":{"nested":[1,true,null]}}`, `[]`, `42`, `"n"`,
	}
	for i, user := range users {
		add(fmt.Sprintf("identity/user/%02d", i), migrationFanboxInput{Operation: "parse_identity", Document: migrationFanboxMetadata(user)})
	}
	for i, document := range []string{
		"", "<html>no metadata</html>", `<meta name="metadata" content="not json">`, `<meta name="metadata" content="null">`,
		`<meta name="metadata" content="[]">`, `<meta name="metadata" content="{}">`, good + good,
		`<META CONTENT='{"context":{"user":{"userId":"42","name":"n"}}}' NAME=" METADATA "/>`,
		`<meta name="metadata" content=" ">` + good, `<meta name="metadata" content="bad">` + good,
		`<!-- ` + good + ` -->`, `<script>` + good + `</script>`, `<metadata name="metadata" content="{}">`,
		`<meta name="other" name="metadata" content='{"context":{"user":{"userId":42,"name":"n"}}}'>`,
		`<meta name="metadata" name="other" content='{"context":{"user":{"userId":42,"name":"n"}}}'>`,
		`<meta name="metadata" content='{"context":{"user":{"userId":42,"name":"n"}}} {}'>`,
		`<meta name="metadata" content='{"context":{"user":{"userId":42,"name":"n"}}} null'>`,
		`<meta name="metadata" content='{"CONTEXT":{"USER":{"USERID":42,"NAME":"n"}}}'>`,
		`<meta name="metadata" content='{"context":{"user":{"userId":42,"name":"n"}},"context":{"user":{"creatorId":"artist"}}}'>`,
	} {
		add(fmt.Sprintf("identity/document/%02d", i), migrationFanboxInput{Operation: "parse_identity", Document: document})
	}
	for i, target := range []string{
		"https://api.fanbox.cc/post.info?postId=1", "https://www.fanbox.cc/", "https://fanbox.cc/", "https://creator.fanbox.cc/path",
		"https://API.FANBOX.CC:443/a%2Fb?raw=%zz#fragment", "HTTPS://api.fanbox.cc/a", "https://api.fanbox.cc:8443/a", "https://api.fanbox.cc./a",
		"http://api.fanbox.cc/a", "https://user:pass@api.fanbox.cc/a", "https://evilfanbox.cc/a", "https://fanbox.cc.evil.example/a",
		"https://i.pximg.net/a", "https://downloads.fanbox.cc/a", "https://api.fanbox.cc/%zz", "//api.fanbox.cc/a", "https:///a", "https://api.fanbox.cc:bad/a",
	} {
		if i == 0 || i == 8 {
			add(fmt.Sprintf("api_url/%02d", i), migrationFanboxInput{Operation: "validate_api_url", URL: target})
		}
		add(fmt.Sprintf("api_request/%02d", i), migrationFanboxInput{Operation: "session_get_json", URL: target, Steps: []migrationFanboxStep{{Status: 200, Body: `{"ok":true}`}}})
	}
	for i, target := range []string{"https://downloads.fanbox.cc/a", "https://i.pximg.net/a", "https://cdn.pximg.net/a", "https://pximg.net/a", "https://fanbox.pixiv.net/a", "https://cdn.fanbox.pixiv.net/a", "https://pixiv.net/a", "https://evilpximg.net/a", "https://pximg.net.evil.example/a", "https://other.fanbox.cc/a", "http://i.pximg.net/a", "https://user@i.pximg.net/a"} {
		add(fmt.Sprintf("media_url_validation_only/%02d", i), migrationFanboxInput{Operation: "validate_media_url", URL: target})
	}
	for _, status := range []int{200, 204, 304, 400, 401, 403, 404, 408, 429, 500, 503} {
		step := migrationFanboxStep{Status: status, Body: good}
		add(fmt.Sprintf("current_user/status/%d", status), migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{step}})
	}
	for i, step := range []migrationFanboxStep{
		{Status: 403, Body: `{"challenge":"business field"}`}, {Status: 403, Body: "prefix CF-CHL suffix"}, {Status: 403, Body: "cf_chl", Chunk: 1},
		{Status: 403, Body: strings.Repeat("x", 32767) + "CF-CHL" + strings.Repeat("x", 65536)},
		{Status: 403, Body: "denied", Headers: http.Header{"Cf-Mitigated": {"CHALLENGE"}}},
		{Status: 403, Body: "denied", Headers: http.Header{"Content-Type": {" text/html; charset=UTF-8"}, "Server": {"CloudFlare"}}},
		{Status: 403, Body: "denied", Headers: http.Header{"Content-Type": {"text/html"}, "Cf-Ray": {"synthetic-ray"}}},
		{Status: 403, Body: "denied", Headers: http.Header{"Content-Type": {"application/json"}, "Server": {"cloudflare"}}},
		{Status: 403, Body: "cf-chl", ReadError: "raw"}, {Status: 403, Body: "cf-chl", CloseError: "raw"}, {Status: 403, Body: "cf-chl", ReadError: "raw", CloseError: "raw"},
		{Status: 401, Body: "ignored", ReadError: "raw"}, {Status: 401, Body: "ignored", CloseError: "raw"},
	} {
		add(fmt.Sprintf("classification/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{step}})
	}
	for i, step := range []migrationFanboxStep{
		{Status: 200, Body: good, ReadError: "raw"}, {Status: 200, Body: good, CloseError: "raw"}, {Status: 200, Body: good, ReadError: "raw", CloseError: "raw"},
		{Status: 200, Body: good, ReadError: "canceled"}, {Status: 200, Body: good, ReadError: "deadline"}, {Status: 200, Body: good, CancelOnRead: true, ReadError: "raw"},
		{Status: 200, Body: good, CancelOnClose: true, CloseError: "raw"}, {Status: 200, Body: good, ReadError: "eof"},
		{Status: 200, NilBody: true}, {Status: 200, NilBody: true, ContentLength: 1},
		{TransportError: "raw"}, {TransportError: "canceled"}, {TransportError: "deadline"}, {TransportError: "nil_response"},
	} {
		add(fmt.Sprintf("identity_body/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{step}})
	}
	for i, body := range []string{`{"ok":true}`, `{"ok":true} trailing`, `{"ok":true} {"second":true}`, `null`, `[]`, ``, `{`, `1`, `1.5`, `9007199254740993`} {
		add(fmt.Sprintf("json_decode/%02d", i), migrationFanboxInput{Operation: "session_get_json", URL: "https://api.fanbox.cc/fixture", Steps: []migrationFanboxStep{{Status: 200, Body: body}}})
	}
	for i, step := range []migrationFanboxStep{{Status: 200, Body: `{"ok":true}`, ReadError: "raw"}, {Status: 200, Body: `{`, ReadError: "raw"}, {Status: 200, Body: `{`, CloseError: "raw"}, {Status: 200, Body: `{`, ReadError: "raw", CloseError: "raw"}, {Status: 200, Body: `{"ok":true}`, CloseError: "raw"}, {Status: 304, Body: `{"ok":true}`}} {
		add(fmt.Sprintf("json_body/%02d", i), migrationFanboxInput{Operation: "session_get_json", URL: "https://api.fanbox.cc/fixture", Steps: []migrationFanboxStep{step}})
	}
	for _, status := range []int{301, 302, 303, 307, 308} {
		add(fmt.Sprintf("redirect/status/%d", status), migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{{Status: status, Body: "redirect ignored", Location: "/next"}, ok}})
	}
	for i, location := range []string{"", " ", "/%zz", "http://www.fanbox.cc/next", "https://user:pass@www.fanbox.cc/next", "https://external.example/next", "https://www.fanbox.cc/", "https://WWW.FANBOX.CC/next", "//api.fanbox.cc/next", "https://creator.fanbox.cc/next", "#fragment", "?page=2"} {
		steps := []migrationFanboxStep{{Status: 302, Body: "redirect body", Location: location}}
		if i == 7 || i == 8 || i == 9 || i == 11 {
			steps = append(steps, ok)
		}
		if i == 10 {
			steps = append(steps, migrationFanboxStep{Status: 302, Location: "#fragment", Body: "second redirect"})
		}
		add(fmt.Sprintf("redirect/location/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", Steps: steps})
	}
	for i, location := range []string{"/next", "/%zz", "https://external.example/next", ""} {
		add(fmt.Sprintf("redirect/close_error/%02d", i), migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{{Status: 302, Body: "redirect body", Location: location, CloseError: "raw"}}})
	}
	long := []migrationFanboxStep{}
	for i := 0; i < 12; i++ {
		long = append(long, migrationFanboxStep{Status: 302, Location: fmt.Sprintf("/hop%d", i), Body: "redirect body"})
	}
	long = append(long, ok)
	add("redirect/no_ten_hop_cap", migrationFanboxInput{Operation: "sdk_current_user", Steps: long})
	return rows
}

func TestMigrationFanboxIdentityProtocolFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	sources := map[string]string{
		"sdk/fanbox/fanbox.go":                          "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542",
		"sdk/fanbox/request.go":                         "deea7897788f1013297fc3cf03a7b71ff0d893335d6434d1908feb2245ece263",
		"sdk/fanbox/ops.go":                             "868b3b68de07d7638af5f9dd3ab1be8f0c9751f222c05c7776cde092052a7c8d",
		"sdk/fanbox/errors.go":                          "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909",
		"sdk/fanbox/dto.go":                             "860f6d3f18d083526681cf86112dbbdb7faa382e1f9cac3e35b543f56c2df505",
		"sdk/fanbox/models.go":                          "1dd3928aa6752c51f114c678eef3b0a2b4784a275ec80fcdbbd61006407e4623",
		"internal/services/fanbox/protocol/identity.go": "10ba481b43e3ec7d0bdaa2defbbd629c4b5bb2aa169c5bff99c8e756de8143d9",
		"internal/services/fanbox/protocol/cookie.go":   "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9",
		"internal/services/fanbox/protocol/protocol.go": "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3",
		"internal/services/fanbox/protocol/solver.go":   "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea",
		"sdk/error.go":                                  "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
		"go.mod":                                        "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum":                                        "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("frozen Go toolchain required, got %s", runtime.Version())
	}
	for path, want := range sources {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(data)); got != want {
			t.Fatalf("frozen source changed: %s: %s", path, got)
		}
		original, err := exec.Command("git", "-C", root, "show", "4b4426487ef18bed276706daec385e0d0a6979f9:"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, data) {
			t.Fatalf("source differs from frozen commit: %s", path)
		}
	}
	stdlib := map[string]string{"net/http/client.go": "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9", "net/http/request.go": "c3257079994b4e4f74f01cef72508919983ba31f91fcf599d348e9d52cec1540", "net/http/response.go": "0b32a0b4ee51e00e3410764f3b2ffe1163dccb9cad548b097f47db85893a2f44"}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(data)); got != want {
			t.Fatalf("Go standard library changed: %s", path)
		}
	}
	dependencies := []map[string]string{
		{"module": "github.com/bogdanfinn/fhttp", "version": "v0.6.8", "zip_sha256": "51f74cb0f96633810038f94773bdbf0356c98a23d9de9eada3008521ae97f3cb", "sum": "h1:LiQyHOY3i0QoxxNB7nq27/nGNNbtPj0fuBPozhR7Ws4="},
		{"module": "github.com/bogdanfinn/tls-client", "version": "v1.15.1", "zip_sha256": "01aa0c1f09bc29397b97090a9fb03677ce22491f8a1cdd0a0dc7cdbce16cf031", "sum": "h1:KiFAlED55DJ8Fcocn+/1nX6PrDFcttIHAf/GDkV6KN8="},
		{"module": "github.com/bogdanfinn/utls", "version": "v1.7.7-barnius", "zip_sha256": "6e21ca668a463b41d3a4f92449e69872dbb0056b1d41b14b760e1237894203ef", "sum": "h1:OuJ497cc7F3yKNVHRsYPQdGggmk5x6+V5ZlrCR7fOLU="},
		{"module": "golang.org/x/net", "version": "v0.48.0", "zip_sha256": "cf5206797e66bbe72fc13542d53a57d069a563cebb6d045c07a870eb4fd888c9", "sum": "h1:zyQRTTrjc33Lhh0fBgT/H3oZq9WuvRR5gPC70xpDiQU="},
	}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("GOMODCACHE must identify the verified offline cache")
	}
	goSum, err := os.ReadFile(filepath.Join(root, "go.sum"))
	if err != nil {
		t.Fatal(err)
	}
	for _, dep := range dependencies {
		module, version := dep["module"], dep["version"]
		path := filepath.Join(cache, "cache", "download", module, "@v", version+".zip")
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(data)); got != dep["zip_sha256"] {
			t.Fatalf("official dependency archive changed: %s", path)
		}
		if !bytes.Contains(goSum, []byte(module+" "+version+" "+dep["sum"]+"\n")) {
			t.Fatalf("dependency sum absent: %s", module)
		}
		archive, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
		if err != nil {
			t.Fatal(err)
		}
		for _, file := range archive.File {
			if file.FileInfo().IsDir() {
				continue
			}
			reader, err := file.Open()
			if err != nil {
				t.Fatal(err)
			}
			original, readErr := io.ReadAll(reader)
			closeErr := reader.Close()
			if readErr != nil || closeErr != nil {
				t.Fatalf("archive read: %v / %v", readErr, closeErr)
			}
			extracted, err := os.ReadFile(filepath.Join(cache, filepath.FromSlash(file.Name)))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(original, extracted) {
				t.Fatalf("dependency source differs from official archive: %s", file.Name)
			}
		}
	}
	rows := migrationFanboxCases()
	seen := map[string]bool{}
	for i := range rows {
		if seen[rows[i].Name] {
			t.Fatalf("duplicate case %s", rows[i].Name)
		}
		seen[rows[i].Name] = true
		t.Run(rows[i].Name, func(t *testing.T) { rows[i].Observation = migrationFanboxObserve(t, rows[i].Input) })
	}
	var nilClient *fanbox.Client
	nilClient.CloseIdleConnections()
	var nilSession *protocol.Session
	nilSession.CloseIdleConnections()
	contract := map[string]any{"source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "source_sha256": sources, "go_version": runtime.Version(), "go_stdlib_sha256": stdlib, "dependencies": dependencies, "cases": rows,
		"go_only_projections": []string{"go_only_format: Go fmt syntax and type spellings; credential redaction remains semantic", "go_only_error_tree: concrete Go error types and unwrap/join topology; message/classification/context identity remain semantic", "go_only_matches_reason: Go errors.Is with sdk.Error reason-only sentinel", "go_only_read_calls and go_only_events: injected reader calls, sizes and ordering under Go io/net/http/json", "go_only_injected_client_unchanged: Go pointer/dependency ownership comparison"},
		"limitations":         []string{"new capture after filesystem replacement; no lost fixture bytes, historical row count or prototype passes reused", "owned synthetic inputs and genuine injected *http.Client/RoundTripper only; no external accounts, services, browser, media reads or trust changes", "media host policy validation only; HEAD, resource opening/reopening, content families, native TLS/HTTP2/Accept/decompression, timeout/pacing and platform lifecycle are later families", "solver option validation only; solver recovery/control/state is a separate contract family", "nil Client/Session CloseIdleConnections are exercised; constructor-created native idle closure does not prove physical socket cleanup", "synthetic session value is projected to [SYNTHETIC_SESSION] in observed Cookie headers; no real credential enters fixture"}}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-identity-protocol.json")
	if *migrationCaptureFanboxIdentity {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatalf("FANBOX identity/options/protocol differs from frozen Go fixture; recapture only after reviewing actual Go behavior")
	}
	families := map[string]int{}
	for _, row := range rows {
		families[strings.Split(row.Name, "/")[0]]++
	}
	names := []string{}
	for name := range families {
		names = append(names, name)
	}
	sort.Strings(names)
	t.Logf("fresh frozen Go %s: %d cases; fixture bytes=%d sha256=%x", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
	for _, name := range names {
		t.Logf("%s: %d", name, families[name])
	}
}
