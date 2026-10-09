package cli

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"flag"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	dbstorage "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateAuthAccounts = flag.Bool("migration-update-auth-accounts", false, "capture isolated auth account commands")

type authAccountCase struct {
	userWorksStartupCase
	MixedMax         bool               `json:"mixed_max"`
	MaxYear          bool               `json:"max_year"`
	LargeYear        bool               `json:"large_year"`
	Seed             bool               `json:"seed"`
	States           []authAccountState `json:"states"`
	Environment      map[string]string  `json:"environment"`
	ShortWrite       bool               `json:"short_write"`
	WriteError       bool               `json:"write_error"`
	DiagnosticsError bool               `json:"diagnostics_error"`
}

type authAccountState struct {
	ID          int64  `json:"id"`
	Revision    int64  `json:"revision"`
	Schedulable int64  `json:"schedulable"`
	Frozen      *int64 `json:"frozen"`
	Marker      int64  `json:"marker"`
}

type migrationAuthWriter struct{}

type migrationAuthShortWriter struct{}

func (migrationAuthShortWriter) Write(p []byte) (int, error) { return 0, nil }

func (migrationAuthWriter) Write([]byte) (int, error) { return 0, errors.New("fixture write failed") }
func TestMigrationAuthAccountsChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_AUTH_ACCOUNT_CHILD")
	if encoded == "" {
		t.Skip("isolated config helper")
	}
	var row authAccountCase
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	home := os.Getenv("HOME")
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if row.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if row.Seed {
		db, err := dbstorage.Open(directory)
		if err != nil {
			t.Fatal(err)
		}
		for _, id := range []int64{2, 1} {
			a := account.New(id, map[int64]string{1: "", 2: "合成-user"}[id], []byte("synthetic-secret"))
			a.PoolLastSelected = id == 2
			if row.MixedMax && id == 1 {
				f := int64(4102444800)
				a.PoolFrozenUntil = &f
			}
			future := int64(4102444800)
			if row.LargeYear {
				future = 253402300800
			}
			if id == 2 {
				if row.MaxYear {
					future = 9223372036854775807
				}
				a.PoolFrozenUntil = &future
				premium := false
				a.PremiumStatus = &premium
			}
			if err := db.SavePixivCredential(context.Background(), a); err != nil {
				t.Fatal(err)
			}
		}
		if err := db.SetPixivSchedulable(context.Background(), []int64{2}, false); err != nil {
			t.Fatal(err)
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	var outputWriter io.Writer = &out
	var diagnosticWriter io.Writer = &diagnostics
	if row.ShortWrite {
		outputWriter = migrationAuthShortWriter{}
	}
	if row.WriteError {
		outputWriter = migrationAuthWriter{}
	}
	if row.DiagnosticsError {
		diagnosticWriter = migrationAuthWriter{}
	}
	if row.ReadError {
		row.Exit = Run(append([]string{"pixiv", "auth"}, row.Args...), migrationSearchFailedRead{}, outputWriter, diagnosticWriter)
	} else {
		row.Exit = Run(append([]string{"pixiv", "auth"}, row.Args...), strings.NewReader(row.Input), outputWriter, diagnosticWriter)
	}
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = strings.ReplaceAll(diagnostics.String(), home, "<HOME>")
	_, err := os.Stat(path)
	row.Config = err == nil
	_, err = os.Stat(filepath.Join(directory, "pixiv-cli.db"))
	row.Database = err == nil
	if row.Config {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(data)
	}
	if row.Database {
		db, err := sql.Open("sqlite", filepath.Join(directory, "pixiv-cli.db"))
		if err != nil {
			t.Fatal(err)
		}
		rs, err := db.Query("SELECT user_id,credential_revision,schedulable,pool_frozen_until,pool_last_selected FROM pixiv_account ORDER BY sort_order")
		if err != nil {
			t.Fatal(err)
		}
		row.States = make([]authAccountState, 0)
		for rs.Next() {
			var id, rev, on, marker int64
			var frozen sql.NullInt64
			if err := rs.Scan(&id, &rev, &on, &frozen, &marker); err != nil {
				t.Fatal(err)
			}
			var f *int64
			if frozen.Valid {
				value := frozen.Int64
				f = &value
			}
			row.States = append(row.States, authAccountState{ID: id, Revision: rev, Schedulable: on, Frozen: f, Marker: marker})
		}
		rs.Close()
		db.Close()
	}
	data, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
func TestMigrationAuthAccounts(t *testing.T) {
	var rows []authAccountCase
	add := func(name string, args ...string) {
		rows = append(rows, authAccountCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}, Environment: map[string]string{}})
	}
	text := func(s string) *string { return &s }
	add("group")
	add("group-extra", "extra")
	add("pool-group", "pool")
	add("pool-extra", "pool", "extra")
	for _, op := range [][]string{{"list"}, {"pool", "status"}, {"pool", "enable"}, {"pool", "disable"}} {
		add(strings.Join(op, "-")+"-help", append(op, "--help")...)
	}
	for _, op := range [][]string{{"list"}, {"pool", "status"}} {
		name := strings.Join(op, "-")
		add(name, op...)
		add(name+"-json", append(op, "--json")...)
		add(name+"-extra", append(op, "x")...)
		add(name+"-noinput", op...)
		rows[len(rows)-1].ReadError = true
		add(name+"-writer", op...)
		rows[len(rows)-1].WriteError = true
		add(name+"-json-writer", append(op, "--json")...)
		rows[len(rows)-1].WriteError = true
		add(name+"-badconfig", op...)
		rows[len(rows)-1].Before = text("[unfinished\n")
		add(name+"-config-json", op...)
		rows[len(rows)-1].Before = text("[output]\njson=true\n")
		for _, value := range []string{"TRUE", "0", "bad", ""} {
			add(name+"-bool-"+value, append(op, "--json="+value)...)
		}
	}
	for _, name := range []string{"enable", "disable"} {
		add(name+"-empty", "pool", name)
		add(name+"-all", "pool", name, "--all")
		add(name+"-all-json", "pool", name, "--all", "--json")
		add(name+"-all-uid", "pool", name, "--all", "1")
		add(name+"-stdin", "pool", name)
		rows[len(rows)-1].Input = "1\n"
		add(name+"-stdin-whole", "pool", name)
		rows[len(rows)-1].Input = "1\n2\n"
		add(name+"-read-error", "pool", name)
		rows[len(rows)-1].ReadError = true
		for _, uid := range []string{"1", "+1", "0", "9223372036854775808", "0x10", "  ", "1_0", "a\a"} {
			add(name+"-uid-"+uid, "pool", name, "--", uid)
		}
		add(name+"-invalid-config-first", "pool", name, "bad")
		rows[len(rows)-1].Before = text("[unfinished\n")
		add(name+"-all-false", "pool", name, "--all=false")
		add(name+"-unknown", "pool", name, "--unknown")
	}
	for _, args := range [][]string{{"list"}, {"list", "--json"}, {"pool", "status"}, {"pool", "status", "--json"}, {"pool", "enable", "2", "1", "--json"}, {"pool", "disable", "1"}, {"pool", "enable", "2", "99"}, {"pool", "disable", "1", "1"}, {"pool", "disable", "--all", "--json"}} {
		add("seed-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].Before = text("[pixiv.auth]\ndefault_user_id=1\n[account_pool]\nenabled=true\nstrategy='random'\n")
	}
	for _, args := range [][]string{{}, {"pool"}, {"list", "--json"}, {"pool", "status", "--json"}, {"pool", "enable", "--all", "--json"}} {
		add("short-writer-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].ShortWrite = true
	}
	for _, args := range [][]string{{"-h", "list"}, {"--help", "pool", "status"}, {"pool", "--help", "status"}, {"--json", "list"}, {"pool", "--json", "status"}, {"--help=false", "list"}, {"pool", "--all", "enable"}} {
		add("discovery-"+strings.Join(args, "-"), args...)
	}
	for _, args := range [][]string{{"list"}, {"list", "--json"}, {"pool", "status"}, {"pool", "status", "--json"}} {
		add("large-year-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].LargeYear = true
	}
	for _, args := range [][]string{{"list", "extra", "--json"}, {"pool", "status", "extra", "--json=false"}, {"pool", "enable", "--json=false"}, {"pool", "enable", "bad", "--json=false"}, {"list", "--json=false", "--json=bad"}} {
		add("json-errors-"+strings.Join(args, "-"), args...)
	}
	for _, args := range [][]string{{"list"}, {"list", "--json"}, {"pool", "status"}, {"pool", "status", "--json"}} {
		add("max-year-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].MaxYear = true
	}
	for _, args := range [][]string{{"pool", "status"}, {"pool", "status", "--json"}} {
		add("mixed-max-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].MaxYear = true
		rows[len(rows)-1].MixedMax = true
	}
	for index, row := range rows {
		home := t.TempDir()
		data, err := json.Marshal(row)
		if err != nil {
			t.Fatal(err)
		}
		child := exec.Command(os.Args[0], "-test.run=^TestMigrationAuthAccountsChild$")
		for _, entry := range os.Environ() {
			key, _, _ := strings.Cut(entry, "=")
			switch key {
			case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY":
				continue
			}
			child.Env = append(child.Env, entry)
		}
		child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_AUTH_ACCOUNT_CHILD="+string(data))
		for key, value := range row.Environment {
			child.Env = append(child.Env, key+"="+value)
		}
		if out, err := child.CombinedOutput(); err != nil {
			t.Fatalf("%s: %v %s", row.Name, err, out)
		}
		data, err = os.ReadFile(filepath.Join(home, "result.json"))
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(data, &rows[index]); err != nil {
			t.Fatal(err)
		}
	}
	recommendedFixture(t, "cli-auth-accounts.json", rows, *updateAuthAccounts)
}
