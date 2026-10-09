package loginhelper_test

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
	filelock "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/lock"
)

func TestMigrationHandoffStateJSON(t *testing.T) {
	cases := []struct {
		name, body string
		valid      bool
		session    string
	}{
		{"basic", `{"version":1,"origin":" https://relay.example/ ","session_id":" session ","proof":" proof "}`, true, " session "},
		{"casefold", `{"VERSION":1,"ORIGIN":"https://relay.example","SESSION_ID":"session","PROOF":"proof"}`, true, "session"},
		{"unicode_fold", `{"verſion":1,"origin":"https://relay.example","ſeſſion_id":"session","proof":"proof"}`, true, "session"},
		{"duplicates", `{"version":0,"VERSION":1,"origin":"https://relay.example","session_id":"old","SESSION_ID":"new","proof":"proof"}`, true, "new"},
		{"null_preserves", `{"version":1,"version":null,"origin":"https://relay.example","origin":null,"session_id":"session","session_id":null,"proof":"proof","proof":null}`, true, "session"},
		{"unknown", `{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof","ttl":-1,"unknown":{"deep":[null]}}`, true, "session"},
		{"null", `null`, false, ""},
		{"empty", ``, false, ""},
		{"float", `{"version":1.0,"origin":"https://relay.example","session_id":"session","proof":"proof"}`, false, ""},
		{"bad_then_good", `{"version":"bad","version":1,"origin":"https://relay.example","session_id":"session","proof":"proof"}`, false, ""},
		{"blank", `{"version":1,"origin":"https://relay.example","session_id":"\u0085\u00a0","proof":"proof"}`, false, ""},
		{"wrong_name", `{"version":1,"origin":"https://relay.example","SessionID":"session","proof":"proof"}`, false, ""},
		{"trailing", `{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof"} {}`, false, ""},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "state.json")
			if err := os.WriteFile(path, []byte(c.body), 0600); err != nil {
				t.Fatal(err)
			}
			active, err := loginhelper.LoadActiveRemoteLoginAt(path)
			if !c.valid {
				if err == nil || err.Error() != "active remote login handoff is invalid" {
					t.Fatalf("got %v", err)
				}
				return
			}
			if err != nil || active.Version != 1 || active.Origin != "https://relay.example" || active.SessionID != c.session || active.Proof != "proof" && c.name != "basic" {
				t.Fatalf("got %#v, %v", active, err)
			}
		})
	}
}

func TestMigrationHandoffStateSaveAndErrors(t *testing.T) {
	path := filepath.Join(t.TempDir(), "private", "state.json")
	if _, err := loginhelper.LoadActiveRemoteLoginAt(path); !errors.Is(err, loginhelper.ErrNoActiveRemoteLogin) {
		t.Fatal(err)
	}
	invalid := loginhelper.ActiveRemoteLogin{Version: 7, Origin: "bad", SessionID: "", Proof: "synthetic"}
	if err := loginhelper.SaveActiveRemoteLoginAt(path, invalid); err != nil {
		t.Fatal(err)
	}
	body, _ := os.ReadFile(path)
	if string(body) != `{"version":7,"origin":"bad","session_id":"","proof":"synthetic"}` {
		t.Fatal(string(body))
	}
	if _, err := loginhelper.LoadActiveRemoteLoginAt(filepath.Dir(path)); err == nil || err.Error() != "could not read active remote login handoff" {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		for p, mode := range map[string]os.FileMode{path: 0600, filepath.Dir(path): 0700} {
			info, err := os.Stat(p)
			if err != nil || info.Mode().Perm() != mode {
				t.Fatalf("%s: %v %v", p, info, err)
			}
		}
	}
}

func TestMigrationHandoffPrivateLockProcess(t *testing.T) {
	if path := os.Getenv("PIXIV_MIGRATION_LOCK_PATH"); path != "" {
		if err := os.WriteFile(path+".ready", []byte("ready"), 0600); err != nil {
			t.Fatal(err)
		}
		err := filelock.WithPrivateLock(context.Background(), path, func() error { return os.WriteFile(path+".acquired", []byte("yes"), 0600) })
		if err != nil {
			t.Fatal(err)
		}
		return
	}
	path := filepath.Join(t.TempDir(), "state")
	var child *exec.Cmd
	childDone := make(chan error, 1)
	err := filelock.WithPrivateLock(context.Background(), path, func() error {
		child = exec.Command(os.Args[0], "-test.run=^TestMigrationHandoffPrivateLockProcess$")
		child.Env = append(os.Environ(), "PIXIV_MIGRATION_LOCK_PATH="+path)
		if err := child.Start(); err != nil {
			return err
		}
		go func() { childDone <- child.Wait() }()
		deadline := time.Now().Add(10 * time.Second)
		for {
			select {
			case err := <-childDone:
				t.Fatalf("lock child exited before readiness: %v", err)
			default:
			}
			if time.Now().After(deadline) {
				_ = child.Process.Kill()
				t.Fatal("lock child did not become ready")
			}
			if _, err := os.Stat(path + ".ready"); err == nil {
				break
			}
			time.Sleep(5 * time.Millisecond)
		}
		time.Sleep(50 * time.Millisecond)
		if _, err := os.Stat(path + ".acquired"); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("second process acquired held lock")
		}
		if err := loginhelper.SaveActiveRemoteLoginAt(path, loginhelper.ActiveRemoteLogin{Version: 1, Origin: "https://relay.example", SessionID: "replacement", Proof: "proof"}); err != nil {
			return err
		}
		time.Sleep(50 * time.Millisecond)
		if _, err := os.Stat(path + ".acquired"); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("second process acquired after target replacement")
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := <-childDone; err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path + ".acquired"); err != nil {
		t.Fatal(err)
	}
	canceled, cancel := context.WithCancel(context.Background())
	cancel()
	if err := filelock.WithPrivateLock(canceled, path+"-cancel", nil); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if err := filelock.WithPrivateLock(context.Background(), path, nil); err == nil || !strings.Contains(err.Error(), "action is not configured") {
		t.Fatal(err)
	}
}

func TestMigrationHandoffConditionalClear(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	start := loginhelper.RemoteLoginStart{Origin: " https://relay.example/ ", SessionID: "old", Proof: "proof"}
	if err := loginhelper.ClearRemoteLoginHandoff(start); err != nil {
		t.Fatal(err)
	}
	active := loginhelper.ActiveRemoteLogin{Version: 1, Origin: "https://relay.example", SessionID: "new", Proof: "proof"}
	if err := loginhelper.SaveActiveRemoteLogin(active); err != nil {
		t.Fatal(err)
	}
	if err := loginhelper.ClearRemoteLoginHandoff(start); err != nil {
		t.Fatal(err)
	}
	got, err := loginhelper.LoadActiveRemoteLogin()
	if err != nil || got != active {
		t.Fatalf("%#v %v", got, err)
	}
	for _, stale := range []loginhelper.RemoteLoginStart{{Origin: active.Origin, SessionID: active.SessionID, Proof: "proof "}, {Origin: active.Origin, SessionID: "new ", Proof: active.Proof}, {Origin: "https://different.example", SessionID: active.SessionID, Proof: active.Proof}} {
		if err := loginhelper.ClearRemoteLoginHandoff(stale); err != nil {
			t.Fatal(err)
		}
		got, err = loginhelper.LoadActiveRemoteLogin()
		if err != nil || got != active {
			t.Fatalf("%#v %v", got, err)
		}
	}
	start = loginhelper.RemoteLoginStart{Origin: " https://relay.example/ ", SessionID: active.SessionID, Proof: active.Proof}
	if err := loginhelper.ClearRemoteLoginHandoff(start); err != nil {
		t.Fatal(err)
	}
	if _, err := loginhelper.LoadActiveRemoteLogin(); !errors.Is(err, loginhelper.ErrNoActiveRemoteLogin) {
		t.Fatal(err)
	}
	path, err := loginhelper.ActiveRemoteLoginPath()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path + ".lock"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("synthetic-invalid"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := loginhelper.ClearRemoteLoginHandoff(start); err == nil || err.Error() != "could not clear active remote login handoff" {
		t.Fatal(err)
	}
	start.SessionID = " "
	if err := loginhelper.ClearRemoteLoginHandoff(start); err == nil || err.Error() != "invalid remote login handoff" {
		t.Fatal(err)
	}
}

func TestMigrationHandoffGoJSONStringEncoding(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	active := loginhelper.ActiveRemoteLogin{Version: 7, Origin: "bad", Proof: "<&>\u2028\u2029"}
	if err := loginhelper.SaveActiveRemoteLoginAt(path, active); err != nil {
		t.Fatal(err)
	}
	body, _ := os.ReadFile(path)
	if string(body) != `{"version":7,"origin":"bad","session_id":"","proof":"\u003c\u0026\u003e\u2028\u2029"}` {
		t.Fatal(string(body))
	}
	prefix := `{"version":1,"origin":"https://relay.example","session_id":"`
	for _, value := range []struct {
		raw  []byte
		want string
	}{{[]byte(`\ud800`), "�"}, {[]byte{0xff, 0xfe}, "��"}} {
		body := append([]byte(prefix), value.raw...)
		body = append(body, []byte(`","proof":"proof"}`)...)
		if err := os.WriteFile(path, body, 0600); err != nil {
			t.Fatal(err)
		}
		got, err := loginhelper.LoadActiveRemoteLoginAt(path)
		if err != nil || got.SessionID != value.want {
			t.Fatalf("%#v %v", got, err)
		}
	}
}

func TestMigrationHandoffJSONDepth(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	for _, depth := range []int{129, 9999, 10000} {
		body := `{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof","unknown":` + strings.Repeat("[", depth) + "null" + strings.Repeat("]", depth) + "}"
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
		_, err := loginhelper.LoadActiveRemoteLoginAt(path)
		if depth < 10000 {
			if err != nil {
				t.Fatal(err)
			}
		} else if err == nil || err.Error() != "active remote login handoff is invalid" {
			t.Fatal(err)
		}
	}
}

func TestMigrationHandoffCancelAfterNativeAcquire(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ready := make(chan struct{})
	done := make(chan error, 1)
	if err := filelock.WithPrivateLock(context.Background(), path, func() error {
		go func() {
			close(ready)
			done <- filelock.WithPrivateLock(ctx, path, func() error { return errors.New("cancelled action ran") })
		}()
		<-ready
		time.Sleep(50 * time.Millisecond)
		cancel()
		time.Sleep(50 * time.Millisecond)
		select {
		case err := <-done:
			t.Fatalf("native wait returned before lock release: %v", err)
		default:
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatal(err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("cancelled waiter did not finish after release")
	}
	if err := filelock.WithPrivateLock(context.Background(), path, func() error { return nil }); err != nil {
		t.Fatal(err)
	}
}
