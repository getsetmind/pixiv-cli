//go:build !windows

package secret_test

import (
	secret "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/secret"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestMigrationExportWriterWorkflow(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "existing")
	if err := os.Mkdir(dir, 0751); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "synthetic-secret-path")
	if err := secret.WriteSecretFile(path, []byte("old"), false); err != nil {
		t.Fatal(err)
	}
	if err := secret.WriteSecretFile(path, []byte("new"), false); err == nil || strings.Contains(err.Error(), path) || !strings.Contains(err.Error(), "destination already exists") {
		t.Fatalf("unsafe collision: %v", err)
	}
	if err := secret.WriteSecretFile(path, []byte("new"), true); err != nil {
		t.Fatal(err)
	}
	body, _ := os.ReadFile(path)
	if string(body) != "new" {
		t.Fatal(string(body))
	}
	info, _ := os.Stat(path)
	if info.Mode().Perm() != 0600 {
		t.Fatal(info.Mode())
	}
	info, _ = os.Stat(dir)
	if info.Mode().Perm() != 0751 {
		t.Fatal(info.Mode())
	}
	entries, _ := os.ReadDir(dir)
	if len(entries) != 1 {
		t.Fatal(entries)
	}
	target := filepath.Join(dir, "target")
	os.WriteFile(target, []byte("untouched"), 0600)
	link := filepath.Join(dir, "link")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	if err := secret.WriteSecretFile(link, []byte("replacement"), false); err == nil {
		t.Fatal("symlink exclusive write succeeded")
	}
	if err := secret.WriteSecretFile(link, []byte("replacement"), true); err != nil {
		t.Fatal(err)
	}
	body, _ = os.ReadFile(target)
	if string(body) != "untouched" {
		t.Fatal("followed symlink")
	}
	info, _ = os.Lstat(link)
	if !info.Mode().IsRegular() {
		t.Fatal(info.Mode())
	}
	if err := secret.WriteSecretFile(dir, []byte("invalid"), true); err == nil || strings.Contains(err.Error(), dir) {
		t.Fatal(err)
	}
	entries, _ = os.ReadDir(dir)
	if len(entries) != 3 {
		t.Fatal(entries)
	}
	nested := filepath.Join(dir, "new", "deeper", "export")
	if err := secret.WriteSecretFile(nested, []byte("synthetic"), false); err != nil {
		t.Fatal(err)
	}
	info, _ = os.Stat(filepath.Dir(nested))
	if info.Mode().Perm() != 0700 {
		t.Fatal(info.Mode())
	}
}

func TestMigrationExportWriterNonDirectoryParent(t *testing.T) {
	parent := filepath.Join(t.TempDir(), "synthetic-parent")
	if err := os.WriteFile(parent, []byte("untouched"), 0600); err != nil {
		t.Fatal(err)
	}
	for _, force := range []bool{false, true} {
		err := secret.WriteSecretFile(filepath.Join(parent, "output"), []byte("secret"), force)
		if err == nil || err.Error() != "authentication export write failed: filesystem operation failed (not_committed)" {
			t.Fatalf("got %v", err)
		}
	}
}
