package loginhelper_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
)

func TestMigrationHandlerManifestFixture(t *testing.T) {
	body, err := os.ReadFile("../../../../../../crates/pixiv-app/tests/fixtures/handler_manifest.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name   string
		Input  string
		Output *string
	}
	if err := json.Unmarshal(body, &cases); err != nil {
		t.Fatal(err)
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	path, err := loginhelper.HandlerManifestPath()
	if err != nil {
		t.Fatal(err)
	}
	if err = os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			if err := os.WriteFile(path, []byte(c.Input), 0600); err != nil {
				t.Fatal(err)
			}
			manifest, exists, err := loginhelper.LoadHandlerManifest()
			if c.Output == nil {
				if err == nil || exists || err.Error() != "Pixiv URL handler manifest is invalid" {
					t.Fatalf("unexpected result: %v %v", exists, err)
				}
				return
			}
			if err != nil || !exists {
				t.Fatalf("load: %v %v", exists, err)
			}
			encoded, err := json.Marshal(manifest)
			if err != nil || string(encoded) != *c.Output {
				t.Fatalf("marshal: %s, %v; want %s", encoded, err, *c.Output)
			}
		})
	}
	for _, depth := range []int{9999, 10000} {
		input := `{"version":1,"executable_path":"p","future":` + strings.Repeat("[", depth) + "0" + strings.Repeat("]", depth) + "}"
		if err = os.WriteFile(path, []byte(input), 0600); err != nil {
			t.Fatal(err)
		}
		_, exists, err := loginhelper.LoadHandlerManifest()
		if depth == 9999 && (err != nil || !exists) || depth == 10000 && (err == nil || exists) {
			t.Fatalf("depth %d: %v %v", depth, exists, err)
		}
	}
	input := append([]byte(`{"version":1,"executable_path":"`), 0xff, 0xfe)
	input = append(input, []byte(`"}`)...)
	if err = os.WriteFile(path, input, 0600); err != nil {
		t.Fatal(err)
	}
	m, _, err := loginhelper.LoadHandlerManifest()
	if err != nil || m.ExecutablePath != "��" {
		t.Fatalf("nonUTF8: %q %v", m.ExecutablePath, err)
	}
}

func TestMigrationHandlerManifestStorage(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	path, err := loginhelper.HandlerManifestPath()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasSuffix(path, filepath.Join(".pixiv-cli", "url-handler", "handler-manifest.json")) {
		t.Fatal(path)
	}
	if _, exists, err := loginhelper.LoadHandlerManifest(); err != nil || exists {
		t.Fatalf("missing: %v %v", exists, err)
	}
	if err = loginhelper.RemoveHandlerManifest(); err != nil {
		t.Fatal(err)
	}
	for _, version := range []int{0, -2, 2} {
		m := loginhelper.HandlerManifest{Version: version, LinuxMIMESnapshots: []loginhelper.HandlerFileSnapshot{}}
		if err = loginhelper.SaveHandlerManifest(m); err != nil {
			t.Fatal(err)
		}
		want := version
		if want == 0 {
			want = 1
		}
		m.Version = want
		expected, _ := json.Marshal(m)
		actual, err := os.ReadFile(path)
		if err != nil || string(actual) != string(expected) {
			t.Fatalf("save: %s %v", actual, err)
		}
		if info, err := os.Stat(path); err != nil || (runtime.GOOS != "windows" && info.Mode().Perm() != 0600) {
			t.Fatalf("mode: %v %v", info, err)
		}
	}
	if err = loginhelper.RemoveHandlerManifest(); err != nil {
		t.Fatal(err)
	}
	if err = os.Mkdir(path, 0700); err != nil {
		t.Fatal(err)
	}
	if _, _, err = loginhelper.LoadHandlerManifest(); err == nil || err.Error() == "Pixiv URL handler manifest is invalid" {
		t.Fatalf("read directory: %v", err)
	}
	if err = os.WriteFile(filepath.Join(path, "child"), []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = loginhelper.RemoveHandlerManifest(); err == nil {
		t.Fatal("nonempty directory removal succeeded")
	}
	if err = loginhelper.SaveHandlerManifest(loginhelper.HandlerManifest{}); err == nil {
		t.Fatal("directory save succeeded")
	}
}

func TestMigrationHandlerManifestNilAndEmptySlices(t *testing.T) {
	var nilManifest, emptyManifest loginhelper.HandlerManifest
	if err := json.Unmarshal([]byte(`{"version":1,"executable_path":"p","linux_mime_snapshots":null}`), &nilManifest); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal([]byte(`{"version":1,"executable_path":"p","linux_mime_snapshots":[]}`), &emptyManifest); err != nil {
		t.Fatal(err)
	}
	if nilManifest.LinuxMIMESnapshots != nil || emptyManifest.LinuxMIMESnapshots == nil {
		t.Fatal("nil and empty slices merged")
	}
	var snapshots []loginhelper.HandlerFileSnapshot
	if err := json.Unmarshal([]byte(`[{"content":[]},{"content":null}]`), &snapshots); err != nil {
		t.Fatal(err)
	}
	if snapshots[0].Content == nil || snapshots[1].Content != nil {
		t.Fatal("nil and empty byte slices merged")
	}
}
