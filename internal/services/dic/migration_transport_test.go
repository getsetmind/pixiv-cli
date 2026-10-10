package dic_test

import (
	"compress/gzip"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

type migrationDictionaryHTTPRequest struct {
	Method          string      `json:"method"`
	URL             string      `json:"url"`
	RequestURI      string      `json:"request_uri"`
	Host            string      `json:"host"`
	Headers         http.Header `json:"headers"`
	BodyNil         bool        `json:"body_nil"`
	ContextCanceled bool        `json:"context_canceled"`
	ContextDeadline bool        `json:"context_deadline"`
}

type migrationDictionaryHTTPResult struct {
	Body    string                    `json:"body"`
	BodyNil bool                      `json:"body_nil"`
	Status  int                       `json:"status"`
	Error   *migrationDictionaryError `json:"error"`
}

type migrationDictionaryTransportRow struct {
	Name                  string                           `json:"name"`
	Mode                  string                           `json:"mode"`
	Context               string                           `json:"context"`
	Client                string                           `json:"client"`
	RawURL                string                           `json:"raw_url"`
	Accept                string                           `json:"accept"`
	UserAgent             string                           `json:"user_agent"`
	Repeat                int                              `json:"repeat"`
	Response              migrationDictionaryResponse      `json:"response"`
	ReadError             string                           `json:"read_error"`
	CloseError            bool                             `json:"close_error"`
	RedirectStatus        int                              `json:"redirect_status"`
	CrossHost             bool                             `json:"cross_host"`
	Requests              []migrationDictionaryHTTPRequest `json:"requests"`
	Results               []migrationDictionaryHTTPResult  `json:"results"`
	BodyReads             int                              `json:"body_reads"`
	BodyCloses            int                              `json:"body_closes"`
	IdleCloses            int                              `json:"idle_closes"`
	CallerClientUnchanged bool                             `json:"caller_client_unchanged"`
}

type migrationDictionaryReadCloser struct {
	reader     *strings.Reader
	row        *migrationDictionaryTransportRow
	failed     bool
	closed     bool
	closeError bool
}

func (body *migrationDictionaryReadCloser) Read(buffer []byte) (int, error) {
	body.row.BodyReads++
	if body.row.ReadError != "" && !body.failed {
		body.failed = true
		count, _ := body.reader.Read(buffer)
		if body.row.ReadError == "unexpected-eof" {
			return count, io.ErrUnexpectedEOF
		}
		return count, migrationDictionaryCause
	}
	return body.reader.Read(buffer)
}

func (body *migrationDictionaryReadCloser) Close() error {
	if body.closed {
		return errors.New("dictionary response body closed twice")
	}
	body.closed = true
	body.row.BodyCloses++
	if body.closeError {
		return migrationDictionaryCause
	}
	return nil
}

type migrationDictionaryRoundTripper struct {
	row *migrationDictionaryTransportRow
	t   *testing.T
}

func migrationDictionaryObserveHTTPRequest(request *http.Request) migrationDictionaryHTTPRequest {
	return migrationDictionaryHTTPRequest{
		Method: request.Method, URL: request.URL.String(), RequestURI: request.URL.RequestURI(), Host: request.Host,
		Headers: request.Header.Clone(), BodyNil: request.Body == nil,
		ContextCanceled: errors.Is(request.Context().Err(), context.Canceled), ContextDeadline: errors.Is(request.Context().Err(), context.DeadlineExceeded),
	}
}

func (transport *migrationDictionaryRoundTripper) RoundTrip(request *http.Request) (*http.Response, error) {
	transport.row.Requests = append(transport.row.Requests, migrationDictionaryObserveHTTPRequest(request))
	var err error
	switch transport.row.Response.Error {
	case "":
	case "fixture":
		err = migrationDictionaryCause
	case "context":
		err = request.Context().Err()
		if err == nil {
			transport.t.Fatal("HTTP context response requires an ended context")
		}
	default:
		transport.t.Fatalf("unknown HTTP response error %q", transport.row.Response.Error)
	}
	if err != nil {
		return nil, err
	}
	return &http.Response{
		StatusCode: transport.row.Response.Status, Header: http.Header{}, Request: request,
		Body: &migrationDictionaryReadCloser{reader: strings.NewReader(transport.row.Response.Body), row: transport.row, closeError: transport.row.CloseError},
	}, nil
}

func (transport *migrationDictionaryRoundTripper) CloseIdleConnections() { transport.row.IdleCloses++ }

func migrationDictionaryTransportRows() []migrationDictionaryTransportRow {
	rows := []migrationDictionaryTransportRow{}
	add := func(name, mode string) *migrationDictionaryTransportRow {
		rows = append(rows, migrationDictionaryTransportRow{
			Name: name, Mode: mode, Context: "background", Client: "supplied", RawURL: "https://dic.pixiv.net/_api/get_article/title%2Fpart?lang=ja",
			Accept: "application/json", Repeat: 1, Response: migrationDictionaryResponse{Body: "owned dictionary response", Status: 200},
		})
		return &rows[len(rows)-1]
	}
	add("synthetic-default-user-agent-json", "synthetic")
	add("synthetic-default-user-agent-html", "synthetic").Accept = "text/html"
	add("synthetic-empty-accept", "synthetic").Accept = ""
	add("synthetic-custom-user-agent", "synthetic").UserAgent = "fixture dictionary/1.0"
	add("synthetic-client-reuse-and-body-close", "synthetic").Repeat = 2
	add("synthetic-non-success-body-status", "synthetic").Response.Status = 404
	add("synthetic-read-error-discards-prefix-preserves-status", "synthetic").ReadError = "fixture"
	row := add("synthetic-404-read-error-is-transport-failure", "synthetic")
	row.Response.Status, row.ReadError = 404, "unexpected-eof"
	add("synthetic-close-error-ignored", "synthetic").CloseError = true
	add("synthetic-roundtrip-error-source", "synthetic").Response.Error = "fixture"
	add("synthetic-nil-context-before-roundtrip", "synthetic").Context = "nil"
	add("synthetic-invalid-url-before-roundtrip", "synthetic").RawURL = ":invalid"
	add("synthetic-invalid-accept-before-roundtrip", "synthetic").Accept = "text/html\nX-Fixture: bad"
	add("synthetic-invalid-user-agent-before-roundtrip", "synthetic").UserAgent = "fixture\nX-Fixture: bad"
	for _, client := range []string{"nil", "zero"} {
		row := add("synthetic-unconfigured-"+client, "synthetic")
		row.Client, row.Context = client, "nil"
	}
	for _, state := range []string{"canceled", "deadline"} {
		add("synthetic-ready-response-despite-"+state, "synthetic").Context = state
		row := add("synthetic-context-aware-response-"+state, "synthetic")
		row.Context, row.Response.Error = state, "context"
	}
	for _, accept := range []string{"application/json", "text/html", ""} {
		row := add("loopback-default-headers-"+fmt.Sprintf("%x", accept), "loopback")
		row.Accept, row.RawURL, row.Client = accept, "/_api/get_article/title%2Fpart?lang=ja", "default"
	}
	row = add("loopback-custom-user-agent", "loopback")
	row.UserAgent, row.RawURL = "fixture dictionary/1.0", "/custom"
	row = add("loopback-non-success-body-status", "loopback")
	row.Response.Status, row.Response.Body, row.RawURL = 404, "not an HTML document", "/missing"
	row = add("loopback-empty-204", "loopback")
	row.Response.Status, row.Response.Body, row.RawURL = 204, "", "/empty"
	row = add("loopback-gzip-decoded", "loopback")
	row.RawURL, row.Response.Body = "/gzip", "expanded dictionary ✓"
	for _, status := range []int{301, 302, 303, 307, 308} {
		for _, cross := range []bool{false, true} {
			row := add(fmt.Sprintf("loopback-redirect-%d-cross-%t", status, cross), "loopback")
			row.RedirectStatus, row.CrossHost, row.RawURL = status, cross, "/redirect?from=first#fragment"
		}
	}
	add("loopback-redirect-limit", "loopback").RawURL = "/loop?hop=0"
	for _, status := range []int{200, 404} {
		row := add(fmt.Sprintf("loopback-truncated-body-%d", status), "loopback")
		row.RawURL, row.Response.Status, row.Response.Body = "/truncated", status, "prefix"
	}
	for _, state := range []string{"canceled", "deadline"} {
		row := add("loopback-preended-context-"+state, "loopback")
		row.RawURL, row.Context = "/must-not-arrive", state
	}
	return rows
}

func migrationDictionaryRunHTTPTransport(t *testing.T, row migrationDictionaryTransportRow) migrationDictionaryTransportRow {
	t.Helper()
	row.Requests, row.Results = []migrationDictionaryHTTPRequest{}, []migrationDictionaryHTTPResult{}
	ctx, _ := migrationDictionaryContext(t, row.Context)
	if row.Mode == "loopback" {
		return migrationDictionaryRunLoopbackTransport(t, ctx, row)
	}
	roundTripper := &migrationDictionaryRoundTripper{row: &row, t: t}
	client := &http.Client{Transport: roundTripper}
	transport := dic.NewHTTPTransport(dic.HTTPTransportOptions{HTTPClient: client, UserAgent: row.UserAgent})
	switch row.Client {
	case "supplied":
	case "nil":
		transport = nil
	case "zero":
		transport = &dic.HTTPTransport{}
	default:
		t.Fatalf("unexpected synthetic client %q", row.Client)
	}
	for index := 0; index < row.Repeat; index++ {
		body, status, err := transport.Get(ctx, row.RawURL, row.Accept)
		row.Results = append(row.Results, migrationDictionaryHTTPResult{Body: string(body), BodyNil: body == nil, Status: status, Error: migrationDictionaryObserveError(err)})
	}
	row.CallerClientUnchanged = client.Transport == roundTripper && client.Timeout == 0 && client.CheckRedirect == nil && client.Jar == nil
	if row.IdleCloses != 0 || !row.CallerClientUnchanged {
		t.Fatal("dictionary HTTP transport changed its caller-owned client")
	}
	return row
}

func migrationDictionaryRunLoopbackTransport(t *testing.T, ctx context.Context, row migrationDictionaryTransportRow) migrationDictionaryTransportRow {
	t.Helper()
	var mutex sync.Mutex
	var origin, otherOrigin string
	normalize := func(text string) string {
		text = strings.ReplaceAll(text, otherOrigin, "<OTHER_ORIGIN>")
		return strings.ReplaceAll(text, origin, "<ORIGIN>")
	}
	observe := func(request *http.Request) {
		observation := migrationDictionaryObserveHTTPRequest(request)
		observation.URL = normalize(observation.URL)
		if request.Host == strings.TrimPrefix(origin, "http://") {
			observation.Host = "<HOST>"
		} else if request.Host == strings.TrimPrefix(otherOrigin, "http://") {
			observation.Host = "<OTHER_HOST>"
		} else {
			t.Errorf("unexpected owned loopback host %q", request.Host)
		}
		for key, values := range observation.Headers {
			for index := range values {
				values[index] = normalize(values[index])
			}
			observation.Headers[key] = values
		}
		mutex.Lock()
		row.Requests = append(row.Requests, observation)
		mutex.Unlock()
	}
	writeFinal := func(writer http.ResponseWriter, request *http.Request) {
		observe(request)
		writer.WriteHeader(row.Response.Status)
		_, _ = io.WriteString(writer, row.Response.Body)
	}
	otherServer := httptest.NewServer(http.HandlerFunc(writeFinal))
	defer otherServer.Close()
	otherOrigin = strings.Replace(otherServer.URL, "127.0.0.1", "localhost", 1)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		observe(request)
		switch request.URL.Path {
		case "/redirect":
			location := "/final?step=2#ignored"
			if row.CrossHost {
				location = otherOrigin + location
			}
			writer.Header().Set("Location", location)
			writer.WriteHeader(row.RedirectStatus)
			_, _ = io.WriteString(writer, "redirect body must not replace terminal response")
		case "/loop":
			hop := request.URL.Query().Get("hop")
			var number int
			if _, err := fmt.Sscan(hop, &number); err != nil {
				t.Error(err)
			}
			writer.Header().Set("Location", fmt.Sprintf("/loop?hop=%d", number+1))
			writer.WriteHeader(302)
		case "/truncated":
			writer.Header().Set("Content-Length", "99")
			writer.WriteHeader(row.Response.Status)
			_, _ = io.WriteString(writer, row.Response.Body)
		case "/gzip":
			writer.Header().Set("Content-Encoding", "gzip")
			writer.WriteHeader(row.Response.Status)
			compressed := gzip.NewWriter(writer)
			if _, err := io.WriteString(compressed, row.Response.Body); err != nil {
				t.Error(err)
			}
			if err := compressed.Close(); err != nil {
				t.Error(err)
			}
		default:
			writer.WriteHeader(row.Response.Status)
			_, _ = io.WriteString(writer, row.Response.Body)
		}
	}))
	defer server.Close()
	origin = server.URL
	client := server.Client()
	options := dic.HTTPTransportOptions{UserAgent: row.UserAgent}
	if row.Client == "supplied" {
		options.HTTPClient = client
	} else if row.Client != "default" {
		t.Fatalf("unexpected loopback client %q", row.Client)
	}
	transport := dic.NewHTTPTransport(options)
	beforeTransport, beforeTimeout, beforeRedirect, beforeJar := client.Transport, client.Timeout, client.CheckRedirect, client.Jar
	for index := 0; index < row.Repeat; index++ {
		body, status, err := transport.Get(ctx, origin+row.RawURL, row.Accept)
		observed := migrationDictionaryObserveError(err)
		if observed != nil {
			observed.Message, observed.Cause = normalize(observed.Message), normalize(observed.Cause)
		}
		row.Results = append(row.Results, migrationDictionaryHTTPResult{Body: string(body), BodyNil: body == nil, Status: status, Error: observed})
	}
	row.CallerClientUnchanged = client.Transport == beforeTransport && client.Timeout == beforeTimeout && fmt.Sprintf("%p", client.CheckRedirect) == fmt.Sprintf("%p", beforeRedirect) && client.Jar == beforeJar
	if !row.CallerClientUnchanged {
		t.Fatal("dictionary HTTP transport changed its caller-owned loopback client")
	}
	return row
}

func TestMigrationDictionaryHTTPReadFailureIsNotSearch404(t *testing.T) {
	row := migrationDictionaryTransportRow{
		Response: migrationDictionaryResponse{Body: "malformed 404 body", Status: 404}, ReadError: "unexpected-eof", Requests: []migrationDictionaryHTTPRequest{},
	}
	roundTripper := &migrationDictionaryRoundTripper{row: &row, t: t}
	transport := dic.NewHTTPTransport(dic.HTTPTransportOptions{HTTPClient: &http.Client{Transport: roundTripper}})
	results, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "missing"})
	if results != nil || dic.CodeOf(err) != dic.CodeTransport || !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("search through actual HTTPTransport = %#v, %v; want transport read failure before 404 handling", results, err)
	}
	if row.BodyCloses != 1 || len(row.Requests) != 1 || row.Requests[0].Method != "GET" || row.Requests[0].Headers.Get("Accept") != "text/html" {
		t.Fatalf("actual HTTP search boundary = %#v", row)
	}
	var typed *dic.Error
	var urlError *url.Error
	if !errors.As(err, &typed) || typed.StatusCode() != 0 || errors.As(err, &urlError) {
		t.Fatal("body-read failures must preserve direct source and dictionary transport type")
	}
}
