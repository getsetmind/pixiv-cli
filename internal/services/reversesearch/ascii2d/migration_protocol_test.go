package ascii2d

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/http/cookiejar"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	reversesearch "github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
)

var migrationCaptureASCII2D = flag.String("capture-reverse-ascii2d", "", "write frozen ASCII2D observations to the specified owned fixture")

const migrationASCII2DHash = "0123456789abcdef0123456789abcdef"
const migrationASCII2DForm = `<html><body><form id="file_upload" enctype="multipart/form-data" action="/search/file" method="post"><input type="hidden" name="authenticity_token" value="fixture-csrf"><input type="file" name="file"></form></body></html>`
const migrationASCII2DResults = `<html><body><div class="row item-box"><div class="image-box">query image</div></div><div class="row item-box"><div class="image-box"><img src="/thumbnail/fixture.jpg"></div><div class="info-box"><div>1200x800 JPEG 100KB</div><div class="detail-box"><h6><a href="https://www.pixiv.net/artworks/123">Fixture title</a></h6><h6><a href="https://www.pixiv.net/users/456">Fixture author</a></h6><small>pixiv</small></div></div></div></body></html>`
const migrationASCII2DSolution = `{"status":"ok","solution":{"userAgent":"Mozilla/5.0 (X11; Linux x86_64) Chrome/146.0.0.0","cookies":[{"name":"other","value":"discard-me"},{"name":"cf_clearance","value":"clearance-fixture","expires":"2030-01-02T03:04:05Z"}]}}`

var migrationASCII2DSources = map[string]string{
	"internal/services/reversesearch/ascii2d/client.go":       "a92e94d8cb7c42805d66df3bd7d2c6f08beb315218fbcde42f2b674fbb1890e9",
	"internal/services/reversesearch/ascii2d/html.go":         "683a37188dc9823b0f0ab77904ae00a13fb85f38572dd44a6960c214c1635e0b",
	"internal/services/reversesearch/ascii2d/solver.go":       "ae70c6b2f8012f985c87da72efc85749acb69b3589ef93016c95c2062828c82a",
	"internal/services/reversesearch/ascii2d/solver_state.go": "898ce8f370b759ed3a27f02898a57da70981c26f598b0096f3129c3945d34008",
	"internal/services/reversesearch/ascii2d/transport.go":    "a489d18b93012f2a1ef186bb570a0d295197919f1557c90aa2384cfb267ddfa5",
	"internal/services/reversesearch/contracts.go":            "dd6e7df4143baab4ef8afc5dbc80f428391dc90e47c7e8f5eeae2dc7568522df",
	"internal/services/reversesearch/errors.go":               "8d7d8964cae1a095aa3bcff72e56b36b9e3d27c9844bd6e6301210164569b4f0",
	"internal/services/reversesearch/source.go":               "e42ac2585b190ff8092afca916e02a643160904e81c685571d45bc3cf8c82e0f",
}

type migrationASCII2DError struct {
	Code              reversesearch.ErrorCode `json:"code"`
	Message           string                  `json:"message"`
	Chain             []string                `json:"chain"`
	Canceled          bool                    `json:"canceled"`
	Deadline          bool                    `json:"deadline"`
	Challenge         bool                    `json:"challenge"`
	SolverUnavailable bool                    `json:"solver_unavailable"`
	SolverFailed      bool                    `json:"solver_failed"`
	MalformedSolver   bool                    `json:"malformed_solver"`
}

func migrationASCII2DErr(err error) *migrationASCII2DError {
	if err == nil {
		return nil
	}
	out := &migrationASCII2DError{Code: reversesearch.CodeOf(err), Message: err.Error(), Chain: []string{}, Canceled: errors.Is(err, context.Canceled), Deadline: errors.Is(err, context.DeadlineExceeded), Challenge: errors.Is(err, errChallengeDetected), SolverUnavailable: errors.Is(err, ErrSolverUnavailable), SolverFailed: errors.Is(err, ErrSolverFailed), MalformedSolver: errors.Is(err, ErrMalformedSolverResponse)}
	for cause := err; cause != nil; cause = errors.Unwrap(cause) {
		out.Chain = append(out.Chain, cause.Error())
	}
	return out
}

type migrationASCII2DPart struct {
	Name          string      `json:"name"`
	Filename      string      `json:"filename"`
	ContentType   string      `json:"content_type"`
	Headers       http.Header `json:"headers"`
	RawHeadersHex string      `json:"raw_headers_hex,omitempty"`
	DelimiterHex  string      `json:"delimiter_hex,omitempty"`
	Size          int         `json:"size"`
	SHA256        string      `json:"sha256"`
	Hex           string      `json:"hex,omitempty"`
}

type migrationASCII2DMultipart struct {
	MediaType             string            `json:"media_type"`
	Parameters            map[string]string `json:"parameters"`
	BoundaryLength        int               `json:"boundary_length"`
	BoundaryDecodedLength int               `json:"boundary_decoded_length"`
	BoundaryLowerHex      bool              `json:"boundary_lower_hex"`
	BodyRead              bool              `json:"body_read"`
	CompleteFraming       bool              `json:"complete_framing"`
	WireLength            int               `json:"wire_length"`
	WireHex               string            `json:"wire_hex,omitempty"`
	PreambleHex           *string           `json:"preamble_hex"`
	EpilogueHex           *string           `json:"epilogue_hex"`
	TerminatorHex         string            `json:"terminator_hex,omitempty"`
}

func migrationASCII2DMultipartMetadata(t *testing.T, raw string) (string, *migrationASCII2DMultipart, string) {
	t.Helper()
	media, params, err := mime.ParseMediaType(raw)
	if err != nil {
		t.Fatal(err)
	}
	boundary := params["boundary"]
	decoded, err := hex.DecodeString(boundary)
	if media != "multipart/form-data" || len(params) != 1 || err != nil || len(decoded) != 30 || boundary != strings.ToLower(boundary) || raw != "multipart/form-data; boundary="+boundary {
		t.Fatalf("complete multipart Content-Type parameters or boundary properties changed: %q %#v", raw, params)
	}
	params["boundary"] = "<GENERATED_BOUNDARY>"
	info := &migrationASCII2DMultipart{MediaType: media, Parameters: params, BoundaryLength: len(boundary), BoundaryDecodedLength: len(decoded), BoundaryLowerHex: boundary == strings.ToLower(boundary)}
	return strings.Replace(raw, boundary, "<GENERATED_BOUNDARY>", 1), info, boundary
}

func migrationASCII2DObserveMultipart(t *testing.T, data []byte, boundary string, info *migrationASCII2DMultipart) []migrationASCII2DPart {
	t.Helper()
	reader := multipart.NewReader(bytes.NewReader(data), boundary)
	parts := []migrationASCII2DPart{}
	payloads := [][]byte{}
	for {
		part, err := reader.NextPart()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		payload, err := io.ReadAll(part)
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(payload)
		p := migrationASCII2DPart{Name: part.FormName(), Filename: part.FileName(), ContentType: part.Header.Get("Content-Type"), Headers: http.Header(part.Header).Clone(), Size: len(payload), SHA256: hex.EncodeToString(digest[:])}
		if len(payload) <= 512 {
			p.Hex = hex.EncodeToString(payload)
		}
		parts = append(parts, p)
		payloads = append(payloads, payload)
		if err := part.Close(); err != nil {
			t.Fatal(err)
		}
	}
	if len(parts) != 2 || parts[0].Name != "authenticity_token" || parts[0].Filename != "" || parts[0].ContentType != "" || len(parts[0].Headers) != 1 || len(parts[0].Headers["Content-Disposition"]) != 1 || parts[0].Headers.Get("Content-Disposition") != `form-data; name="authenticity_token"` {
		t.Fatalf("ordered CSRF multipart part or complete header keys changed: %#v", parts)
	}
	filename := map[string]string{"image/png": "image.png", "image/jpeg": "image.jpg", "image/webp": "image.webp"}[parts[1].ContentType]
	fileDisposition := fmt.Sprintf(`form-data; name="file"; filename="%s"`, filename)
	if filename == "" || parts[1].Name != "file" || parts[1].Filename != filename || len(parts[1].Headers) != 2 || len(parts[1].Headers["Content-Disposition"]) != 1 || len(parts[1].Headers["Content-Type"]) != 1 || parts[1].Headers.Get("Content-Disposition") != fileDisposition {
		t.Fatalf("ordered file multipart part or complete header keys changed: %#v", parts[1])
	}
	headers := []string{`Content-Disposition: form-data; name="authenticity_token"`, "Content-Disposition: " + fileDisposition + "\r\nContent-Type: " + parts[1].ContentType}
	var exact bytes.Buffer
	for index, header := range headers {
		if index > 0 {
			exact.WriteString("\r\n")
		}
		exact.WriteString("--" + boundary + "\r\n" + header + "\r\n\r\n")
		exact.Write(payloads[index])
	}
	exact.WriteString("\r\n--" + boundary + "--\r\n")
	if !bytes.Equal(exact.Bytes(), data) {
		t.Fatal("actual complete multipart wire differs in delimiter, ordered raw headers, payload, framing, preamble or epilogue")
	}
	offset := 0
	var canonical bytes.Buffer
	for index := range parts {
		delimiter := "--" + boundary + "\r\n"
		if index > 0 {
			delimiter = "\r\n" + delimiter
		}
		rawDelimiter := data[offset : offset+len(delimiter)]
		offset += len(delimiter)
		maskedDelimiter := bytes.Replace(rawDelimiter, []byte(boundary), []byte("<GENERATED_BOUNDARY>"), 1)
		parts[index].DelimiterHex = hex.EncodeToString(maskedDelimiter)
		canonical.Write(maskedDelimiter)
		end := bytes.Index(data[offset:], []byte("\r\n\r\n"))
		if end < 0 {
			t.Fatal("actual MIME header terminator missing")
		}
		end += offset + 4
		rawHeader := data[offset:end]
		parts[index].RawHeadersHex = hex.EncodeToString(rawHeader)
		canonical.Write(rawHeader)
		offset = end
		canonical.Write(data[offset : offset+parts[index].Size])
		offset += parts[index].Size
	}
	terminator := data[offset:]
	maskedTerminator := bytes.Replace(terminator, []byte(boundary), []byte("<GENERATED_BOUNDARY>"), 1)
	info.TerminatorHex = hex.EncodeToString(maskedTerminator)
	canonical.Write(maskedTerminator)
	preambleEnd := bytes.Index(data, []byte("--"+boundary+"\r\n"))
	preamble := hex.EncodeToString(data[:preambleEnd])
	info.PreambleHex = &preamble
	closingMarker := []byte("\r\n--" + boundary + "--\r\n")
	epilogueStart := bytes.LastIndex(data, closingMarker) + len(closingMarker)
	epilogue := hex.EncodeToString(data[epilogueStart:])
	info.EpilogueHex = &epilogue
	info.BodyRead = true
	info.CompleteFraming = true
	info.WireLength = len(data)
	if len(data) <= 2048 {
		info.WireHex = hex.EncodeToString(canonical.Bytes())
	}
	return parts
}

type migrationASCII2DRequest struct {
	Method           string                     `json:"method"`
	URL              string                     `json:"url"`
	Headers          http.Header                `json:"headers"`
	BodyNil          bool                       `json:"body_nil"`
	ContentLength    int64                      `json:"content_length"`
	Multipart        bool                       `json:"multipart"`
	MultipartDetails *migrationASCII2DMultipart `json:"multipart_details"`
	Parts            []migrationASCII2DPart     `json:"parts"`
	JSON             json.RawMessage            `json:"json,omitempty"`
	JSONHex          string                     `json:"json_hex,omitempty"`
	ContextValue     string                     `json:"context_value"`
	ContextCanceled  bool                       `json:"context_canceled"`
	ContextDeadline  bool                       `json:"context_deadline"`
}

type migrationASCII2DResponse struct {
	Method      string      `json:"method"`
	Path        string      `json:"path"`
	Status      int         `json:"status"`
	Headers     http.Header `json:"headers"`
	Body        string      `json:"body"`
	Error       string      `json:"error,omitempty"`
	ReadError   bool        `json:"read_error,omitempty"`
	Cancel      string      `json:"cancel,omitempty"`
	CloseUpload bool        `json:"close_upload,omitempty"`
}

type migrationASCII2DInput struct {
	Operation        string                     `json:"operation"`
	Endpoint         string                     `json:"endpoint"`
	Proxy            string                     `json:"proxy"`
	UserAgent        string                     `json:"user_agent"`
	SolverURL        string                     `json:"solver_url"`
	SolverSession    string                     `json:"solver_session"`
	SolverProxy      string                     `json:"solver_proxy"`
	Context          string                     `json:"context"`
	SearchContext    string                     `json:"search_context"`
	Snapshot         string                     `json:"snapshot"`
	ImageHex         string                     `json:"image_hex"`
	ImageSize        int64                      `json:"image_size"`
	Providers        []reversesearch.Provider   `json:"providers"`
	Repeat           int                        `json:"repeat"`
	Redirect         string                     `json:"redirect"`
	CloseBefore      bool                       `json:"close_before"`
	CloseFromSession bool                       `json:"close_from_session"`
	Responses        []migrationASCII2DResponse `json:"responses"`
	SolverResponses  []migrationASCII2DResponse `json:"solver_responses"`
}

type migrationASCII2DOutcome struct {
	Stage    string                          `json:"stage"`
	Session  bool                            `json:"session"`
	Response *reversesearch.ProviderResponse `json:"response"`
	Error    *migrationASCII2DError          `json:"error"`
}

type migrationASCII2DRow struct {
	Name                  string                    `json:"name"`
	Boundary              string                    `json:"boundary"`
	Input                 migrationASCII2DInput     `json:"input"`
	Outcomes              []migrationASCII2DOutcome `json:"outcomes"`
	Requests              []migrationASCII2DRequest `json:"requests"`
	SolverRequests        []migrationASCII2DRequest `json:"solver_requests"`
	ResponseCloses        []int                     `json:"response_closes"`
	SolverResponseCloses  []int                     `json:"solver_response_closes"`
	IdleCloses            int                       `json:"idle_closes"`
	SolverIdleCloses      int                       `json:"solver_idle_closes"`
	CallerJarUnchanged    bool                      `json:"caller_jar_unchanged"`
	CallerClientUnchanged bool                      `json:"caller_client_unchanged"`
	RedirectCalls         int                       `json:"redirect_calls"`
}

type migrationASCII2DContextKey struct{}

type migrationASCII2DBody struct {
	reader *strings.Reader
	closes *int
	fail   bool
	cancel context.CancelFunc
}

func (b *migrationASCII2DBody) Read(p []byte) (int, error) {
	if b.cancel != nil {
		b.cancel()
		b.cancel = nil
	}
	if b.fail {
		b.fail = false
		return 0, errors.New("synthetic-body-canary")
	}
	return b.reader.Read(p)
}
func (b *migrationASCII2DBody) Close() error { *b.closes++; return nil }

type migrationASCII2DTransport struct {
	t        *testing.T
	steps    []migrationASCII2DResponse
	requests *[]migrationASCII2DRequest
	closes   []int
	idle     int
	cancel   context.CancelFunc
}

func (tr *migrationASCII2DTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	n := len(*tr.requests)
	if n >= len(tr.steps) {
		tr.t.Fatalf("unexpected %s %s", req.Method, req.URL)
	}
	step := tr.steps[n]
	if req.Method != step.Method || req.URL.RequestURI() != step.Path {
		tr.t.Fatalf("request %d = %s %s; want %s %s", n, req.Method, req.URL.RequestURI(), step.Method, step.Path)
	}
	observed := migrationASCII2DRequest{Method: req.Method, URL: req.URL.String(), Headers: req.Header.Clone(), BodyNil: req.Body == nil, ContentLength: req.ContentLength, Parts: []migrationASCII2DPart{}}
	observed.ContextValue, _ = req.Context().Value(migrationASCII2DContextKey{}).(string)
	observed.ContextCanceled = errors.Is(req.Context().Err(), context.Canceled)
	observed.ContextDeadline = errors.Is(req.Context().Err(), context.DeadlineExceeded)
	if req.Body != nil {
		media, _, err := mime.ParseMediaType(req.Header.Get("Content-Type"))
		if err != nil {
			tr.t.Fatal(err)
		}
		boundary := ""
		if media == "multipart/form-data" {
			if len(req.Header.Values("Content-Type")) != 1 {
				tr.t.Fatal("multipart request must carry one complete Content-Type value")
			}
			raw, details, value := migrationASCII2DMultipartMetadata(tr.t, req.Header.Get("Content-Type"))
			observed.Multipart = true
			observed.MultipartDetails = details
			observed.Headers.Set("Content-Type", raw)
			boundary = value
		}
		if step.CloseUpload {
			if err := req.Body.Close(); err != nil {
				tr.t.Fatal(err)
			}
		} else {
			body, err := io.ReadAll(req.Body)
			if err != nil {
				tr.t.Fatal(err)
			}
			if observed.Multipart {
				observed.Parts = migrationASCII2DObserveMultipart(tr.t, body, boundary, observed.MultipartDetails)
			} else {
				if !json.Valid(body) {
					tr.t.Fatalf("control JSON malformed: %s", body)
				}
				observed.JSON = body
				observed.JSONHex = hex.EncodeToString(body)
			}
			if err := req.Body.Close(); err != nil {
				tr.t.Fatal(err)
			}
		}
	}
	*tr.requests = append(*tr.requests, observed)
	if step.Cancel == "transport" {
		tr.cancel()
	}
	if step.Cancel == "wait-context" {
		tr.cancel()
		<-req.Context().Done()
		return nil, req.Context().Err()
	}
	if step.Error != "" {
		if step.Error == "context" {
			return nil, req.Context().Err()
		}
		return nil, errors.New("synthetic-transport-canary")
	}
	header := step.Headers.Clone()
	if header == nil {
		header = make(http.Header)
	}
	b := &migrationASCII2DBody{reader: strings.NewReader(step.Body), closes: &tr.closes[n], fail: step.ReadError}
	if step.Cancel == "read" {
		b.cancel = tr.cancel
	}
	return &http.Response{StatusCode: step.Status, Header: header, Body: b, Request: req}, nil
}
func (tr *migrationASCII2DTransport) CloseIdleConnections() { tr.idle++ }

func migrationASCII2DSnapshot(t *testing.T, input migrationASCII2DInput) *reversesearch.Snapshot {
	t.Helper()
	if input.Snapshot == "nil" {
		return nil
	}
	payload, err := hex.DecodeString(input.ImageHex)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	path := filepath.Join(dir, "fixture-image")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = file.Write(payload); err != nil {
		t.Fatal(err)
	}
	if err = file.Truncate(input.ImageSize); err != nil {
		t.Fatal(err)
	}
	if err = file.Close(); err != nil {
		t.Fatal(err)
	}
	snapshot, err := reversesearch.NewSourceLoader(reversesearch.SourceLoaderOptions{TempDir: dir}).Load(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := snapshot.Close(); err != nil {
			t.Error(err)
		}
	})
	if input.Snapshot == "closed" {
		if err := snapshot.Close(); err != nil {
			t.Fatal(err)
		}
	}
	return snapshot
}

func migrationASCII2DRun(t *testing.T, row migrationASCII2DRow) migrationASCII2DRow {
	t.Helper()
	row.Outcomes = []migrationASCII2DOutcome{}
	row.Requests = []migrationASCII2DRequest{}
	row.SolverRequests = []migrationASCII2DRequest{}
	input := row.Input
	ctx := context.WithValue(context.Background(), migrationASCII2DContextKey{}, "synthetic-context-value")
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	switch input.Context {
	case "nil":
		ctx = nil
	case "canceled":
		cancel()
	case "deadline":
		var c context.CancelFunc
		ctx, c = context.WithDeadline(ctx, time.Unix(1, 0))
		defer c()
	}
	tr := &migrationASCII2DTransport{t: t, steps: input.Responses, requests: &row.Requests, closes: make([]int, len(input.Responses)), cancel: cancel}
	jar, err := cookiejar.New(nil)
	if err != nil {
		t.Fatal(err)
	}
	jarURL, _ := url.Parse("https://ascii2d.invalid")
	jar.SetCookies(jarURL, []*http.Cookie{{Name: "caller-cookie", Value: "caller-fixture", Path: "/"}})
	base := &http.Client{Transport: tr, Jar: jar, Timeout: 0}
	if input.Redirect != "" {
		base.CheckRedirect = func(*http.Request, []*http.Request) error {
			row.RedirectCalls++
			if input.Redirect == "last" {
				return http.ErrUseLastResponse
			}
			if input.Redirect == "error" {
				return errors.New("synthetic-redirect-canary")
			}
			return nil
		}
	}
	beforeRedirect := fmt.Sprintf("%p", base.CheckRedirect)
	opts := Options{HTTPClient: base, Endpoint: input.Endpoint, ProxyURL: input.Proxy, UserAgent: input.UserAgent}
	if input.SolverURL != "" {
		opts.FlareSolverr = &FlareSolverrOptions{URL: input.SolverURL, ProxyURL: input.SolverProxy}
	}
	client, err := New(opts)
	row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "new", Session: client != nil, Error: migrationASCII2DErr(err)})
	var solverTransport *migrationASCII2DTransport
	if client != nil && client.solver != nil {
		solverTransport = &migrationASCII2DTransport{t: t, steps: input.SolverResponses, requests: &row.SolverRequests, closes: make([]int, len(input.SolverResponses)), cancel: cancel}
		client.solver.httpClient = &http.Client{Transport: solverTransport, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
		row.Input.SolverSession = "pixiv-cli-ascii2d-fixture"
		client.solver.session = row.Input.SolverSession
	}
	if client != nil {
		switch input.Operation {
		case "new":
		case "preflight":
			err = client.Preflight(ctx)
			row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "preflight", Error: migrationASCII2DErr(err)})
		case "nil-client":
			var nilClient *Client
			err = nilClient.Preflight(ctx)
			row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "preflight", Error: migrationASCII2DErr(err)})
		case "zero-client":
			err = (&Client{}).Preflight(ctx)
			row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "preflight", Error: migrationASCII2DErr(err)})
		case "nil-session":
			var session *Session
			response, e := session.Search(ctx, reversesearch.ProviderASCII2DColor)
			row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "search", Response: &response, Error: migrationASCII2DErr(e)})
		case "upload":
			snapshot := migrationASCII2DSnapshot(t, input)
			if input.CloseBefore {
				err = client.Close()
				row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "close-before", Error: migrationASCII2DErr(err)})
			}
			for i := 0; i < input.Repeat; i++ {
				session, e := client.Upload(ctx, snapshot)
				row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "upload", Session: session != nil, Error: migrationASCII2DErr(e)})
				if e != nil && client.solverCache != nil {
					client.solverCache.mu.Lock()
					call := client.solverCache.active
					client.solverCache.mu.Unlock()
					if call != nil {
						<-call.done
					}
				}
				if e == nil {
					for _, provider := range input.Providers {
						searchContext := ctx
						if input.SearchContext == "nil" {
							searchContext = nil
						}
						if input.SearchContext == "canceled" {
							ended, c := context.WithCancel(ctx)
							c()
							searchContext = ended
						}
						if input.SearchContext == "deadline" {
							ended, c := context.WithDeadline(ctx, time.Unix(1, 0))
							defer c()
							searchContext = ended
						}
						response, e := session.Search(searchContext, provider)
						row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "search:" + string(provider), Response: &response, Error: migrationASCII2DErr(e)})
					}
					if input.CloseFromSession {
						err = session.(*Session).client.Close()
						row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "close-session-client", Error: migrationASCII2DErr(err)})
					}
				}
			}
		default:
			t.Fatalf("unknown operation %q", input.Operation)
		}
		err = client.Close()
		row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "close", Error: migrationASCII2DErr(err)})
		err = client.Close()
		row.Outcomes = append(row.Outcomes, migrationASCII2DOutcome{Stage: "close-again", Error: migrationASCII2DErr(err)})
	}
	for index := range row.Requests {
		want := 1
		step := input.Responses[index]
		if step.Error != "" || step.Cancel == "wait-context" {
			want = 0
		}
		if tr.closes[index] != want {
			t.Fatalf("response %d closed %d times; want %d", index, tr.closes[index], want)
		}
	}
	if solverTransport != nil {
		for index := range row.SolverRequests {
			want := 1
			step := input.SolverResponses[index]
			if step.Error != "" || step.Cancel == "wait-context" {
				want = 0
			}
			if solverTransport.closes[index] != want {
				t.Fatalf("solver response %d closed %d times; want %d", index, solverTransport.closes[index], want)
			}
		}
	}
	row.ResponseCloses = tr.closes
	row.IdleCloses = tr.idle
	row.SolverResponseCloses = []int{}
	if solverTransport != nil {
		row.SolverResponseCloses = solverTransport.closes
		row.SolverIdleCloses = solverTransport.idle
	}
	row.CallerJarUnchanged = len(jar.Cookies(jarURL)) == 1 && jar.Cookies(jarURL)[0].Name == "caller-cookie" && jar.Cookies(jarURL)[0].Value == "caller-fixture"
	row.CallerClientUnchanged = base.Transport == tr && base.Jar == jar && base.Timeout == 0 && fmt.Sprintf("%p", base.CheckRedirect) == beforeRedirect
	if !row.CallerJarUnchanged || !row.CallerClientUnchanged {
		t.Fatal("provider changed caller-owned HTTP client or jar")
	}
	return row
}

func migrationASCII2DHome() migrationASCII2DResponse {
	return migrationASCII2DResponse{Method: "GET", Path: "/", Status: 200, Headers: http.Header{"Set-Cookie": {"ascii2d_session=fixture-session; Path=/"}}, Body: migrationASCII2DForm}
}
func migrationASCII2DUpload() migrationASCII2DResponse {
	return migrationASCII2DResponse{Method: "POST", Path: "/search/file", Status: 302, Headers: http.Header{"Location": {"/search/color/" + migrationASCII2DHash}}}
}
func migrationASCII2DResult(provider reversesearch.Provider) migrationASCII2DResponse {
	mode := "color"
	if provider == reversesearch.ProviderASCII2DBOVW {
		mode = "bovw"
	}
	return migrationASCII2DResponse{Method: "GET", Path: "/search/" + mode + "/" + migrationASCII2DHash, Status: 200, Body: migrationASCII2DResults}
}
func migrationASCII2DSolverResponse(body string) migrationASCII2DResponse {
	return migrationASCII2DResponse{Method: "POST", Path: "/v1", Status: 200, Body: body}
}

func migrationASCII2DRows() []migrationASCII2DRow {
	rows := []migrationASCII2DRow{}
	png := []byte("\x89PNG\r\n\x1a\nfixture")
	base := func() migrationASCII2DInput {
		return migrationASCII2DInput{Operation: "upload", Endpoint: "https://ascii2d.invalid", Snapshot: "owned", ImageHex: hex.EncodeToString(png), ImageSize: int64(len(png)), Repeat: 1, Providers: []reversesearch.Provider{}, Responses: []migrationASCII2DResponse{}, SolverResponses: []migrationASCII2DResponse{}}
	}
	add := func(name string, in migrationASCII2DInput) {
		boundary := "public-client-http-and-solver-ports"
		if in.CloseFromSession {
			boundary = "public-upload-and-private-session-client-close-observation"
		}
		rows = append(rows, migrationASCII2DRow{Name: name, Boundary: boundary, Input: in})
	}
	for _, endpoint := range []string{"https://ascii2d.invalid/base", "HTTP://ascii2d.invalid", "", "ftp://ascii2d.invalid", "https://user@ascii2d.invalid", "https://ascii2d.invalid/?q=1", "https://ascii2d.invalid/#frag", ":bad", "/relative"} {
		in := base()
		in.Operation = "new"
		in.Endpoint = endpoint
		add("new-endpoint-"+fmt.Sprint(len(rows)), in)
	}
	for _, proxy := range []string{" ", "http://proxy.invalid:8080", "https://proxy.invalid", "socks5://proxy.invalid:1080", "socks5h://proxy.invalid:1080", "ftp://proxy.invalid", "HTTP://proxy.invalid", "http://proxy.invalid/path", "http://proxy.invalid?"} {
		in := base()
		in.Operation = "new"
		in.Proxy = proxy
		add("new-proxy-"+fmt.Sprint(len(rows)), in)
	}
	for _, ua := range []string{"fixture-firefox-agent", "Mozilla/5.0 (Windows NT 10.0) Edg/149.0 Chrome/149.0", "bad\r\nX: secret", "bad\x00agent"} {
		in := base()
		in.Operation = "new"
		in.UserAgent = ua
		add("new-user-agent-"+fmt.Sprint(len(rows)), in)
	}
	in := base()
	in.Operation = "new"
	in.SolverURL = "https://user@solver.invalid"
	add("invalid-solver-configuration", in)
	for _, operation := range []string{"preflight", "nil-client", "zero-client", "nil-session"} {
		for _, state := range []string{"active", "nil", "canceled", "deadline"} {
			in := base()
			in.Operation = operation
			in.Context = state
			add(operation+"-"+state, in)
		}
	}
	for _, state := range []string{"nil", "canceled", "deadline"} {
		in := base()
		in.Context = state
		add("upload-context-"+state, in)
	}
	for _, snapshot := range []string{"nil", "closed"} {
		in := base()
		in.Snapshot = snapshot
		add("upload-snapshot-"+snapshot, in)
	}
	for _, content := range [][]byte{{}, []byte("not-an-image"), []byte("GIF89a-fixture")} {
		in := base()
		in.ImageHex = hex.EncodeToString(content)
		in.ImageSize = int64(len(content))
		add("upload-invalid-image-"+fmt.Sprint(len(rows)), in)
	}
	in = base()
	in.ImageSize = MaxImageBytes + 1
	add("upload-size-over-10mb-no-http", in)
	for _, media := range []struct {
		name string
		body []byte
	}{{"png", png}, {"jpeg", []byte{0xff, 0xd8, 0xff, 0xe0, 0, 0x10, 'J', 'F', 'I', 'F', 0}}, {"webp", []byte("RIFF\x04\x00\x00\x00WEBPVP8 ")}} {
		for _, size := range []int64{int64(len(media.body)), MaxImageBytes} {
			in := base()
			in.ImageHex = hex.EncodeToString(media.body)
			in.ImageSize = size
			in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
			in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor, reversesearch.ProviderASCII2DBOVW}
			in.Responses = append(in.Responses, migrationASCII2DResult(reversesearch.ProviderASCII2DColor), migrationASCII2DResult(reversesearch.ProviderASCII2DBOVW))
			add(fmt.Sprintf("one-upload-both-modes-%s-size-%d", media.name, size), in)
		}
	}
	in = base()
	in.Repeat = 2
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload(), migrationASCII2DHome(), migrationASCII2DUpload()}
	in.Responses[2].Headers = http.Header{"Set-Cookie": {"ascii2d_session=second-session; Path=/"}}
	in.Responses[2].Body = strings.ReplaceAll(migrationASCII2DForm, "fixture-csrf", "second-csrf")
	add("independent-second-upload-cookie-and-csrf", in)
	in = base()
	in.UserAgent = "fixture-firefox-agent"
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
	add("upload-nonchromium-omits-hints", in)
	in = base()
	in.UserAgent = "Mozilla/5.0 (Linux; Android 12) Chrome/149.0"
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
	add("upload-custom-chromium-mobile-hints", in)
	for _, change := range []struct{ name, from, to string }{{"id", "file_upload", "other"}, {"action", "/search/file", "https://ascii2d.invalid/search/file"}, {"method", "method=\"post\"", "method=\"get\""}, {"enctype", "multipart/form-data", "application/x-www-form-urlencoded"}, {"token-type", "type=\"hidden\"", "type=\"text\""}, {"token-empty", "fixture-csrf", " "}, {"file-type", "type=\"file\"", "type=\"text\""}, {"case-insensitive-method", "method=\"post\"", "method=\"POST\""}, {"token-trim", "fixture-csrf", "  fixture-csrf  "}} {
		in := base()
		home := migrationASCII2DHome()
		home.Body = strings.ReplaceAll(home.Body, change.from, change.to)
		in.Responses = []migrationASCII2DResponse{home}
		if change.name == "case-insensitive-method" || change.name == "token-trim" {
			in.Responses = append(in.Responses, migrationASCII2DUpload())
		}
		add("csrf-form-"+change.name, in)
	}
	for _, location := range []string{"/search/color/" + migrationASCII2DHash, "http://ascii2d.invalid/search/color/" + migrationASCII2DHash, "https://ASCII2D.INVALID:443/search/color/" + migrationASCII2DHash, "//evil.invalid/search/color/" + migrationASCII2DHash, "https://user@ascii2d.invalid/search/color/" + migrationASCII2DHash, "http://ascii2d.invalid:80/search/color/" + migrationASCII2DHash, "/search/color/" + migrationASCII2DHash + "?x=1", "/search/color/" + migrationASCII2DHash + "#frag", "/search/bovw/" + migrationASCII2DHash, "/search/color/" + strings.ToUpper(migrationASCII2DHash), "/search/color/short", "/search/color/" + migrationASCII2DHash + "/", "/search/color/%30" + migrationASCII2DHash[1:], "", ":bad"} {
		in := base()
		upload := migrationASCII2DUpload()
		upload.Headers = http.Header{"Location": {location}}
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), upload}
		add("upload-location-"+fmt.Sprint(len(rows)), in)
	}
	for _, stage := range []string{"home", "upload", "result"} {
		for _, status := range []int{200, 204, 301, 400, 403, 429, 500} {
			in := base()
			response := migrationASCII2DHome()
			if stage == "upload" {
				response = migrationASCII2DUpload()
			} else if stage == "result" {
				response = migrationASCII2DResult(reversesearch.ProviderASCII2DColor)
			}
			response.Status = status
			response.Headers = http.Header{}
			response.Body = "ordinary private upstream body"
			in.Responses = []migrationASCII2DResponse{response}
			if stage != "home" {
				in.Responses = append([]migrationASCII2DResponse{migrationASCII2DHome()}, response)
			}
			if stage == "result" {
				in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
				in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload(), response}
			}
			add(fmt.Sprintf("%s-status-%d", stage, status), in)
		}
	}
	for _, challenge := range []struct {
		name, body, content string
		status              int
		header              string
		readError           bool
	}{{"header-success", "ignored-body", "text/plain", 200, " CHALLENGE ", true}, {"header-token-list", "ignored-body", "text/html", 403, "other, challenge", false}, {"403-title-split", "<title>Just <span>a</span> moment</title>", "text/html; charset=utf-8", 403, "", false}, {"403-attribute", "<script src='/cdn-cgi/challenge-platform/script'></script>", "application/xhtml+xml", 403, "", false}, {"403-late-marker", strings.Repeat("x", 70000) + "<h1>Verify you are human</h1>", "text/html", 403, "", false}, {"403-ordinary-waf", "<title>Access denied</title> Ray ID 123", "text/html", 403, "", false}, {"403-nonhtml", "Just a moment", "text/plain", 403, "", false}, {"403-malformed-content-type", "Just a moment", "text/html; bad", 403, "", false}, {"403-read-failure", "", "text/html", 403, "", true}, {"200-html-marker-no-header", "Just a moment", "text/html", 200, "", false}} {
		in := base()
		home := migrationASCII2DHome()
		home.Status = challenge.status
		home.Body = challenge.body
		home.Headers = http.Header{"Content-Type": {challenge.content}}
		if challenge.header != "" {
			home.Headers.Set("Cf-Mitigated", challenge.header)
		}
		home.ReadError = challenge.readError
		in.Responses = []migrationASCII2DResponse{home}
		add("challenge-home-"+challenge.name, in)
	}
	in = base()
	upload := migrationASCII2DUpload()
	upload.Status = 403
	upload.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), upload}
	add("challenge-upload-no-solver", in)
	in = base()
	result := migrationASCII2DResult(reversesearch.ProviderASCII2DColor)
	result.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
	in.SolverURL = "http://solver.invalid"
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload(), result}
	in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
	add("challenge-result-no-recovery", in)
	for _, stage := range []string{"home", "upload", "result"} {
		for _, mode := range []string{"transport-failure", "cancel-transport", "read-failure", "cancel-read"} {
			in := base()
			response := migrationASCII2DHome()
			if stage == "upload" {
				response = migrationASCII2DUpload()
			} else if stage == "result" {
				response = migrationASCII2DResult(reversesearch.ProviderASCII2DColor)
			}
			switch mode {
			case "transport-failure":
				response.Error = "fixture"
			case "cancel-transport":
				response.Cancel = "transport"
			case "read-failure":
				response.ReadError = true
			case "cancel-read":
				response.Cancel = "read"
			}
			if stage == "upload" && (mode == "read-failure" || mode == "cancel-read") {
				response.Status = 403
				response.Headers = http.Header{"Content-Type": {"text/html"}}
			}
			in.Responses = []migrationASCII2DResponse{response}
			if stage == "upload" {
				in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), response}
			}
			if stage == "home" && mode == "cancel-read" {
				terminal := migrationASCII2DUpload()
				terminal.CloseUpload = true
				terminal.Error = "context"
				in.Responses = append(in.Responses, terminal)
			}
			if stage == "result" {
				in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload(), response}
				in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
			}
			add(stage+"-"+mode, in)
		}
	}
	for _, failure := range []string{"status", "cancel"} {
		in := base()
		upload := migrationASCII2DUpload()
		upload.CloseUpload = true
		if failure == "status" {
			upload.Status = 429
			upload.Headers = http.Header{}
		} else {
			upload.Cancel = "transport"
		}
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), upload}
		add("upload-writer-failure-priority-"+failure, in)
	}
	for _, stage := range []string{"home", "result"} {
		for _, redirect := range []string{"same", "cross", "loop", "caller-last", "caller-error"} {
			in := base()
			response := migrationASCII2DHome()
			if stage == "result" {
				response = migrationASCII2DResult(reversesearch.ProviderASCII2DColor)
				in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
			}
			response.Status = 302
			response.Headers = http.Header{"Location": {"/final"}}
			response.Body = "redirect-body"
			steps := []migrationASCII2DResponse{response}
			switch redirect {
			case "cross":
				steps[0].Headers.Set("Location", "https://evil.invalid/final")
			case "loop":
				steps[0].Headers.Set("Location", response.Path)
				for i := 1; i < 10; i++ {
					steps = append(steps, response)
					steps[i].Headers.Set("Location", response.Path)
				}
			case "caller-last":
				in.Redirect = "last"
			case "caller-error":
				in.Redirect = "error"
			default:
				terminal := migrationASCII2DHome()
				terminal.Path = "/final"
				if stage == "result" {
					terminal.Body = migrationASCII2DResults
				}
				steps = append(steps, terminal)
			}
			in.Responses = steps
			if stage == "home" && redirect == "same" {
				in.Responses = append(in.Responses, migrationASCII2DUpload())
			}
			if stage == "result" {
				in.Responses = append([]migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}, steps...)
			}
			add(stage+"-redirect-"+redirect, in)
		}
	}
	for _, provider := range []reversesearch.Provider{reversesearch.ProviderSauceNAO, reversesearch.ProviderAll, ""} {
		in := base()
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
		in.Providers = []reversesearch.Provider{provider}
		add("session-invalid-mode-"+string(provider), in)
	}
	resultBodies := []struct{ name, body string }{{"empty-results", `<div class="item-box">query</div>`}, {"advertisement", strings.Replace(migrationASCII2DResults, `<div class="row item-box"><div class="image-box"><img`, `<div class="item-box">advertisement</div><div class="row item-box"><div class="image-box"><img`, 1)}, {"external", `<div class="item-box">query</div><div class="item-box"><div class="info-box"><div class="detail-box"><div class="external">External title<a href="https://example.test/external">Dlsite</a></div></div></div></div>`}, {"no-query", `<p>no results</p>`}, {"image-without-info", `<div class="item-box">query</div><div class="item-box"><div class="image-box">image</div></div>`}, {"detail-drift", strings.ReplaceAll(migrationASCII2DResults, "detail-box", "detail")}, {"source-missing", strings.ReplaceAll(migrationASCII2DResults, "<small>pixiv</small>", "")}, {"links-missing", `<div class="item-box">query</div><div class="item-box"><div class="info-box"><div class="detail-box"><small>pixiv</small></div></div></div>`}, {"external-title-missing", `<div class="item-box">query</div><div class="item-box"><div class="info-box"><div class="detail-box"><div class="external"><a href="https://example.test/x">source</a></div></div></div></div>`}, {"whitespace-duplicates-and-raw-url", strings.ReplaceAll(strings.ReplaceAll(migrationASCII2DResults, "Fixture title", "  Fixture\n <b>title</b>  "), "https://www.pixiv.net/users/456", "  javascript:fixture  ")}}
	for _, body := range resultBodies {
		in := base()
		result := migrationASCII2DResult(reversesearch.ProviderASCII2DColor)
		result.Body = body.body
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload(), result}
		in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
		add("parser-"+body.name, in)
	}
	for _, stage := range []string{"home", "upload"} {
		for _, mode := range []string{"success", "rechallenge-home", "rechallenge-upload", "second-upload-cache-reuse", "close-session", "destroy-failure"} {
			in := base()
			in.SolverURL = "http://solver.invalid"
			in.SolverProxy = "socks5://solver-proxy.invalid:1080"
			challenge := migrationASCII2DHome()
			challenge.Status = 403
			challenge.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
			challenge.Body = "challenge-canary"
			in.Responses = []migrationASCII2DResponse{challenge}
			if stage == "upload" {
				challenge = migrationASCII2DUpload()
				challenge.Status = 403
				challenge.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
				in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), challenge}
			}
			home := migrationASCII2DHome()
			home.Body = strings.ReplaceAll(home.Body, "fixture-csrf", "fresh-csrf")
			home.Headers = http.Header{"Set-Cookie": {"ascii2d_session=fresh-session; Path=/"}}
			upload := migrationASCII2DUpload()
			if mode == "rechallenge-home" {
				home.Status = 403
				home.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
				in.Responses = append(in.Responses, home)
			} else {
				if mode == "rechallenge-upload" {
					upload.Status = 403
					upload.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
				}
				in.Responses = append(in.Responses, home, upload)
			}
			in.SolverResponses = []migrationASCII2DResponse{migrationASCII2DSolverResponse(`{"status":"ok"}`), migrationASCII2DSolverResponse(migrationASCII2DSolution), migrationASCII2DSolverResponse(`{"status":"ok"}`)}
			if mode == "second-upload-cache-reuse" {
				in.Repeat = 2
				if stage == "upload" {
					in.Responses = append(in.Responses, migrationASCII2DHome())
				}
				in.Responses = append(in.Responses, challenge, home, upload)
			}
			if mode == "success" {
				in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor, reversesearch.ProviderASCII2DBOVW}
				in.Responses = append(in.Responses, migrationASCII2DResult(reversesearch.ProviderASCII2DColor), migrationASCII2DResult(reversesearch.ProviderASCII2DBOVW))
			}
			if mode == "close-session" {
				in.CloseFromSession = true
			}
			if mode == "destroy-failure" {
				in.SolverResponses[2].Status = 500
				in.SolverResponses[2].Body = "destroy-private-canary"
			}
			add("solver-"+stage+"-"+mode, in)
		}
	}
	solverFailures := []struct {
		name, body string
		status     int
		transport  bool
	}{{"transport", "", 0, true}, {"http", "solver-private-canary", 502, false}, {"status-error", `{"status":"error","message":"solver-private-canary"}`, 200, false}, {"status-unknown", `{"status":"pending"}`, 200, false}, {"invalid-json", `{"status":`, 200, false}, {"trailing-json", `{"status":"ok"}{"status":"ok"}`, 200, false}, {"solution-missing", `{"status":"ok"}`, 200, false}, {"ua-missing", `{"status":"ok","solution":{"cookies":[{"name":"cf_clearance","value":"fixture"}]}}`, 200, false}, {"clearance-missing", `{"status":"ok","solution":{"userAgent":"fixture"}}`, 200, false}, {"clearance-duplicate", `{"status":"ok","solution":{"userAgent":"fixture","cookies":[{"name":"cf_clearance","value":"one"},{"name":"cf_clearance","value":"two"}]}}`, 200, false}, {"clearance-invalid", strings.ReplaceAll(migrationASCII2DSolution, "clearance-fixture", "bad;value"), 200, false}, {"expiry-invalid", strings.ReplaceAll(migrationASCII2DSolution, "2030-01-02T03:04:05Z", "not-a-date"), 200, false}}
	for _, failure := range solverFailures {
		in := base()
		in.SolverURL = "http://solver.invalid"
		home := migrationASCII2DHome()
		home.Status = 403
		home.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
		in.Responses = []migrationASCII2DResponse{home}
		get := migrationASCII2DSolverResponse(failure.body)
		get.Status = failure.status
		if failure.transport {
			get.Error = "fixture"
		}
		in.SolverResponses = []migrationASCII2DResponse{migrationASCII2DSolverResponse(`{"status":"ok"}`), get, migrationASCII2DSolverResponse(`{"status":"ok"}`)}
		add("solver-public-"+failure.name, in)
	}
	for _, state := range []string{"nil", "canceled", "deadline"} {
		in := base()
		in.SearchContext = state
		in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
		add("acquired-session-context-"+state, in)
	}
	for _, status := range []int{301, 303, 307, 308} {
		in := base()
		upload := migrationASCII2DUpload()
		upload.Status = status
		in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), upload}
		add(fmt.Sprintf("upload-valid-location-status-%d", status), in)
	}
	for _, stage := range []string{"create", "get"} {
		for _, mode := range []string{"transport-failure", "http-failure", "cancel-transport", "read-failure"} {
			in := base()
			in.SolverURL = "http://solver.invalid"
			home := migrationASCII2DHome()
			home.Status = 403
			home.Headers = http.Header{"Cf-Mitigated": {"challenge"}}
			in.Responses = []migrationASCII2DResponse{home}
			response := migrationASCII2DSolverResponse(`{"status":"ok"}`)
			if stage == "get" {
				response.Body = migrationASCII2DSolution
			}
			switch mode {
			case "transport-failure":
				response.Error = "fixture"
			case "http-failure":
				response.Status = 500
			case "cancel-transport":
				response.Cancel = "wait-context"
			case "read-failure":
				response.ReadError = true
			}
			in.SolverResponses = []migrationASCII2DResponse{response}
			if stage == "get" {
				in.SolverResponses = []migrationASCII2DResponse{migrationASCII2DSolverResponse(`{"status":"ok"}`), response, migrationASCII2DSolverResponse(`{"status":"ok"}`)}
			}
			add("solver-"+stage+"-"+mode, in)
		}
	}
	in = base()
	in.Endpoint = "https://ascii2d.invalid/base"
	home := migrationASCII2DHome()
	home.Path = "/base"
	in.Providers = []reversesearch.Provider{reversesearch.ProviderASCII2DColor}
	in.Responses = []migrationASCII2DResponse{home, migrationASCII2DUpload(), migrationASCII2DResult(reversesearch.ProviderASCII2DColor)}
	add("endpoint-base-path-and-origin-form-headers", in)
	in = base()
	in.CloseBefore = true
	in.Responses = []migrationASCII2DResponse{migrationASCII2DHome(), migrationASCII2DUpload()}
	add("public-upload-after-close-without-challenge", in)
	return rows
}

func TestMigrationASCII2DProtocolFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..")
	for path, want := range migrationASCII2DSources {
		body, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(body)
		if hex.EncodeToString(digest[:]) != want {
			t.Fatalf("frozen source changed: %s", path)
		}
	}
	rows := []migrationASCII2DRow{}
	seen := map[string]bool{}
	for _, row := range migrationASCII2DRows() {
		if seen[row.Name] {
			t.Fatalf("duplicate row: %s", row.Name)
		}
		seen[row.Name] = true
		t.Run(row.Name, func(t *testing.T) { rows = append(rows, migrationASCII2DRun(t, row)) })
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{"schema": 1, "source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "published_baseline": "99246d8bfb8712caa0903426953423850836cefe", "go_version": runtime.Version(), "source_sha256": migrationASCII2DSources, "cases": rows, "private_cache_cases": migrationASCII2DCacheRows(t), "evidence": "Actual frozen New/Preflight/Upload/Session.Search/Close, HTTP redirects, HTML parser and challenge solver recovery through original http.Client and owned synthetic RoundTripper ports. No helper-output projection.", "limitations": []string{"No third-party request, credential, real image upload, external browser, OS registration, host trust/settings, native TLS or HTTP/2 run occurs.", "Chrome_146 profile and default direct solver transport remain source-anchored requirements; injected ordinary transports are not native fingerprint evidence. Denied supplemental multiplex/unfinished HEAD/upload native probes are not retried.", "The generated multipart boundary token alone is masked at its declared Content-Type and delimiter positions. Every Content-Type parameter/key, boundary length/lowercase-hex/decoded-byte properties, all parsed part header keys/values, actual ordered raw header blocks, delimiters and closing terminator are observed. Complete wire equality validates exact fixed Go framing with no preamble or epilogue. Small complete wire bytes and payload bytes are recorded with only boundary tokens masked; large owned zero-padded payloads retain exact byte count/SHA256 and undergo the same full wire equality assertion. Intentional CloseUpload cases do not read or claim complete multipart body framing.", "Nil and zero client/session receiver cases call public methods; CloseFromSession reaches private session client solely to observe source lifecycle sharing and is explicitly named. Solver dependency ports receive a deterministic synthetic session name after New; random-name generation remains source-anchored. Canceled solves are allowed to finish before final Close observations.", "Private cache cases separately freeze actual solverStateCache helper state and barriers; they are not projected as public Client outputs. Native connection/decompression/proxy behavior and platform verification remain unexecuted."}}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "reverse-ascii2d.json")
	if *migrationCaptureASCII2D != "" {
		path = *migrationCaptureASCII2D
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
	} else {
		want, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(data, want) {
			t.Fatal("actual frozen ASCII2D public protocol differs from fixture")
		}
	}
	t.Logf("ASCII2D public protocol rows=%d", len(rows))
}
