package fanbox_test

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
	"runtime"
	"strings"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	secret "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/secret"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var captureFanboxSavedService = flag.Bool("migration-capture-fanbox-saved-service", false, "capture frozen FANBOX saved service contracts")

type migrationFanboxServiceKey struct{}

type migrationFanboxServiceRow struct {
	Name             string           `json:"name"`
	Input            map[string]any   `json:"input"`
	Result           any              `json:"result"`
	Error            map[string]any   `json:"error"`
	Panic            string           `json:"panic"`
	DatabasePoisoned bool             `json:"database_poisoned"`
	Trace            []string         `json:"trace"`
	Rows             []map[string]any `json:"rows"`
	Config           string           `json:"config"`
	Requests         []map[string]any `json:"requests"`
	BodyCloses       int              `json:"body_closes"`
	IdleCloses       int              `json:"idle_closes"`
	Options          any              `json:"options"`
}

type migrationFanboxRepositoryPort struct {
	db     *database.DB
	row    *migrationFanboxServiceRow
	fail   string
	nth    int
	counts map[string]int
}

func (p *migrationFanboxRepositoryPort) before(ctx context.Context, op string) error {
	p.counts[op]++
	marker := "nil"
	ctxErr := ""
	if ctx != nil {
		marker = fmt.Sprint(ctx.Value(migrationFanboxServiceKey{}))
		if ctx.Err() != nil {
			ctxErr = ctx.Err().Error()
		}
	}
	p.row.Trace = append(p.row.Trace, fmt.Sprintf("repository.%s/context=%s/err=%s", op, marker, ctxErr))
	if p.fail == op && (p.nth == 0 || p.counts[op] == p.nth) {
		return errors.New("owned fixture " + op + " failure")
	}
	return nil
}
func (p *migrationFanboxRepositoryPort) SaveFanboxCredential(ctx context.Context, a account.Account) error {
	if e := p.before(ctx, "save"); e != nil {
		return e
	}
	return p.db.SaveFanboxCredential(ctx, a)
}
func (p *migrationFanboxRepositoryPort) RotateFanboxSession(ctx context.Context, id, revision int64, b []byte, at int64) error {
	if e := p.before(ctx, "rotate"); e != nil {
		return e
	}
	return p.db.RotateFanboxSession(ctx, id, revision, b, at)
}
func (p *migrationFanboxRepositoryPort) ListFanbox(ctx context.Context) ([]account.Account, error) {
	if e := p.before(ctx, "list"); e != nil {
		return nil, e
	}
	return p.db.ListFanbox(ctx)
}
func (p *migrationFanboxRepositoryPort) GetFanbox(ctx context.Context, id int64) (account.Account, error) {
	if e := p.before(ctx, "get"); e != nil {
		return account.Account{}, e
	}
	return p.db.GetFanbox(ctx, id)
}
func (p *migrationFanboxRepositoryPort) RemoveFanbox(ctx context.Context, id int64) error {
	if e := p.before(ctx, "remove"); e != nil {
		return e
	}
	return p.db.RemoveFanbox(ctx, id)
}

type migrationFanboxConfigFiles struct {
	path string
	fail string
}

func (f *migrationFanboxConfigFiles) Path() (string, error) {
	if f.fail == "path" {
		return "", errors.New("owned fixture config path failure")
	}
	return f.path, nil
}
func (f *migrationFanboxConfigFiles) ReadFile(path string) ([]byte, error) {
	if f.fail == "read" {
		return nil, errors.New("owned fixture config read failure")
	}
	return os.ReadFile(path)
}
func (f *migrationFanboxConfigFiles) WritePrivateFile(path string, b []byte) error {
	if f.fail == "write" {
		return errors.New("owned fixture config write failure")
	}
	return secret.WritePrivateFile(path, b, 0600)
}
func (f *migrationFanboxConfigFiles) EnsurePrivateFile(path string, b []byte) error {
	return secret.EnsurePrivateFile(path, b, 0600)
}

type migrationFanboxDefaultPort struct {
	store settings.Store
	row   *migrationFanboxServiceRow
	fail  string
	nth   int
	reads int
	noSet bool
}

func (p *migrationFanboxDefaultPort) ReadFanboxDefaultUserID() (int64, bool, error) {
	p.reads++
	p.row.Trace = append(p.row.Trace, "defaults.read")
	if p.fail == "read" && (p.nth == 0 || p.nth == p.reads) {
		return 0, false, errors.New("owned fixture default read failure")
	}
	return p.store.ReadFanboxDefaultUserID()
}
func (p *migrationFanboxDefaultPort) SetFanboxDefaultUserID(id int64) error {
	p.row.Trace = append(p.row.Trace, fmt.Sprintf("defaults.set/%d", id))
	if p.fail == "set" {
		return errors.New("owned fixture default set failure")
	}
	if p.noSet {
		return nil
	}
	return p.store.SetFanboxDefaultUserID(id)
}
func (p *migrationFanboxDefaultPort) ClearFanboxDefaultUserID() error {
	p.row.Trace = append(p.row.Trace, "defaults.clear")
	if p.fail == "clear" {
		return errors.New("owned fixture default clear failure")
	}
	return p.store.ClearFanboxDefaultUserID()
}

type migrationFanboxBody struct {
	io.Reader
	row  *migrationFanboxServiceRow
	fail bool
}

func (b *migrationFanboxBody) Close() error {
	b.row.BodyCloses++
	if b.fail {
		return errors.New("owned fixture identity close failure")
	}
	return nil
}

type migrationFanboxReadFailure struct{}

func (migrationFanboxReadFailure) Read([]byte) (int, error) {
	return 0, errors.New("owned fixture identity read failure")
}

type migrationFanboxTransport struct {
	row    *migrationFanboxServiceRow
	mode   string
	cancel context.CancelFunc
}

func (r *migrationFanboxTransport) CloseIdleConnections() { r.row.IdleCloses++ }
func (r *migrationFanboxTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	marker := fmt.Sprint(req.Context().Value(migrationFanboxServiceKey{}))
	ctxErr := ""
	if req.Context().Err() != nil {
		ctxErr = req.Context().Err().Error()
	}
	r.row.Trace = append(r.row.Trace, "verify.request")
	r.row.Requests = append(r.row.Requests, map[string]any{"method": req.Method, "url": req.URL.String(), "cookie": req.Header.Get("Cookie"), "user_agent": req.Header.Get("User-Agent"), "accept": req.Header.Get("Accept"), "context": marker, "context_error": ctxErr})
	if r.cancel != nil {
		r.cancel()
	}
	if r.mode == "transport_error" {
		return nil, errors.New("owned fixture transport failure")
	}
	if req.Context().Err() != nil {
		return nil, req.Context().Err()
	}
	status := 200
	if r.mode == "unauthorized" {
		status = 401
	}
	if r.mode == "forbidden" {
		status = 403
	}
	identity := `{"context":{"user":{"userId":42,"name":"  fixture verified  ","creatorId":"fixture-creator"}}}`
	if r.mode == "zero_identity" {
		identity = `{"context":{"user":{"userId":0,"name":"fixture"}}}`
	}
	if r.mode == "malformed" {
		identity = "{"
	}
	reader := io.Reader(bytes.NewBufferString(`<html><head><meta name="metadata" content='` + identity + `'></head></html>`))
	if r.mode == "read_failure" {
		reader = migrationFanboxReadFailure{}
	}
	return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"text/html"}}, Body: &migrationFanboxBody{Reader: reader, row: r.row, fail: r.mode == "close_failure"}}, nil
}

type migrationFanboxServiceHarness struct {
	row      *migrationFanboxServiceRow
	db       *database.DB
	repo     *migrationFanboxRepositoryPort
	defaults *migrationFanboxDefaultPort
	files    *migrationFanboxConfigFiles
	service  *account.Service
	ctx      context.Context
	cancel   context.CancelFunc
	start    int64
}

func migrationFanboxNewServiceHarness(t *testing.T, name, config string, seed bool) *migrationFanboxServiceHarness {
	t.Helper()
	row := &migrationFanboxServiceRow{Name: name, Input: map[string]any{}, Trace: []string{}, Requests: []map[string]any{}}
	dir := t.TempDir()
	db, e := database.Open(filepath.Join(dir, "auth"))
	if e != nil {
		t.Fatal(e)
	}
	var h *migrationFanboxServiceHarness
	t.Cleanup(func() {
		if h == nil || !h.row.DatabasePoisoned {
			_ = db.Close()
		}
	})
	files := &migrationFanboxConfigFiles{path: filepath.Join(dir, "config.toml")}
	if config != "missing" {
		if e = os.WriteFile(files.path, []byte(config), 0600); e != nil {
			t.Fatal(e)
		}
	}
	repo := &migrationFanboxRepositoryPort{db: db, row: row, counts: map[string]int{}}
	defaults := &migrationFanboxDefaultPort{store: settings.Store{Files: files}, row: row}
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationFanboxServiceKey{}, "caller"))
	t.Cleanup(cancel)
	h = &migrationFanboxServiceHarness{row: row, db: db, repo: repo, defaults: defaults, files: files, ctx: ctx, cancel: cancel, start: time.Now().Unix()}
	h.service = account.NewService(repo, defaults)
	if seed {
		for _, id := range []int64{7, 9} {
			a := account.New(id, fmt.Sprintf("fixture-%d", id), "", []byte(fmt.Sprintf("fixture-session-%d", id)))
			a.ValidatedAt = 30
			if e = db.SaveFanboxCredential(context.Background(), a); e != nil {
				t.Fatal(e)
			}
		}
		if _, e = db.DB().Exec("UPDATE fanbox_account SET created_at=11,updated_at=22"); e != nil {
			t.Fatal(e)
		}
	}
	return h
}
func (h *migrationFanboxServiceHarness) finish(t *testing.T, call func() error) migrationFanboxServiceRow {
	t.Helper()
	t.Logf("observe service case: %s", h.row.Name)
	var err error
	func() {
		defer func() {
			if p := recover(); p != nil {
				h.row.Panic = fmt.Sprint(p)
				if h.ctx == nil {
					h.row.DatabasePoisoned = true
				}
			}
		}()
		err = call()
	}()
	h.row.Error = migrationFanboxServiceError(err)
	snapshot := h.db
	if h.row.DatabasePoisoned {
		var e error
		snapshot, e = database.Open(filepath.Dir(h.db.Path()))
		if e != nil {
			t.Fatal(e)
		}
		defer snapshot.Close()
	}
	accounts, e := snapshot.ListFanbox(context.Background())
	if e != nil {
		t.Fatal(e)
	}
	h.row.Rows = []map[string]any{}
	for _, a := range accounts {
		stamp := func(n int64) any {
			if n > 100 {
				if n < h.start || n > time.Now().Unix() {
					t.Fatalf("timestamp outside observed operation: %d", n)
				}
				return "current-time"
			}
			return n
		}
		h.row.Rows = append(h.row.Rows, map[string]any{"user_id": a.UserID, "sort_order": a.SortOrder, "display_name": a.DisplayName, "creator_id": a.CreatorID, "session": string(a.SessionIDCopy()), "credential_revision": a.CredentialRevision, "validated_at": stamp(a.ValidatedAt), "created_at": stamp(a.CreatedAt), "updated_at": stamp(a.UpdatedAt)})
	}
	body, e := os.ReadFile(h.files.path)
	if e != nil && !errors.Is(e, os.ErrNotExist) {
		t.Fatal(e)
	}
	h.row.Config = string(body)
	return *h.row
}
func migrationFanboxServiceError(err error) map[string]any {
	text := ""
	if err != nil {
		text = err.Error()
	}
	var typed *sdk.Error
	var classified any
	if errors.As(err, &typed) {
		classified = map[string]any{"product": typed.Product, "operation": typed.Operation, "reason": typed.Reason, "detail": typed.Detail}
	}
	return map[string]any{"text": text, "not_found": errors.Is(err, account.ErrNotFound), "canceled": errors.Is(err, context.Canceled), "deadline": errors.Is(err, context.DeadlineExceeded), "classified": classified}
}

func TestMigrationFanboxSavedServiceFrozenGo(t *testing.T) {
	sources := map[string]string{
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/services/fanbox/account/accounts.go":  "261ae6e5339086f205d7662e435ac664e626d8800cd9937789867bfc82b473bc",
		"internal/services/fanbox/account/fanbox.go":    "0e7af911bd41245607d7bb0e1dc0206776785ddb88cd45f9344f927d2c9b72a8",
		"internal/storage/database/repository.go":       "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		"internal/config/settings/auth.go":              "78729a90d7a71fe5793f469be5770d1acb565ac2945bf252b893dfcec392981e",
		"internal/config/settings/config.go":            "591900e2f5648642a6296eb1306cbf0b2fa25d8f5c06f4d61d6625d5340d73b3",
		"internal/config/settings/store.go":             "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
		"internal/config/settings/snapshot.go":          "137a245260ccf8be8e26210fa4c91edecf342a8a6d69d72c77dd11fec4843609",
		"internal/config/settings/document.go":          "533602371c7a318b686f2ed506f03341c5890d53b7e86e3940234f7eb7a26ebd",
		"internal/config/settings/paths.go":             "f81e702c53e232fa5cbf7f910aec0a60f343547d1f5bcdbdcc21b3efdd3c6b31",
		"internal/config/settings/schema.go":            "ff41a9f3cda2c29ef1474784440e82600ba56f65883e21d35fcb31483b04ab7e",
		"internal/config/settings/values.go":            "35738fb8fdfdbc1d2cf27e541547325d3bae633b12fc2a28b58b1785177f1788",
		"internal/config/settings/defaults.go":          "3dbfbf1f18510bd296c30bba74468732150ee0249f6f3744289cf22f870b0753",
		"sdk/fanbox/fanbox.go":                          "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542",
		"sdk/fanbox/ops.go":                             "868b3b68de07d7638af5f9dd3ab1be8f0c9751f222c05c7776cde092052a7c8d",
		"sdk/fanbox/models.go":                          "1dd3928aa6752c51f114c678eef3b0a2b4784a275ec80fcdbbd61006407e4623",
		"sdk/fanbox/errors.go":                          "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909",
		"sdk/error.go":                                  "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
		"internal/services/fanbox/protocol/protocol.go": "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3",
		"internal/services/fanbox/protocol/identity.go": "10ba481b43e3ec7d0bdaa2defbbd629c4b5bb2aa169c5bff99c8e756de8143d9",
		"internal/services/fanbox/protocol/cookie.go":   "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9",
	}
	for path, want := range sources {
		b, e := os.ReadFile(filepath.Join("..", "..", "..", "..", path))
		if e != nil {
			t.Fatal(e)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(b)); got != want {
			t.Fatalf("frozen source changed: %s: %s", path, got)
		}
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("unexpected Go version: %s", runtime.Version())
	}
	for _, name := range []string{"DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "https_proxy", "HTTPS_PROXY", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY"} {
		t.Setenv(name, "")
		if e := os.Unsetenv(name); e != nil {
			t.Fatal(e)
		}
	}
	var cases []migrationFanboxServiceRow
	for _, action := range []string{"list", "status", "open", "remove", "use", "auto"} {
		for _, selection := range []string{"empty", "auto", "explicit", "missing", "nil_defaults"} {
			cfg := "# preserved fixture\n[unrelated]\nvalue = 'kept'\n"
			seed := selection != "empty"
			if selection == "explicit" {
				cfg += "[fanbox.auth]\ndefault_user_id = 9\n"
			}
			if selection == "missing" {
				cfg += "[fanbox.auth]\ndefault_user_id = 99\n"
			}
			h := migrationFanboxNewServiceHarness(t, action+"/"+selection, cfg, seed)
			h.row.Input = map[string]any{"action": action, "selection": selection, "id": int64(9)}
			if selection == "nil_defaults" {
				h.service = account.NewService(h.repo, nil)
			}
			h.service.LoadOptionsFunc = func() (fanboxsdk.Options, error) {
				h.row.Trace = append(h.row.Trace, "options.load")
				return fanboxsdk.Options{HTTPClient: &http.Client{Transport: &migrationFanboxTransport{row: h.row}}}, nil
			}
			cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, action, 9) }))
		}
	}
	for _, action := range []string{"list", "status", "open", "remove", "use", "auto"} {
		for _, port := range []string{"nil_repository", "nil_service"} {
			h := migrationFanboxNewServiceHarness(t, action+"/"+port, "", true)
			h.row.Input = map[string]any{"action": action, "port": port}
			if port == "nil_repository" {
				h.service = account.NewService(nil, h.defaults)
			} else {
				h.service = nil
			}
			cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, action, 7) }))
		}
	}
	for _, action := range []string{"use", "remove"} {
		for _, id := range []int64{0, -7} {
			h := migrationFanboxNewServiceHarness(t, fmt.Sprintf("%s/nonpositive_id/%d", action, id), "", true)
			h.row.Input = map[string]any{"action": action, "id": id}
			cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, action, id) }))
		}
	}
	for _, input := range []struct {
		action, fail string
		nth          int
	}{
		{"list", "list", 1}, {"list", "list", 2}, {"list", "list", 3}, {"list", "default_read", 1}, {"list", "default_read", 2},
		{"status", "list", 1}, {"status", "get", 1}, {"status", "default_read", 1},
		{"open", "list", 1}, {"open", "get", 1}, {"open", "default_read", 1},
		{"remove", "remove", 1}, {"remove", "default_read", 1}, {"use", "get", 1}, {"use", "default_set", 1}, {"auto", "default_clear", 1},
	} {
		h := migrationFanboxNewServiceHarness(t, fmt.Sprintf("%s/failure/%s/%d", input.action, input.fail, input.nth), "", true)
		h.row.Input = map[string]any{"action": input.action, "failure": input.fail, "nth": input.nth}
		h.repo.fail = input.fail
		h.repo.nth = input.nth
		if input.fail == "default_read" {
			h.defaults.fail = "read"
			h.defaults.nth = input.nth
		}
		if input.fail == "default_set" {
			h.defaults.fail = "set"
		}
		if input.fail == "default_clear" {
			h.defaults.fail = "clear"
		}
		cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, input.action, 7) }))
	}
	for _, action := range []string{"status", "open"} {
		for _, nth := range []int{1, 2} {
			h := migrationFanboxNewServiceHarness(t, fmt.Sprintf("%s/explicit_get_failure/%d", action, nth), "[fanbox.auth]\ndefault_user_id=7\n", true)
			h.repo.fail = "get"
			h.repo.nth = nth
			cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, action, 7) }))
		}
	}
	for _, action := range []string{"list", "status", "open", "remove", "use"} {
		for _, mode := range []string{"canceled", "deadline", "nil_context"} {
			if mode == "nil_context" {
				cases = append(cases, migrationFanboxNilContextSubprocess(t, action))
				continue
			}
			h := migrationFanboxNewServiceHarness(t, action+"/context/"+mode, "", true)
			switch mode {
			case "canceled":
				h.cancel()
			case "deadline":
				var stop context.CancelFunc
				h.ctx, stop = context.WithDeadline(h.ctx, time.Unix(1, 0))
				t.Cleanup(stop)
			case "nil_context":
				h.ctx = nil
			}
			cases = append(cases, h.finish(t, func() error { return migrationFanboxServiceAction(h, action, 7) }))
		}
	}
	for _, input := range []struct {
		name, mode, config string
		setDefault, seed   bool
		fail               string
		nth                int
	}{
		{name: "empty session", mode: "empty"}, {name: "trimmed first import", mode: "success"},
		{name: "auto existing accounts import selects new", mode: "success", seed: true},
		{name: "explicit retained", mode: "success", seed: true, config: "[fanbox.auth]\ndefault_user_id=7\n"},
		{name: "explicit overridden", mode: "success", seed: true, config: "[fanbox.auth]\ndefault_user_id=7\n", setDefault: true},
		{name: "missing explicit retained", mode: "success", seed: true, config: "[fanbox.auth]\ndefault_user_id=99\n"},
		{name: "invalid explicit after save", mode: "success", config: "[fanbox.auth]\ndefault_user_id='invalid'\n"},
		{name: "verification unauthorized", mode: "unauthorized"}, {name: "verification forbidden", mode: "forbidden"}, {name: "verification malformed", mode: "malformed"}, {name: "verification zero identity", mode: "zero_identity"},
		{name: "verification transport error", mode: "transport_error"}, {name: "verification read failure", mode: "read_failure"}, {name: "verification close failure", mode: "close_failure"},
		{name: "session factory error", mode: "factory_error"}, {name: "session factory partial error", mode: "partial_factory_error"}, {name: "session factory nil success", mode: "nil_factory"},
		{name: "save failure", mode: "success", fail: "save"}, {name: "default read failure after save", mode: "success", fail: "default_read", nth: 1}, {name: "default set failure after save", mode: "success", fail: "default_set"}, {name: "final default read failure after save", mode: "success", fail: "default_read", nth: 2},
		{name: "final automatic list failure after save", mode: "success", fail: "list"},
		{name: "verification cancellation", mode: "cancel_during"}, {name: "canceled before verification", mode: "cancel_before"}, {name: "deadline before verification", mode: "deadline"}, {name: "nil verification context", mode: "nil_context"},
	} {
		h := migrationFanboxNewServiceHarness(t, "import/"+input.name, input.config, input.seed)
		h.row.Input = map[string]any{"mode": input.mode, "set_default": input.setDefault, "initial_config": input.config, "seed": input.seed}
		if input.fail == "save" || input.fail == "list" {
			h.repo.fail = input.fail
		}
		if input.fail == "default_read" {
			h.defaults.fail = "read"
			h.defaults.nth = input.nth
		}
		if input.fail == "default_set" {
			h.defaults.fail = "set"
		}
		if input.fail == "list" {
			h.defaults.noSet = true
		}
		if input.mode == "cancel_before" {
			h.cancel()
		}
		if input.mode == "deadline" {
			var stop context.CancelFunc
			h.ctx, stop = context.WithDeadline(h.ctx, time.Unix(1, 0))
			t.Cleanup(stop)
		}
		if input.mode == "nil_context" {
			h.ctx = nil
		}
		h.service.OpenSessionFunc = func(value string) (*fanboxsdk.Client, error) {
			h.row.Trace = append(h.row.Trace, "session.factory/"+value)
			if input.mode == "factory_error" {
				return nil, errors.New("owned fixture session factory failure")
			}
			if input.mode == "nil_factory" {
				return nil, nil
			}
			transport := &migrationFanboxTransport{row: h.row, mode: input.mode}
			if input.mode == "cancel_during" {
				transport.cancel = h.cancel
			}
			client, e := fanboxsdk.OpenWith(fanboxsdk.SessionCredentials{FANBOXSESSID: value}, fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}})
			if e != nil {
				t.Fatal(e)
			}
			if input.mode == "partial_factory_error" {
				return client, errors.New("owned fixture session factory failure")
			}
			return client, nil
		}
		session := "  fixture-import-session  "
		if input.mode == "empty" {
			session = " \t\r\n "
		}
		cases = append(cases, h.finish(t, func() error {
			summary, e := h.service.ImportSession(h.ctx, session, input.setDefault)
			h.row.Result = summary
			return e
		}))
	}
	for _, input := range []string{"missing", "0", "-7", "1.0", "1.5", "'7'", "true", "9223372036854775807", "[7]", "{value=7}"} {
		cfg := "# preserved fixture\n[unrelated]\nvalue='kept'\n"
		if input != "missing" {
			cfg += "[fanbox.auth]\ndefault_user_id=" + input + "\n"
		}
		h := migrationFanboxNewServiceHarness(t, "config/default_read/"+input, cfg, false)
		h.row.Input = map[string]any{"raw": input}
		cases = append(cases, h.finish(t, func() error {
			id, ok, e := h.defaults.store.ReadFanboxDefaultUserID()
			h.row.Result = map[string]any{"id": id, "present": ok}
			return e
		}))
	}
	for _, action := range []string{"read", "set", "clear"} {
		for _, failure := range []string{"path", "read", "write", "malformed", "nil_files"} {
			h := migrationFanboxNewServiceHarness(t, "config/"+action+"/"+failure, "[fanbox.auth]\ndefault_user_id=7\n", false)
			h.files.fail = failure
			if failure == "malformed" {
				if e := os.WriteFile(h.files.path, []byte("[broken"), 0600); e != nil {
					t.Fatal(e)
				}
			}
			store := h.defaults.store
			if failure == "nil_files" {
				store = settings.Store{}
			}
			cases = append(cases, h.finish(t, func() error {
				switch action {
				case "read":
					id, ok, e := store.ReadFanboxDefaultUserID()
					h.row.Result = map[string]any{"id": id, "present": ok}
					return e
				case "set":
					return store.SetFanboxDefaultUserID(9)
				default:
					return store.ClearFanboxDefaultUserID()
				}
			}))
		}
	}
	for _, id := range []int64{0, -7, 7} {
		h := migrationFanboxNewServiceHarness(t, fmt.Sprintf("config/set_id/%d", id), "# preserved fixture\n[unrelated]\nvalue='kept'\n", false)
		cases = append(cases, h.finish(t, func() error { return h.defaults.store.SetFanboxDefaultUserID(id) }))
	}
	for _, name := range []string{"defensive_copy_formatting_and_json", "nil_session", "empty_session"} {
		h := migrationFanboxNewServiceHarness(t, "account/value/"+name, "", false)
		cases = append(cases, h.finish(t, func() error {
			b := []byte("fixture-secret-canary")
			if name == "nil_session" {
				b = nil
			}
			if name == "empty_session" {
				b = []byte{}
			}
			a := account.New(7, "fixture-name", "fixture-creator", b)
			if len(b) > 0 {
				b[0] = 'X'
			}
			copy := a.SessionIDCopy()
			if len(copy) > 0 {
				copy[0] = 'Y'
			}
			formats := []string{}
			for _, verb := range []string{"%v", "%+v", "%#v", "%q", "%x", "%d", "%s", "%p"} {
				formats = append(formats, fmt.Sprintf(verb, a))
			}
			body, e := json.Marshal(a)
			h.row.Result = map[string]any{"session": string(a.SessionIDCopy()), "session_nil": a.SessionIDCopy() == nil, "has_session": a.HasSession(), "formatted": formats, "format_verbs": []string{"%v", "%+v", "%#v", "%q", "%x", "%d", "%s", "%p"}, "json": string(body)}
			return e
		}))
	}
	cases = append(cases, migrationFanboxOptionCases(t)...)
	section := map[string]any{"source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "source_sha256": sources, "go_version": runtime.Version(), "cases": cases, "go_only": []string{"OpenSessionFunc raw nil,nil causes Go nil-pointer panic and partial client+error is returned without idle-close", "private selectedUserID and isDefault error ordering, repeated storage reads, missing-default masking", "nil caller context reaches database/sql panic with its mutex locked; each poisoned handle is isolated to an owned subprocess and never reused or closed; SDK nil context instead returns request-build failure", "private SDK protocol session option values are inspected read-only using reflection; no production API is expanded", "Standard Account formatting verbs are redacted and private session is omitted by default JSON; unsupported %p on a value bypasses fmt.Formatter and exposes session bytes in its invalid-format diagnostic, an explicit Go-only security defect"}, "limitations": []string{"All sessions, identities, config bodies, storage-port failures and HTTP responses are synthetic; production SDK parsing and actual SQLite/config writes are executed", "Recognized config environment variables are cleared in the test process and restored afterwards; no user paths or real account state are accessed", "Wall-clock generated validated_at/created_at/updated_at are range checked and represented as current-time", "Runtime config parsing and leaf option override are captured here; private CLI composition translator precedence is a separate startup contract", "Native TLS, remote validation, browser cookie extraction and Windows ACL are not exercised"}}
	migrationFanboxServiceFixture(t, section)
	t.Logf("fresh service cases: %d", len(cases))
}

func migrationFanboxServiceAction(h *migrationFanboxServiceHarness, action string, id int64) error {
	switch action {
	case "list":
		v, e := h.service.ListAccounts(h.ctx)
		h.row.Result = v
		return e
	case "status":
		v, e := h.service.Status(h.ctx)
		h.row.Result = v
		return e
	case "open":
		client, e := h.service.OpenClient(h.ctx)
		h.row.Result = map[string]any{"client_present": client != nil}
		if client != nil {
			h.row.Trace = append(h.row.Trace, "caller.client.close")
			client.CloseIdleConnections()
		}
		return e
	case "remove":
		return h.service.RemoveAccount(h.ctx, id)
	case "use":
		return h.service.UseAccount(h.ctx, id)
	case "auto":
		return h.service.UseAuto()
	}
	panic("unknown service action")
}

func migrationFanboxOptionCases(t *testing.T) []migrationFanboxServiceRow {
	var rows []migrationFanboxServiceRow
	for _, input := range []struct {
		name, proxy, override, agent, solver, solverProxy, mode string
		hasOverride                                             bool
	}{
		{name: "default import loads options and passes proxy", mode: "default_import", proxy: "http://127.0.0.1:19001", override: "http://127.0.0.1:19003", hasOverride: true, agent: "fixture-agent"},
		{name: "default import loader failure before verification", mode: "default_import_loader_error"},
		{name: "default import invalid proxy before verification", mode: "default_import", proxy: "ftp://fixture.invalid"},
		{name: "default import empty override masks invalid proxy", mode: "default_import", proxy: "ftp://fixture.invalid", hasOverride: true},
		{name: "loaded options", proxy: "http://127.0.0.1:19001", agent: "fixture-agent", solver: "https://fixture-solver.invalid/v1", solverProxy: "http://127.0.0.1:19002"},
		{name: "override proxy only", proxy: "http://127.0.0.1:19001", override: "http://127.0.0.1:19003", hasOverride: true, agent: "fixture-agent", solver: "https://fixture-solver.invalid/v1", solverProxy: "http://127.0.0.1:19002"},
		{name: "explicit empty proxy disables", proxy: "http://127.0.0.1:19001", hasOverride: true},
		{name: "invalid loaded proxy", proxy: "http://user:password@fixture.invalid"},
		{name: "override masks invalid loaded proxy", proxy: "http://user:password@fixture.invalid", hasOverride: true},
		{name: "invalid override", override: "ftp://fixture.invalid", hasOverride: true},
		{name: "invalid user agent", agent: "fixture\r\ninvalid"},
		{name: "loader error", mode: "loader_error"},
		{name: "injected client bypasses loader selection and proxy", mode: "injected"},
		{name: "injected nil success preserved", mode: "injected_nil"},
		{name: "injected partial error preserved", mode: "injected_partial"},
		{name: "injected session bypasses loader and proxy", mode: "injected_session", hasOverride: true, override: "ftp://fixture.invalid"},
	} {
		h := migrationFanboxNewServiceHarness(t, "options/"+input.name, "", true)
		h.row.Input = map[string]any{"proxy": input.proxy, "override": input.override, "has_override": input.hasOverride, "user_agent": input.agent, "solver": input.solver, "solver_proxy": input.solverProxy, "mode": input.mode}
		transport := &migrationFanboxTransport{row: h.row}
		h.service.LoadOptionsFunc = func() (fanboxsdk.Options, error) {
			h.row.Trace = append(h.row.Trace, "options.load")
			if input.mode == "default_import_loader_error" || input.mode == "loader_error" || input.mode == "injected" || input.mode == "injected_nil" || input.mode == "injected_partial" || input.mode == "injected_session" {
				return fanboxsdk.Options{}, errors.New("owned fixture option loader failure")
			}
			options := fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}, ProxyURL: input.proxy, UserAgent: input.agent}
			if input.solver != "" {
				options.FlareSolverr = &fanboxsdk.FlareSolverrOptions{URL: input.solver, ProxyURL: input.solverProxy}
			}
			return options, nil
		}
		var override *string
		if input.hasOverride {
			override = &input.override
		}
		if input.mode == "injected" || input.mode == "injected_nil" || input.mode == "injected_partial" {
			h.service = account.NewService(nil, nil)
			h.service.LoadOptionsFunc = func() (fanboxsdk.Options, error) { panic("injected client called option loader") }
			h.service.OpenClientFunc = func(ctx context.Context) (*fanboxsdk.Client, error) {
				h.row.Trace = append(h.row.Trace, "client.factory/context="+fmt.Sprint(ctx.Value(migrationFanboxServiceKey{})))
				if input.mode == "injected_nil" {
					return nil, nil
				}
				client, e := fanboxsdk.OpenWith(fanboxsdk.SessionCredentials{FANBOXSESSID: "fixture-injected-session"}, fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}})
				if e != nil {
					t.Fatal(e)
				}
				if input.mode == "injected_partial" {
					return client, errors.New("owned fixture injected client failure")
				}
				return client, nil
			}
		}
		if input.mode == "injected_session" {
			h.service.OpenSessionFunc = func(value string) (*fanboxsdk.Client, error) {
				h.row.Trace = append(h.row.Trace, "session.factory/"+value)
				return fanboxsdk.OpenWith(fanboxsdk.SessionCredentials{FANBOXSESSID: value}, fanboxsdk.Options{HTTPClient: &http.Client{Transport: transport}})
			}
		}
		rows = append(rows, h.finish(t, func() error {
			if input.mode == "injected_session" || strings.HasPrefix(input.mode, "default_import") {
				v, e := h.service.ImportSessionWithProxy(h.ctx, "fixture-injected-session", false, override)
				h.row.Result = v
				return e
			}
			client, e := h.service.OpenClientWithProxy(h.ctx, override)
			h.row.Result = map[string]any{"client_present": client != nil}
			if client != nil {
				h.row.Options = migrationFanboxPrivateClientOptions(client)
				if e == nil {
					_, e = client.CurrentUser(h.ctx, fanboxsdk.CurrentUserRequest{})
				}
				h.row.Trace = append(h.row.Trace, "caller.client.close")
				client.CloseIdleConnections()
			}
			return e
		}))
	}
	for _, input := range []struct{ name, body string }{
		{"global proxy", "[network]\nhttps_proxy='http://127.0.0.1:19001'\n"},
		{"service proxy", "[network]\nhttps_proxy='http://127.0.0.1:19001'\n[fanbox.network]\nproxy_url='http://127.0.0.1:19002'\nuser_agent='fixture-agent'\n"},
		{"service empty proxy", "[network]\nhttps_proxy='http://127.0.0.1:19001'\n[fanbox.network]\nproxy_url=''\n"},
		{"independent solver", "[fanbox.flaresolverr]\nurl='https://fixture-solver.invalid/v1'\nproxy_url='http://127.0.0.1:19003'\n"},
		{"proxy wrong type", "[fanbox.network]\nproxy_url=7\n"},
		{"agent wrong type", "[fanbox.network]\nuser_agent=7\n"},
		{"solver missing URL", "[fanbox.flaresolverr]\nproxy_url='http://127.0.0.1:19003'\n"},
		{"invalid default does not affect Runtime", "[fanbox.auth]\ndefault_user_id='invalid'\n"},
	} {
		h := migrationFanboxNewServiceHarness(t, "config/runtime/"+input.name, input.body, false)
		h.row.Input = map[string]any{"config": input.body}
		rows = append(rows, h.finish(t, func() error {
			snap, e := h.defaults.store.Current()
			if e != nil {
				return e
			}
			cfg, e := snap.Runtime()
			h.row.Result = map[string]any{"global_proxy": cfg.HTTPSProxy, "fanbox_network": cfg.FanboxNetwork, "fanbox_solver": cfg.FanboxFlareSolverr}
			return e
		}))
	}
	return rows
}
func migrationFanboxPrivateClientOptions(client *fanboxsdk.Client) map[string]any {
	session := reflect.ValueOf(client).Elem().FieldByName("session").Elem()
	var solver any
	value := session.FieldByName("flareSolverr")
	if !value.IsNil() {
		value = value.Elem()
		solver = map[string]any{"url": value.FieldByName("URL").String(), "proxy_url": value.FieldByName("ProxyURL").String()}
	}
	return map[string]any{"proxy_url": session.FieldByName("proxyURL").String(), "user_agent": session.FieldByName("userAgent").String(), "solver": solver}
}
func migrationFanboxServiceFixture(t *testing.T, section any) {
	t.Helper()
	data, e := json.MarshalIndent(section, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	if *captureFanboxSavedService {
		if e = os.WriteFile(filepath.Join(os.TempDir(), "fanbox-saved-accounts-service.json"), data, 0600); e != nil {
			t.Fatal(e)
		}
		return
	}
	body, e := os.ReadFile(filepath.Join("..", "..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "fanbox-saved-accounts.json"))
	if e != nil {
		t.Fatal(e)
	}
	var fixture map[string]json.RawMessage
	if e = json.Unmarshal(body, &fixture); e != nil {
		t.Fatal(e)
	}
	var want any
	decoder := json.NewDecoder(bytes.NewReader(fixture["service"]))
	decoder.UseNumber()
	if e = decoder.Decode(&want); e != nil {
		t.Fatal(e)
	}
	canonical, e := json.MarshalIndent(want, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	canonical = append(canonical, '\n')
	var observed any
	observedDecoder := json.NewDecoder(bytes.NewReader(data))
	observedDecoder.UseNumber()
	if e = observedDecoder.Decode(&observed); e != nil {
		t.Fatal(e)
	}
	actual, e := json.MarshalIndent(observed, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	actual = append(actual, '\n')
	if !bytes.Equal(actual, canonical) {
		path := filepath.Join(t.TempDir(), "service-observed.json")
		if e = os.WriteFile(path, data, 0600); e != nil {
			t.Fatal(e)
		}
		t.Fatalf("FANBOX saved service contracts changed: %s", path)
	}
}

func TestMigrationFanboxSavedNilContextProbe(t *testing.T) {
	action := os.Getenv("PIXIV_MIGRATION_FANBOX_NIL_CONTEXT_ACTION")
	if action == "" {
		t.Skip("owned nil-context subprocess only")
	}
	h := migrationFanboxNewServiceHarness(t, action+"/context/nil_context", "", true)
	h.ctx = nil
	row := h.finish(t, func() error { return migrationFanboxServiceAction(h, action, 7) })
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	fmt.Printf("FANBOX_NIL_PROBE=%s\n", body)
}
func migrationFanboxNilContextSubprocess(t *testing.T, action string) migrationFanboxServiceRow {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationFanboxSavedNilContextProbe$", "-test.timeout=15s")
	command.Env = append(os.Environ(), "PIXIV_MIGRATION_FANBOX_NIL_CONTEXT_ACTION="+action)
	output, err := command.CombinedOutput()
	if err != nil {
		t.Fatalf("nil-context subprocess %s failed: %v: %s", action, err, output)
	}
	for _, line := range strings.Split(string(output), "\n") {
		if strings.HasPrefix(line, "FANBOX_NIL_PROBE=") {
			var row migrationFanboxServiceRow
			if err := json.Unmarshal([]byte(strings.TrimPrefix(line, "FANBOX_NIL_PROBE=")), &row); err != nil {
				t.Fatal(err)
			}
			if !row.DatabasePoisoned || row.Panic == "" {
				t.Fatalf("nil-context subprocess did not record poisoned panic: %+v", row)
			}
			return row
		}
	}
	t.Fatalf("nil-context subprocess %s returned no observation: %s", action, output)
	return migrationFanboxServiceRow{}
}
