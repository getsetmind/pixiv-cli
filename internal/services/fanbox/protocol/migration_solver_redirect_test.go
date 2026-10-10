package protocol

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

var migrationUpdateSolverRedirect = flag.Bool("migration-update-fanbox-solver-redirect", false, "capture fixed Go injected solver malformed redirect cleanup")

type migrationSolverRedirectInput struct {
	Name       string `json:"name"`
	Location   string `json:"location"`
	CloseError string `json:"close_error"`
}

type migrationSolverRedirectRequest struct {
	Method string      `json:"method"`
	URL    string      `json:"url"`
	Header http.Header `json:"header"`
	Body   string      `json:"body"`
}

type migrationSolverRedirectCallback struct {
	Method             string `json:"method"`
	URL                string `json:"url"`
	ViaCount           int    `json:"via_count"`
	ResponseStatus     int    `json:"response_status"`
	ResponseCloseCount int    `json:"response_close_count"`
}

type migrationSolverRedirectCause struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

type migrationSolverRedirectError struct {
	Chain              []migrationSolverRedirectCause `json:"chain"`
	URL                string                         `json:"url"`
	Operation          string                         `json:"operation"`
	Timeout            bool                           `json:"timeout"`
	Temporary          bool                           `json:"temporary"`
	Canceled           bool                           `json:"canceled"`
	Deadline           bool                           `json:"deadline"`
	CloseErrorRetained bool                           `json:"close_error_retained"`
	SolverUnavailable  bool                           `json:"solver_unavailable"`
	SolverFailed       bool                           `json:"solver_failed"`
	MalformedSolver    bool                           `json:"malformed_solver"`
}

type migrationSolverRedirectState struct {
	UserAgent string `json:"user_agent"`
	Clearance string `json:"clearance"`
	ExpiresAt string `json:"expires_at"`
	HasExpiry bool   `json:"has_expiry"`
}

type migrationSolverRedirectResult struct {
	Requests              []migrationSolverRedirectRequest  `json:"requests"`
	Callbacks             []migrationSolverRedirectCallback `json:"callbacks"`
	Trace                 []string                          `json:"trace"`
	ResponseReturned      bool                              `json:"response_returned"`
	ResponseStatus        int                               `json:"response_status"`
	ResponseCloseAtReturn int                               `json:"response_close_at_return"`
	ResponseCloseCount    int                               `json:"response_close_count"`
	ResponseReadCount     int                               `json:"response_read_count"`
	ResponseBytesRead     int                               `json:"response_bytes_read"`
	ConsumerCloseError    string                            `json:"consumer_close_error"`
	Error                 migrationSolverRedirectError      `json:"error"`
	State                 *migrationSolverRedirectState     `json:"state"`
	DiagnosticReason      string                            `json:"diagnostic_reason"`
}

type migrationSolverRedirectCase struct {
	Input   migrationSolverRedirectInput  `json:"input"`
	Control migrationSolverRedirectResult `json:"control"`
	Solver  migrationSolverRedirectResult `json:"solver"`
}

type migrationSolverRedirectSource struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type migrationSolverRedirectFixture struct {
	Reference      string                          `json:"reference"`
	GoVersion      string                          `json:"go_version"`
	Sources        []migrationSolverRedirectSource `json:"sources"`
	ClientSHA256   string                          `json:"net_http_client_sha256"`
	ResponseSHA256 string                          `json:"net_http_response_sha256"`
	Evidence       string                          `json:"evidence"`
	Limitations    []string                        `json:"limitations"`
	Cases          []migrationSolverRedirectCase   `json:"cases"`
}

type migrationSolverRedirectBody struct {
	reader     *strings.Reader
	closeError error
	trace      *[]string
	closeCount int
	readCount  int
	bytesRead  int
}

func (body *migrationSolverRedirectBody) Read(buffer []byte) (int, error) {
	body.readCount++
	*body.trace = append(*body.trace, "response.read")
	count, err := body.reader.Read(buffer)
	body.bytesRead += count
	return count, err
}

func (body *migrationSolverRedirectBody) Close() error {
	body.closeCount++
	*body.trace = append(*body.trace, "response.close")
	return body.closeError
}

type migrationSolverRedirectTransport func(*http.Request) (*http.Response, error)

func (transport migrationSolverRedirectTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	return transport(request)
}

func TestMigrationFanboxSolverRedirectClosesMalformedLocationBeforePolicy(t *testing.T) {
	const reference = "4b4426487ef18bed276706daec385e0d0a6979f9"
	const clientSHA = "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9"
	const responseSHA = "0b32a0b4ee51e00e3410764f3b2ffe1163dccb9cad548b097f47db85893a2f44"
	sources := []migrationSolverRedirectSource{
		{"internal/services/fanbox/protocol/solver.go", "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea"},
		{"internal/services/fanbox/protocol/protocol.go", "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3"},
		{"internal/services/fanbox/protocol/cookie.go", "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9"},
		{"internal/shared/diagnostics/diagnostics.go", "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76"},
	}
	root := filepath.Join("..", "..", "..", "..")
	for _, source := range sources {
		current, err := os.ReadFile(filepath.Join(root, source.Path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", reference+":"+source.Path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read fixed Go source %s: %v", source.Path, err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(frozen)) != source.SHA256 || !bytes.Equal(current, frozen) {
			t.Fatalf("%s differs from fixed Go source", source.Path)
		}
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("Go version = %s, want pinned go1.27.1", runtime.Version())
	}
	clientSource, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", "net", "http", "client.go"))
	if err != nil {
		t.Fatal(err)
	}
	if fmt.Sprintf("%x", sha256.Sum256(clientSource)) != clientSHA {
		t.Fatal("net/http client source differs from pinned Go toolchain")
	}
	responseSource, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", "net", "http", "response.go"))
	if err != nil {
		t.Fatal(err)
	}
	if fmt.Sprintf("%x", sha256.Sum256(responseSource)) != responseSHA {
		t.Fatal("net/http response source differs from pinned Go toolchain")
	}
	fixture := migrationSolverRedirectFixture{
		Reference: reference, GoVersion: runtime.Version(), Sources: sources, ClientSHA256: clientSHA, ResponseSHA256: responseSHA,
		Evidence: "Byte-identical fixed Go solver and Session constructor, genuine injected standard *http.Client.Do, synthetic owned RoundTripper responses, tracked response Read/Close and redirect policy. Direct Client.Do and actual solveFlareSolverr receive the same three inputs. Every private solverState field is captured.",
		Limitations: []string{
			"No external network, account, browser, host trust or native TLS/HTTP2 operation is performed.",
			"The direct solver control boundary is tested; challenge detection, shared waiters, business replay and public SDK error mapping remain separate contracts.",
			"Synthetic response Close errors are captured at the control boundary; physical native stream cancellation is not established.",
		},
		Cases: []migrationSolverRedirectCase{},
	}
	for _, input := range []migrationSolverRedirectInput{
		{Name: "malformed_location", Location: "https://%"},
		{Name: "malformed_location_close_error", Location: "https://%", CloseError: "solver redirect close failure"},
		{Name: "valid_location_no_follow", Location: "https://solver.example/next"},
	} {
		t.Run(input.Name, func(t *testing.T) {
			control := migrationCaptureSolverRedirect(t, input, false)
			solver := migrationCaptureSolverRedirect(t, input, true)
			if control.ResponseCloseCount != 1 || solver.ResponseCloseCount != 1 {
				t.Fatalf("response close counts = %d / %d, want 1 / 1", control.ResponseCloseCount, solver.ResponseCloseCount)
			}
			if input.Location == "https://%" {
				if len(control.Callbacks) != 0 || len(solver.Callbacks) != 0 || !solver.Error.SolverUnavailable {
					t.Fatal("malformed Location must close before redirect policy and map to solver unavailable")
				}
			} else if len(control.Callbacks) != 1 || len(solver.Callbacks) != 1 || !solver.Error.SolverFailed || control.ResponseCloseAtReturn != 0 {
				t.Fatal("valid no-follow Location must reach policy and return the unclosed redirect before solver classification")
			}
			fixture.Cases = append(fixture.Cases, migrationSolverRedirectCase{input, control, solver})
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-redirect.json")
	if *migrationUpdateSolverRedirect {
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
		t.Fatal("injected solver redirect cleanup differs from fixed Go contract")
	}
}

func migrationCaptureSolverRedirect(t *testing.T, input migrationSolverRedirectInput, solver bool) migrationSolverRedirectResult {
	t.Helper()
	result := migrationSolverRedirectResult{
		Requests: []migrationSolverRedirectRequest{}, Callbacks: []migrationSolverRedirectCallback{}, Trace: []string{},
	}
	var closeError error
	if input.CloseError != "" {
		closeError = errors.New(input.CloseError)
	}
	body := &migrationSolverRedirectBody{reader: strings.NewReader("owned solver redirect response"), closeError: closeError, trace: &result.Trace}
	client := &http.Client{
		Transport: migrationSolverRedirectTransport(func(request *http.Request) (*http.Response, error) {
			result.Trace = append(result.Trace, "transport.request")
			if len(result.Requests) != 0 {
				return nil, errors.New("unexpected solver redirect follow")
			}
			var payload []byte
			if request.Body != nil {
				defer request.Body.Close()
				var err error
				payload, err = io.ReadAll(request.Body)
				if err != nil {
					return nil, err
				}
			}
			result.Requests = append(result.Requests, migrationSolverRedirectRequest{request.Method, request.URL.String(), request.Header.Clone(), string(payload)})
			return &http.Response{
				StatusCode: http.StatusFound, Header: http.Header{"Location": {input.Location}},
				Body: body, ContentLength: int64(body.reader.Len()), Request: request,
			}, nil
		}),
		CheckRedirect: func(request *http.Request, via []*http.Request) error {
			result.Trace = append(result.Trace, "redirect.policy")
			result.Callbacks = append(result.Callbacks, migrationSolverRedirectCallback{
				request.Method, request.URL.String(), len(via), request.Response.StatusCode, body.closeCount,
			})
			return http.ErrUseLastResponse
		},
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var operationError error
	if solver {
		session, err := NewSessionWithOptions("FANBOXSESSID=owned-solver-redirect-canary", SessionOptions{
			HTTPClient: &http.Client{Transport: migrationSolverRedirectTransport(func(*http.Request) (*http.Response, error) {
				return nil, errors.New("unexpected business request")
			})},
			SolverHTTPClient: client, FlareSolverr: &FlareSolverrOptions{URL: "http://solver.example"},
		})
		if err != nil {
			t.Fatal(err)
		}
		state, err := session.solveFlareSolverr(ctx)
		operationError = err
		result.Trace = append(result.Trace, "solver.return")
		result.State = &migrationSolverRedirectState{state.userAgent, state.clearance, state.expiresAt.Format(time.RFC3339Nano), state.hasExpiry}
		result.DiagnosticReason = string(solverDiagnosticReason(err))
	} else {
		request, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://solver.example/v1", strings.NewReader(`{"cmd":"request.get","url":"https://www.fanbox.cc/"}`))
		if err != nil {
			t.Fatal(err)
		}
		request.Header.Set("Content-Type", "application/json")
		request.Header.Set("Accept", "application/json")
		response, err := client.Do(request)
		operationError = err
		result.Trace = append(result.Trace, "client.return")
		result.ResponseReturned = response != nil
		if response != nil {
			result.ResponseStatus = response.StatusCode
		}
		result.ResponseCloseAtReturn = body.closeCount
		if response != nil {
			result.Trace = append(result.Trace, "consumer.close")
			if err := response.Body.Close(); err != nil {
				result.ConsumerCloseError = err.Error()
			}
		}
	}
	if solver {
		result.ResponseCloseAtReturn = body.closeCount
	}
	result.ResponseCloseCount, result.ResponseReadCount, result.ResponseBytesRead = body.closeCount, body.readCount, body.bytesRead
	result.Error = migrationSolverRedirectError{
		Chain: []migrationSolverRedirectCause{}, Canceled: errors.Is(operationError, context.Canceled), Deadline: errors.Is(operationError, context.DeadlineExceeded),
		CloseErrorRetained: closeError != nil && errors.Is(operationError, closeError), SolverUnavailable: errors.Is(operationError, ErrSolverUnavailable),
		SolverFailed: errors.Is(operationError, ErrSolverFailed), MalformedSolver: errors.Is(operationError, ErrMalformedSolverResponse),
	}
	for cause := operationError; cause != nil; cause = errors.Unwrap(cause) {
		result.Error.Chain = append(result.Error.Chain, migrationSolverRedirectCause{fmt.Sprintf("%T", cause), cause.Error()})
	}
	var urlError *url.Error
	if errors.As(operationError, &urlError) {
		result.Error.URL, result.Error.Operation = urlError.URL, urlError.Op
		result.Error.Timeout, result.Error.Temporary = urlError.Timeout(), urlError.Temporary()
	}
	return result
}
