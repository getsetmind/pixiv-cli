package download

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
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	pixivservice "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateRandomOptions = flag.Bool("migration-update-download-random-options", false, "capture frozen Go random MCP schema and preflight")

const randomOptionsRef = "4b4426487ef18bed276706daec385e0d0a6979f9"
const randomOptionsConfiguration = "# preserve fixture comment\n[account_pool]\nenabled=true\nstrategy='round_robin'\n[unrelated]\nkeep='untouched'\n"

var randomOptionsHashes = map[string]string{
	"internal/mcpserver/pixiv/tools/download/download.go":                 "60cd7f8fcecfe27c185a9df4469dd8eaa0117e87586c047dfdf911e3400a11e5",
	"internal/mcpserver/pixiv/internal/runtime/runtime.go":                "a6ca224d1949e7716277677f81f80c1133eb0b91e196dc64e04d29fad2c7fe79",
	"internal/media/downloader/downloader.go":                             "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
	"internal/services/pixiv/account/accounts.go":                         "129d83a89a09bc8a4dbd926ce19e9551fedf048a4c1fb9f71d551ee3089fad91",
	"internal/services/pixiv/facade.go":                                   "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
	"internal/config/settings/store.go":                                   "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
	"internal/shared/lifecycle/lease.go":                                  "9715a8f3592aa0269b6f85b1a52751f2e0f2947e34fb133836fb1ef07172b381",
	"sdk/pixiv/ops_artwork.go":                                            "f8aa00684b84463c6ba82b18db87d4c2e403a0dcaa048f3c3282f6555fd7445c",
	"sdk/pixiv/map_artwork.go":                                            "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
	"internal/services/pixiv/endpoint/artwork/recommended/recommended.go": "3167adbfc47d50d458484a98a5dbe64694d3a39a845014f64ec6e5fac04c3688",
}

type randomOptionsCase struct {
	Name         string                  `json:"name"`
	Request      string                  `json:"request"`
	Response     json.RawMessage         `json:"response"`
	OpenCalls    int                     `json:"open_calls"`
	ConfigReads  int                     `json:"config_reads"`
	ConfigWrites int                     `json:"config_writes"`
	Acquired     int                     `json:"gate_acquired"`
	Released     int                     `json:"gate_released"`
	ExecuteCalls int                     `json:"execute_calls"`
	ManagerCalls int                     `json:"manager_calls"`
	DefaultReads int                     `json:"manager_default_reads"`
	ListCalls    int                     `json:"list_calls"`
	Closes       int                     `json:"lease_closes"`
	Accounts     int                     `json:"account_count_after"`
	ConfigAfter  string                  `json:"config_after"`
	Selection    *randomOptionsSelection `json:"selection,omitempty"`
}
type randomOptionsSelection struct {
	Count     int     `json:"count"`
	IDs       []int64 `json:"ids"`
	Pages     []int   `json:"pages"`
	Quality   string  `json:"quality"`
	Ugoira    string  `json:"ugoira_format"`
	Path      string  `json:"download_path"`
	Filename  string  `json:"filename_template"`
	Directory string  `json:"directory_template"`
}
type randomOptionsStore struct {
	path string
	row  *randomOptionsCase
}

func (s randomOptionsStore) Path() (string, error) { return s.path, nil }
func (s randomOptionsStore) ReadFile(path string) ([]byte, error) {
	s.row.ConfigReads++
	return os.ReadFile(path)
}
func (s randomOptionsStore) WritePrivateFile(path string, body []byte) error {
	s.row.ConfigWrites++
	return os.WriteFile(path, body, 0600)
}
func (s randomOptionsStore) EnsurePrivateFile(path string, body []byte) error {
	s.row.ConfigWrites++
	return errors.New("unexpected configuration creation")
}

type randomOptionsManager struct{ row *randomOptionsCase }

func (m randomOptionsManager) SetDownloadPath(string) error {
	return errors.New("unexpected manager mutation")
}
func (m randomOptionsManager) DownloadPath() string { m.row.DefaultReads++; return " configured path " }
func (m randomOptionsManager) FilenameTemplate() string {
	m.row.DefaultReads++
	return " configured filename "
}
func (m randomOptionsManager) DirectoryTemplate() string {
	m.row.DefaultReads++
	return " configured directory "
}
func (m randomOptionsManager) Download(_ context.Context, r downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	m.row.ManagerCalls++
	m.row.Selection = &randomOptionsSelection{len(r.IllustIDs), r.IllustIDs, r.Pages, string(r.Quality), string(r.UgoiraFormat), r.DownloadPath, r.FilenameTemplate, r.DirectoryTemplate}
	return downloader.DownloadBatchResult{}, nil
}
func randomOptionsRequest(arguments string) string {
	if arguments == "" {
		return `{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download_random_from_recommendation"}}`
	}
	return `{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download_random_from_recommendation","arguments":` + arguments + `}}`
}
func randomOptionsRows() []randomOptionsCase {
	rows := []randomOptionsCase{}
	add := func(name, args string) {
		rows = append(rows, randomOptionsCase{Name: name, Request: randomOptionsRequest(args)})
	}
	for _, v := range []struct{ name, args string }{
		{"arguments-omitted", ""}, {"arguments-null", "null"}, {"arguments-empty-object", "{}"},
		{"arguments-array", "[]"}, {"arguments-string", `"42"`}, {"arguments-boolean", "true"}, {"arguments-number", "1"},
		{"count-null", `{"count":null}`}, {"count-one", `{"count":1}`}, {"count-twenty", `{"count":20}`},
		{"count-zero", `{"count":0}`}, {"count-negative", `{"count":-1}`}, {"count-twenty-one", `{"count":21}`},
		{"count-max-int", `{"count":9223372036854775807}`}, {"count-overflow-positive", `{"count":9223372036854775808}`},
		{"count-min-int", `{"count":-9223372036854775808}`}, {"count-overflow-negative", `{"count":-9223372036854775809}`},
		{"count-large-unsigned", `{"count":18446744073709551615}`}, {"count-huge-float", `{"count":1e100}`},
		{"count-fraction", `{"count":1.5}`}, {"count-decimal-integer", `{"count":1.0}`}, {"count-exponent-integer", `{"count":1e0}`},
		{"count-negative-zero", `{"count":-0}`}, {"count-string", `{"count":"5"}`}, {"count-boolean", `{"count":true}`},
		{"count-object", `{"count":{}}`}, {"count-array", `{"count":[]}`},
		{"delivery-first", `{"delivery":"image_content","count":0,"pages":"0","quality":"large","ugoira_mode":"webm"}`},
		{"count-before-pages", `{"count":0,"pages":"0","quality":"large","ugoira_mode":"webm"}`},
		{"pages-before-quality", `{"count":1,"pages":"0","quality":"large","ugoira_mode":"webm"}`},
		{"quality-before-ugoira", `{"count":1,"quality":"large","ugoira_mode":"webm"}`},
		{"mode-invalid", `{"count":1,"ugoira_mode":"webm"}`},
		{"delivery-case-sensitive", `{"delivery":"LOCAL_PATH"}`}, {"quality-case-sensitive", `{"quality":"ORIGINAL"}`},
		{"mode-case-sensitive", `{"ugoira_mode":"GIF"}`}, {"pages-empty-segment", `{"pages":"1,,2"}`},
		{"pages-descending", `{"pages":"3-1"}`}, {"pages-overflow", `{"pages":"9223372036854775808"}`},
		{"blank-options", `{"pages":" \t\n ","quality":" \t\n ","ugoira_mode":" \t\n ","delivery":" \t\n "}`},
		{"trimmed-options", `{"pages":" 3,1-2,2 ","quality":" regular ","ugoira_mode":" raw ","delivery":" local_path "}`},
		{"unicode-trim-options", `{"pages":"\u00a01\u00a0","quality":"\u00a0original\u00a0","ugoira_mode":"\u00a0gif\u00a0","delivery":"\u00a0local_path\u00a0"}`},
		{"binding-before-delivery", `{"count":9223372036854775808,"delivery":"image_content"}`},
		{"schema-before-delivery", `{"count":"5","delivery":"image_content"}`},
	} {
		add(v.name, v.args)
	}
	for _, field := range []string{"src", "srcs", "cursor", "filter", "account", "user_id", "page", "limit", "seed", "content_type", "download_path", "filename_template", "directory_template", "unknown"} {
		add("unknown-"+field, fmt.Sprintf(`{%q:"owned"}`, field))
	}
	for _, field := range []string{"pages", "quality", "ugoira_mode", "delivery"} {
		for _, v := range []struct{ name, args string }{{"null", "null"}, {"number", "1"}, {"boolean", "true"}, {"array", "[]"}, {"object", "{}"}} {
			add(field+"-"+v.name, fmt.Sprintf(`{%q:%s}`, field, v.args))
		}
	}
	return rows
}
func randomOptionsSelections() []randomOptionsCase {
	rows := []randomOptionsCase{}
	add := func(name, args string) {
		rows = append(rows, randomOptionsCase{Name: name, Request: randomOptionsRequest(args)})
	}
	add("omitted-defaults", "{}")
	add("null-count-default", `{"count":null}`)
	add("explicit-one", `{"count":1}`)
	add("explicit-twenty", `{"count":20}`)
	add("decimal-integer-binding", `{"count":1.0}`)
	add("exponent-integer-binding", `{"count":1e0}`)
	add("blank-defaults", `{"pages":" \t\n ","quality":" \t\n ","ugoira_mode":" \t\n ","delivery":" \t\n "}`)
	add("trimmed-page-set", `{"pages":" 3,1-2,2 ","quality":" regular ","ugoira_mode":" raw ","delivery":" local_path "}`)
	add("unicode-trim", `{"pages":"\u00a01\u00a0","quality":"\u00a0original\u00a0","ugoira_mode":"\u00a0gif\u00a0","delivery":"\u00a0local_path\u00a0"}`)
	for _, quality := range []string{"original", "regular", "small", "thumb", "mini"} {
		add("quality-"+quality, fmt.Sprintf(`{"quality":%q}`, quality))
	}
	for _, mode := range []string{"gif", "apng", "zip", "raw"} {
		add("mode-"+mode, fmt.Sprintf(`{"ugoira_mode":%q}`, mode))
	}
	return rows
}

func randomOptionsCapture(t *testing.T, row *randomOptionsCase, private bool, list bool) (json.RawMessage, json.RawMessage) {
	t.Helper()
	root := t.TempDir()
	config := filepath.Join(root, "config.toml")
	if err := os.WriteFile(config, []byte(randomOptionsConfiguration), 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	db, err := database.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	store := settings.Store{Files: randomOptionsStore{config, row}}
	gate := &leaseGate{}
	facade := pixivservice.New(pixivservice.Dependencies{Accounts: account.NewService(db, store), Gate: gate})
	manager := randomOptionsManager{row}
	app := runtime.NewApp(manager, nil, runtime.SDKPorts{
		OpenLease: func(ctx context.Context, a runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
			row.OpenCalls++
			if !private {
				return facade.Open(ctx, pixivservice.Request{UserID: a.UserID})
			}
			client, e := pixiv.NewWith("owned-synthetic-access", pixiv.Options{HTTPClient: &http.Client{Transport: directTransport(func(request *http.Request) (*http.Response, error) {
				row.ListCalls++
				if request.URL.String() != "https://app-api.pixiv.net/v1/illust/recommended" {
					return nil, fmt.Errorf("unexpected owned route %s", request.URL)
				}
				items := []any{}
				for i := 0; i < 20; i++ {
					items = append(items, map[string]any{"id": 42, "title": "owned recommendation", "type": "illust", "page_count": 1, "create_date": "2026-10-08T12:34:56+09:00", "user": map[string]any{"id": 7, "name": "owned author"}})
				}
				body, _ := json.Marshal(map[string]any{"illusts": items, "next_url": nil})
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: request}, nil
			})}})
			if e != nil {
				return nil, e
			}
			return lifecycle.NewLease(client, func() error { row.Closes++; return nil }), nil
		},
		Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
			row.ExecuteCalls++
			return errors.New("unexpected pooled execution")
		},
	}, runtime.Account{})
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	ct, st := mcp.NewInMemoryTransports()
	stopped := make(chan struct{})
	go func() { defer close(stopped); _ = server.Run(ctx, st) }()
	conn, err := ct.Connect(ctx)
	if err != nil {
		t.Fatal(err)
	}
	send := func(raw string) {
		t.Helper()
		message, e := jsonrpc.DecodeMessage([]byte(raw))
		if e != nil {
			t.Fatal(e)
		}
		if e = conn.Write(ctx, message); e != nil {
			t.Fatal(e)
		}
	}
	read := func() json.RawMessage {
		t.Helper()
		message, e := conn.Read(ctx)
		if e != nil {
			t.Fatal(e)
		}
		body, e := jsonrpc.EncodeMessage(message)
		if e != nil {
			t.Fatal(e)
		}
		return body
	}
	send(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`)
	_ = read()
	send(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
	var randomTool, downloadTool json.RawMessage
	if list {
		send(`{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}`)
		var listed struct {
			Result struct {
				Tools []json.RawMessage `json:"tools"`
			} `json:"result"`
		}
		if err = json.Unmarshal(read(), &listed); err != nil {
			t.Fatal(err)
		}
		for _, tool := range listed.Result.Tools {
			var name struct{ Name string }
			_ = json.Unmarshal(tool, &name)
			switch name.Name {
			case "download":
				downloadTool = tool
			case "download_random_from_recommendation":
				randomTool = tool
			}
		}
		if randomTool == nil || downloadTool == nil {
			t.Fatal("registered download tools missing")
		}
	}
	send(row.Request)
	row.Response = read()
	_ = conn.Close()
	cancel()
	<-stopped
	row.Acquired = gate.acquired
	row.Released = gate.released
	accounts, e := db.ListPixiv(context.Background())
	if e != nil {
		t.Fatal(e)
	}
	row.Accounts = len(accounts)
	body, e := os.ReadFile(config)
	if e != nil {
		t.Fatal(e)
	}
	row.ConfigAfter = string(body)
	if row.ConfigAfter != randomOptionsConfiguration || row.ConfigWrites != 0 || row.Accounts != 0 || row.ExecuteCalls != 0 {
		t.Fatalf("unexpected configuration/account/pool effects: %+v", row)
	}
	if !private && row.OpenCalls == 0 && (row.ConfigReads != 0 || row.Acquired != 0 || row.Released != 0 || row.ManagerCalls != 0 || row.DefaultReads != 0 || row.ListCalls != 0) {
		t.Fatalf("invalid input crossed preflight: %+v", row)
	}
	if private && row.Selection == nil {
		t.Fatalf("valid typed binding did not reach actual handler manager: %+v", row)
	}
	return randomTool, downloadTool
}
func TestMigrationMCPDownloadRandomOptionsMatchesFrozenContract(t *testing.T) {
	base := filepath.Join("..", "..", "..", "..", "..")
	for path, want := range randomOptionsHashes {
		body, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		frozen, err := exec.Command("git", "-C", base, "show", randomOptionsRef+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		for _, source := range [][]byte{body, frozen} {
			if fmt.Sprintf("%x", sha256.Sum256(source)) != want {
				t.Fatalf("frozen/worktree source changed: %s", path)
			}
		}
	}
	rows := randomOptionsRows()
	selections := randomOptionsSelections()
	var tool, mainTool json.RawMessage
	for i := range rows {
		t.Run(rows[i].Name, func(t *testing.T) {
			random, main := randomOptionsCapture(t, &rows[i], false, i == 0)
			if i == 0 {
				tool = random
				mainTool = main
			}
		})
	}
	for i := range selections {
		t.Run("private-"+selections[i].Name, func(t *testing.T) { randomOptionsCapture(t, &selections[i], true, false) })
	}
	var randomSchema, mainSchema map[string]any
	if err := json.Unmarshal(tool, &randomSchema); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(mainTool, &mainSchema); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(randomSchema["outputSchema"], mainSchema["outputSchema"]) {
		t.Fatal("registered output schemas differ")
	}
	input := randomSchema["inputSchema"].(map[string]any)
	count := input["properties"].(map[string]any)["count"].(map[string]any)
	if count["minimum"] != nil || count["maximum"] != nil || input["required"] != nil {
		t.Fatal("count range/default moved into schema")
	}
	actual := struct {
		Source        string              `json:"source"`
		SourceHashes  map[string]string   `json:"source_hashes"`
		Configuration string              `json:"configuration"`
		Boundary      []string            `json:"observation_boundaries"`
		Tool          json.RawMessage     `json:"tool"`
		MainTool      json.RawMessage     `json:"main_download_tool"`
		Cases         []randomOptionsCase `json:"cases"`
		Selections    []randomOptionsCase `json:"go_private_selection"`
	}{randomOptionsRef, randomOptionsHashes, randomOptionsConfiguration, []string{"public cases use actual Register and JSON-RPC schema/typed binding/handler through saved Go Facade with empty owned DB and preserved configuration", "open/config/gate/manager counters are internal Go boundary observations; zero-effects and persisted configuration/accounts have public Rust equivalents", "go_private_selection uses twenty identical entries and genuine SDK recommendation plus injected manager to observe handler count/pages/quality/mode/runtime defaults; this is Go-only and is not a stub-success RPC golden for Rust", "raw request strings retain decimal/exponent/overflow JSON spellings; expected protocol messages are observed, not manually reconstructed"}, tool, mainTool, rows, selections}
	body, err := json.MarshalIndent(actual, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_random_options.json")
	if *updateRandomOptions {
		if err = os.WriteFile(path, body, 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var gotValue, wantValue any
	if err = json.Unmarshal(body, &gotValue); err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(want, &wantValue); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(gotValue, wantValue) {
		out := filepath.Join(t.TempDir(), "actual.json")
		_ = os.WriteFile(out, body, 0600)
		t.Fatalf("frozen random options fixture changed: %s", out)
	}
	if strings.Contains(string(body), "unexpected owned route") {
		t.Fatal("unexpected synthetic network route")
	}
}
