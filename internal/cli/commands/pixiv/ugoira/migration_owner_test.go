package ugoira

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
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

var migrationUpdateUgoiraOwner = flag.Bool("migration-update-ugoira-owner", false, "capture the frozen standalone ugoira command contract")

const migrationUgoiraOwnerReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type migrationUgoiraOwnerInput struct {
	Name           string                   `json:"name"`
	Args           []string                 `json:"args"`
	ConfiguredJSON bool                     `json:"configured_json"`
	JSONError      string                   `json:"json_error,omitempty"`
	PoolError      string                   `json:"pool_error,omitempty"`
	Missing        string                   `json:"missing,omitempty"`
	Writer         string                   `json:"writer"`
	ArtworkBody    json.RawMessage          `json:"artwork_body"`
	MetadataBody   json.RawMessage          `json:"metadata_body"`
	ArtworkStatus  int                      `json:"artwork_status"`
	MetadataStatus int                      `json:"metadata_status"`
	TypedMetadata  *pixiv.UgoiraMetadataDTO `json:"typed_metadata,omitempty"`
}

type migrationUgoiraOwnerRequest struct {
	Method           string `json:"method"`
	URI              string `json:"uri"`
	Authorization    string `json:"authorization"`
	ContextInherited bool   `json:"context_inherited"`
}

type migrationUgoiraOwnerWrite struct {
	Input         string `json:"input"`
	Count         int    `json:"count"`
	Error         string `json:"error"`
	UnderCallback bool   `json:"under_callback"`
	OpenBodies    int    `json:"open_bodies"`
}

type migrationUgoiraOwnerResult struct {
	Error             string                        `json:"error"`
	ErrorDTO          json.RawMessage               `json:"error_dto"`
	Usage             bool                          `json:"usage"`
	Stdout            string                        `json:"stdout"`
	HelpOutput        string                        `json:"help_output"`
	Diagnostics       string                        `json:"diagnostics"`
	JSONOverrides     []*bool                       `json:"json_overrides"`
	ProxyOverrides    []*string                     `json:"proxy_overrides"`
	Requests          []migrationUgoiraOwnerRequest `json:"requests"`
	Events            []string                      `json:"events"`
	Writes            []migrationUgoiraOwnerWrite   `json:"writes"`
	CallbackCommitted []bool                        `json:"callback_committed"`
	InputReads        int                           `json:"input_reads"`
	BodyCloses        int                           `json:"body_closes"`
}

type migrationUgoiraOwnerCase struct {
	Input  migrationUgoiraOwnerInput  `json:"input"`
	Result migrationUgoiraOwnerResult `json:"result"`
}

type migrationUgoiraOwnerUsage struct{ error }

func (err migrationUgoiraOwnerUsage) Unwrap() error { return err.error }

type migrationUgoiraOwnerTransport func(*http.Request) (*http.Response, error)

func (transport migrationUgoiraOwnerTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	return transport(request)
}

type migrationUgoiraOwnerInputReader struct{ reads *int }

func (input migrationUgoiraOwnerInputReader) Read([]byte) (int, error) {
	*input.reads++
	panic("standalone ugoira must not read stdin")
}

type migrationUgoiraOwnerBody struct {
	io.Reader
	close func()
}

func (body migrationUgoiraOwnerBody) Close() error {
	body.close()
	return nil
}

type migrationUgoiraOwnerWriter struct {
	mode   string
	result *migrationUgoiraOwnerResult
	active *bool
	bodies *int
	output bytes.Buffer
}

func (writer *migrationUgoiraOwnerWriter) Write(input []byte) (int, error) {
	count := len(input)
	var err error
	switch writer.mode {
	case "short-nil":
		count /= 2
	case "zero-nil":
		count = 0
	case "fail-first":
		count, err = 0, errors.New("fixture writer failure")
	case "partial-error":
		count, err = min(5, count), errors.New("fixture writer failure")
	case "fail-fourth":
		if len(writer.result.Writes) == 3 {
			count, err = 0, errors.New("fixture writer failure")
		}
	case "broken-pipe":
		count, err = 0, syscall.EPIPE
	}
	write := migrationUgoiraOwnerWrite{Input: string(input), Count: count, UnderCallback: *writer.active, OpenBodies: *writer.bodies}
	if err != nil {
		write.Error = err.Error()
	}
	writer.result.Writes = append(writer.result.Writes, write)
	writer.result.Events = append(writer.result.Events, "write")
	_, _ = writer.output.Write(input[:count])
	return count, err
}

func migrationUgoiraOwnerSources(t *testing.T, root string) map[string]string {
	t.Helper()
	paths := []string{
		"internal/cli/commands/pixiv/ugoira/ugoira.go",
		"internal/cli/commands/lifecycle.go",
		"sdk/pixiv/reference.go", "sdk/pixiv/pixiv.go", "sdk/pixiv/ops_artwork.go",
		"sdk/pixiv/map_artwork.go", "sdk/pixiv/ugoira.go", "sdk/pixiv/dto.go",
		"sdk/pixiv/errors.go", "sdk/pixiv/resource.go", "sdk/error.go",
		"sdk/resource_dto.go", "sdk/ref.go",
		"internal/services/pixiv/endpoint/artwork/detail/detail.go",
		"internal/services/pixiv/appapi/appapi.go",
		"internal/services/pixiv/protocol/required.go",
		"internal/services/pixiv/protocol/failure.go",
	}
	identities := map[string]string{}
	for _, path := range paths {
		working, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", migrationUgoiraOwnerReference+":"+path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read frozen %s: %v", path, err)
		}
		if !bytes.Equal(working, frozen) {
			t.Fatalf("%s differs from frozen Go reference", path)
		}
		digest := sha256.Sum256(frozen)
		identities[path] = hex.EncodeToString(digest[:])
	}
	if identities[paths[0]] != "c4439b144e472b56d39421e465c4ffdc5d3e4b8cfc976a1cc2e5ae54b69e1320" {
		t.Fatal("frozen standalone ugoira owner identity changed")
	}
	return identities
}

func migrationUgoiraOwnerInputs() []migrationUgoiraOwnerInput {
	artwork := json.RawMessage(`{"illust":{"id":42,"type":"ugoira","create_date":"2024-01-02T03:04:05+00:00","user":{"id":9,"name":"fixture"}}}`)
	metadata := json.RawMessage(`{"ugoira_metadata":{"zip_urls":{"medium":"https://i.pximg.net/medium.zip?fixture=secret","original":"https://i.pximg.net/original.zip?fixture=secret"},"frames":[{"file":"z.jpg","delay":100},{"file":"dir/a.jpg","delay":-100},{"file":"zero.jpg","delay":0}]}}`)
	newInput := func(name string, args ...string) migrationUgoiraOwnerInput {
		return migrationUgoiraOwnerInput{Name: name, Args: append([]string{}, args...), Writer: "normal", ArtworkBody: artwork, MetadataBody: metadata, ArtworkStatus: 200, MetadataStatus: 200}
	}
	inputs := []migrationUgoiraOwnerInput{}
	argumentCases := []struct {
		name string
		args []string
	}{
		{"human", []string{"42"}}, {"json", []string{"42", "--json"}}, {"short-json", []string{"42", "-j"}},
		{"missing-argument", []string{}}, {"extra-argument", []string{"42", "43"}},
		{"trim-unicode", []string{"\u2003\u300042\u00a0"}}, {"signed-integer", []string{"+42"}}, {"zero-padded-integer", []string{"00042"}},
		{"maximum-integer", []string{"9223372036854775807"}}, {"canonical-url", []string{"https://www.pixiv.net/artworks/42"}},
		{"locale-url", []string{"https://www.pixiv.net/en/artworks/42?utm=fixture#frame"}}, {"legacy-url", []string{"https://www.pixiv.net/member_illust.php?mode=medium&illust_id=42"}},
		{"bare-host-url", []string{"https://pixiv.net/artworks/42"}}, {"zero-id", []string{"0"}}, {"negative-id", []string{"-1"}},
		{"negative-after-separator", []string{"--", "-1"}}, {"overflow-id", []string{"9223372036854775808"}}, {"empty-source", []string{""}},
		{"wrong-user-url", []string{"https://www.pixiv.net/users/42"}}, {"wrong-novel-url", []string{"https://www.pixiv.net/novel/show.php?id=42"}},
		{"wrong-series-url", []string{"https://www.pixiv.net/user/9/series/42"}}, {"foreign-url", []string{"https://fixture.invalid/artworks/42"}},
		{"insecure-url", []string{"http://www.pixiv.net/artworks/42"}}, {"invalid-url", []string{"https://www.pixiv.net/artworks/%"}},
		{"unknown-ndjson", []string{"42", "--ndjson"}}, {"unknown-format", []string{"42", "--format=gif"}}, {"unknown-account", []string{"42", "--account=9"}},
		{"unknown-quality", []string{"42", "--quality=original"}}, {"unknown-pages", []string{"42", "--pages=all"}}, {"unknown-output", []string{"42", "--output=fixture"}},
		{"missing-proxy-value", []string{"42", "--proxy"}}, {"invalid-json-value", []string{"42", "--json=invalid"}}, {"invalid-no-proxy-value", []string{"42", "--no-proxy=invalid"}},
		{"json-explicit-false", []string{"42", "--json=false"}}, {"json-repeated-last-false", []string{"42", "--json", "--json=false"}},
		{"json-bool-one", []string{"42", "--json=1"}}, {"json-bool-zero", []string{"42", "--json=0"}},
		{"json-bool-lower-t", []string{"42", "--json=t"}}, {"json-bool-upper-t", []string{"42", "--json=T"}},
		{"json-bool-upper-true", []string{"42", "--json=TRUE"}}, {"json-bool-title-true", []string{"42", "--json=True"}},
		{"json-bool-lower-f", []string{"42", "--json=f"}}, {"json-bool-upper-f", []string{"42", "--json=F"}},
		{"json-bool-upper-false", []string{"42", "--json=FALSE"}}, {"json-bool-title-false", []string{"42", "--json=False"}},
		{"json-repeated-last-true", []string{"42", "--json=false", "-j"}}, {"proxy", []string{"42", "--proxy=http://fixture.invalid:8080"}},
		{"proxy-empty", []string{"42", "--proxy="}}, {"proxy-validation-deferred", []string{"42", "--proxy=invalid-proxy-text"}},
		{"no-proxy", []string{"42", "--no-proxy"}}, {"no-proxy-false", []string{"42", "--no-proxy=false"}},
		{"no-proxy-bool-one", []string{"42", "--no-proxy=1"}}, {"no-proxy-bool-zero", []string{"42", "--no-proxy=0"}},
		{"no-proxy-bool-upper-t", []string{"42", "--no-proxy=T"}}, {"no-proxy-bool-title-false", []string{"42", "--no-proxy=False"}},
		{"no-proxy-last-false", []string{"42", "--no-proxy", "--no-proxy=false"}},
		{"proxy-no-proxy-conflict", []string{"42", "--proxy=http://fixture", "--no-proxy"}},
		{"proxy-no-proxy-false-conflict", []string{"42", "--proxy=", "--no-proxy=false"}},
		{"proxy-conflict-before-source", []string{"invalid", "--proxy=http://fixture", "--no-proxy=false"}},
		{"proxy-conflict-before-missing-argument", []string{"--proxy=http://fixture", "--no-proxy=false"}},
		{"help-long", []string{"--help"}}, {"help-short", []string{"-h"}},
	}
	for _, current := range argumentCases {
		inputs = append(inputs, newInput(current.name, current.args...))
	}
	for _, current := range []struct {
		name string
		args []string
	}{
		{"configured-json", []string{"42"}}, {"configured-json-explicit-false", []string{"42", "--json=false"}},
	} {
		input := newInput(current.name, current.args...)
		input.ConfiguredJSON = true
		inputs = append(inputs, input)
	}
	for _, kind := range []string{"illust", "manga", "novel", "", "unexpected"} {
		input := newInput("kind-preflight-"+kind, "42", "--json")
		input.ArtworkBody = json.RawMessage(fmt.Sprintf(`{"illust":{"id":42,"type":%q,"create_date":"2024-01-02T03:04:05+00:00"}}`, kind))
		inputs = append(inputs, input)
	}
	for _, quality := range []string{"original", "medium"} {
		input := newInput("only-"+quality, "42", "--json")
		input.MetadataBody = json.RawMessage(fmt.Sprintf(`{"ugoira_metadata":{"zip_urls":{%q:"https://i.pximg.net/archive.zip"},"frames":[{"file":"0.jpg","delay":80}]}}`, quality))
		inputs = append(inputs, input)
	}
	escaping := json.RawMessage(`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/archive.zip?fixture=secret"},"frames":[{"file":"<>&\u2028\u2029.jpg","delay":-9223372036854775808},{"file":"line\n\t\u001b[31m.jpg","delay":9223372036854775807},{"file":"missing.jpg"},{"file":"null.jpg","delay":null}]}}`)
	for _, mode := range []string{"human", "json"} {
		input := newInput("escaping-signed-all-frames-"+mode, "42")
		if mode == "json" {
			input.Args = append(input.Args, "--json")
		}
		input.MetadataBody = escaping
		inputs = append(inputs, input)
	}
	for _, stage := range []string{"artwork", "metadata"} {
		input := newInput(stage+"-malformed", "42", "--json")
		if stage == "artwork" {
			input.ArtworkBody = json.RawMessage(`{"illust":null}`)
		} else {
			input.MetadataBody = json.RawMessage(`{"ugoira_metadata":null}`)
		}
		inputs = append(inputs, input)
		input = newInput(stage+"-not-found", "42", "--json")
		if stage == "artwork" {
			input.ArtworkStatus = 404
		} else {
			input.MetadataStatus = 404
		}
		inputs = append(inputs, input)
	}
	for _, mode := range []string{"human", "json"} {
		for _, writer := range []string{"short-nil", "zero-nil", "fail-first", "partial-error", "fail-fourth", "broken-pipe"} {
			input := newInput("writer-"+writer+"-"+mode, "42")
			if mode == "json" {
				input.Args = append(input.Args, "--json")
			}
			input.Writer = writer
			inputs = append(inputs, input)
		}
	}
	input := newInput("json-resolver-error", "42", "--json")
	input.JSONError = "fixture JSON resolver failure"
	inputs = append(inputs, input)
	input = newInput("source-before-json-resolver-error", "invalid", "--json")
	input.JSONError = "fixture JSON resolver failure"
	inputs = append(inputs, input)
	input = newInput("pool-error", "42")
	input.PoolError = "fixture pool failure"
	inputs = append(inputs, input)
	for _, missing := range []string{"json-resolver", "pool", "fetchers"} {
		input := newInput("missing-"+missing, "42")
		input.Missing = missing
		inputs = append(inputs, input)
	}
	for _, mode := range []string{"human", "json"} {
		input := newInput("typed-stable-archive-groups-"+mode, "42")
		if mode == "json" {
			input.Args = append(input.Args, "--json")
		}
		value := pixiv.UgoiraMetadata{ArtworkID: 42, Frames: []pixiv.UgoiraFrame{{Filename: "frame.jpg", DelayMilliseconds: -7}}}
		for index, quality := range []string{"future-a", "original", "medium", "original", "future-b", "medium"} {
			ref, err := sdk.NewResourceRef("pixiv", []byte(fmt.Sprintf(`{"k":"ugoira_archive","id":42,"p":-1,"v":"fixture-%d"}`, index)))
			if err != nil {
				panic(err)
			}
			value.Archives = append(value.Archives, pixiv.UgoiraArchive{Quality: pixiv.UgoiraQuality(quality), Resource: sdk.Resource{Ref: ref, URL: "https://i.pximg.net/fixture.zip?secret=typed", RequestHeaders: map[string]string{"Referer": "https://www.pixiv.net/"}}})
		}
		dto := pixiv.ToUgoiraMetadataDTO(value)
		input.TypedMetadata = &dto
		inputs = append(inputs, input)
	}
	return inputs
}

func migrationUgoiraOwnerExecute(t *testing.T, input migrationUgoiraOwnerInput) migrationUgoiraOwnerResult {
	t.Helper()
	result := migrationUgoiraOwnerResult{JSONOverrides: []*bool{}, ProxyOverrides: []*string{}, Requests: []migrationUgoiraOwnerRequest{}, Events: []string{}, Writes: []migrationUgoiraOwnerWrite{}, CallbackCommitted: []bool{}}
	active, openBodies := false, 0
	writer := &migrationUgoiraOwnerWriter{mode: input.Writer, result: &result, active: &active, bodies: &openBodies}
	var help, diagnostics bytes.Buffer
	type contextKey struct{}
	ctx := context.WithValue(context.Background(), contextKey{}, "fixture context")
	dependencies := Dependencies{
		Output: writer,
		UsageError: func(err error) error {
			return migrationUgoiraOwnerUsage{err}
		},
		JSONOut: func(override *bool) (bool, error) {
			result.Events = append(result.Events, "json-resolve")
			if override == nil {
				result.JSONOverrides = append(result.JSONOverrides, nil)
			} else {
				value := *override
				result.JSONOverrides = append(result.JSONOverrides, &value)
			}
			if input.JSONError != "" {
				return false, errors.New(input.JSONError)
			}
			if override != nil {
				return *override, nil
			}
			return input.ConfiguredJSON, nil
		},
		Pooled: func(ctx context.Context, request Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			result.Events = append(result.Events, "pool-enter")
			if request.HTTPSProxyOverride == nil {
				result.ProxyOverrides = append(result.ProxyOverrides, nil)
			} else {
				value := *request.HTTPSProxyOverride
				result.ProxyOverrides = append(result.ProxyOverrides, &value)
			}
			if input.PoolError != "" {
				return errors.New(input.PoolError)
			}
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationUgoiraOwnerTransport(func(request *http.Request) (*http.Response, error) {
				if !active || request.Method != http.MethodGet || request.URL.Host != "app-api.pixiv.net" || request.Header.Get("Authorization") != "Bearer fixture-access" {
					t.Fatalf("unexpected owner request: %s %s", request.Method, request.URL)
				}
				result.Requests = append(result.Requests, migrationUgoiraOwnerRequest{Method: request.Method, URI: request.URL.RequestURI(), Authorization: request.Header.Get("Authorization"), ContextInherited: request.Context().Value(contextKey{}) == "fixture context"})
				body, status, stage := input.ArtworkBody, input.ArtworkStatus, "artwork"
				switch request.URL.Path {
				case "/v1/illust/detail":
				case "/v1/ugoira/metadata":
					body, status, stage = input.MetadataBody, input.MetadataStatus, "metadata"
				default:
					t.Fatalf("unexpected standalone ugoira route: %s", request.URL.Path)
				}
				result.Events = append(result.Events, "http-"+stage)
				openBodies++
				return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"application/json"}}, Body: migrationUgoiraOwnerBody{Reader: bytes.NewReader(body), close: func() {
					openBodies--
					result.BodyCloses++
					result.Events = append(result.Events, "body-close-"+stage)
				}}, Request: request}, nil
			})}})
			if err != nil {
				return err
			}
			result.Events = append(result.Events, "client-created", "callback-enter")
			active = true
			committed, err := invoke(ctx, client)
			active = false
			result.CallbackCommitted = append(result.CallbackCommitted, committed)
			result.Events = append(result.Events, fmt.Sprintf("callback-return:%t", committed), "pool-release")
			client.CloseIdleConnections()
			return err
		},
		FetchArtwork: func(ctx context.Context, client *pixiv.Client, id int64) (pixiv.Artwork, error) {
			return client.Artwork(ctx, pixiv.ArtworkRequest{ArtworkID: id})
		},
		FetchUgoiraMetadata: func(ctx context.Context, client *pixiv.Client, id int64) (pixiv.UgoiraMetadata, error) {
			if input.TypedMetadata == nil {
				return client.UgoiraMetadata(ctx, pixiv.UgoiraMetadataRequest{ArtworkID: id})
			}
			result.Events = append(result.Events, "typed-metadata")
			value := pixiv.UgoiraMetadata{ArtworkID: input.TypedMetadata.ArtworkID}
			for _, archive := range input.TypedMetadata.Archives {
				resource := sdk.Resource{}
				if archive.Resource != nil {
					var err error
					resource.Ref, err = sdk.ParseResourceRef(archive.Resource.Ref)
					if err != nil {
						t.Fatal(err)
					}
				}
				value.Archives = append(value.Archives, pixiv.UgoiraArchive{Quality: archive.Quality, Resource: resource})
			}
			for _, frame := range input.TypedMetadata.Frames {
				value.Frames = append(value.Frames, pixiv.UgoiraFrame{Filename: frame.Filename, DelayMilliseconds: frame.DelayMilliseconds})
			}
			return value, nil
		},
	}
	switch input.Missing {
	case "json-resolver":
		dependencies.JSONOut = nil
	case "pool":
		dependencies.Pooled = nil
	case "fetchers":
		dependencies.FetchArtwork, dependencies.FetchUgoiraMetadata = nil, nil
	}
	command := New(dependencies)
	root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
	root.AddCommand(command)
	root.SetOut(&help)
	root.SetErr(&diagnostics)
	root.SetIn(migrationUgoiraOwnerInputReader{reads: &result.InputReads})
	root.SetArgs(append([]string{"ugoira"}, input.Args...))
	err := root.ExecuteContext(ctx)
	if err != nil {
		result.Error = err.Error()
		var usage migrationUgoiraOwnerUsage
		result.Usage = errors.As(err, &usage)
		var classified *sdk.Error
		if errors.As(err, &classified) {
			result.ErrorDTO, err = json.Marshal(classified)
			if err != nil {
				t.Fatal(err)
			}
		}
	}
	result.Stdout, result.HelpOutput, result.Diagnostics = writer.output.String(), help.String(), diagnostics.String()
	if active || openBodies != 0 || result.InputReads != 0 {
		t.Fatalf("owner leaked callback/body or read stdin: %s", input.Name)
	}
	for _, committed := range result.CallbackCommitted {
		if committed {
			t.Fatalf("standalone ugoira callback reported output committed: %s", input.Name)
		}
	}
	for _, write := range result.Writes {
		if !write.UnderCallback || write.OpenBodies != 0 {
			t.Fatalf("output escaped pooled callback or retained body: %s", input.Name)
		}
	}
	if strings.Contains(result.Stdout, "fixture=secret") || strings.Contains(result.Stdout, "Bearer fixture-access") || strings.Contains(result.Stdout, "i.pximg.net") {
		t.Fatalf("standalone ugoira stdout leaked runtime data: %s", input.Name)
	}
	return result
}

func TestMigrationUgoiraOwnerMatchesFrozenCommand(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..", "..")
	sources := migrationUgoiraOwnerSources(t, root)
	command := New(Dependencies{})
	flags := []struct {
		Name        string `json:"name"`
		Shorthand   string `json:"shorthand"`
		Type        string `json:"type"`
		Default     string `json:"default"`
		NoOptDefVal string `json:"no_opt_default"`
		Usage       string `json:"usage"`
	}{}
	command.InitDefaultHelpFlag()
	command.Flags().VisitAll(func(value *pflag.Flag) {
		flags = append(flags, struct {
			Name        string `json:"name"`
			Shorthand   string `json:"shorthand"`
			Type        string `json:"type"`
			Default     string `json:"default"`
			NoOptDefVal string `json:"no_opt_default"`
			Usage       string `json:"usage"`
		}{value.Name, value.Shorthand, value.Value.Type(), value.DefValue, value.NoOptDefVal, value.Usage})
	})
	cases := []migrationUgoiraOwnerCase{}
	for _, input := range migrationUgoiraOwnerInputs() {
		t.Run(input.Name, func(t *testing.T) {
			cases = append(cases, migrationUgoiraOwnerCase{Input: input, Result: migrationUgoiraOwnerExecute(t, input)})
		})
	}
	fixture := struct {
		ReferenceCommit string                     `json:"reference_commit"`
		SourceSHA256    map[string]string          `json:"source_sha256"`
		CapturePlatform string                     `json:"capture_platform"`
		Use             string                     `json:"use"`
		Short           string                     `json:"short"`
		Aliases         []string                   `json:"aliases"`
		Flags           any                        `json:"flags"`
		Cases           []migrationUgoiraOwnerCase `json:"cases"`
	}{migrationUgoiraOwnerReference, sources, runtime.GOOS + "/" + runtime.GOARCH, command.Use, command.Short, append([]string{}, command.Aliases...), flags, cases}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "cli-ugoira-owner.json")
	if *migrationUpdateUgoiraOwner {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d standalone ugoira owner cases; %d frozen/worktree source identities", len(cases), len(sources))
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("standalone ugoira command differs from the frozen Go reference")
	}
	t.Logf("replayed %d standalone ugoira owner cases; %d frozen/worktree source identities", len(cases), len(sources))
}
