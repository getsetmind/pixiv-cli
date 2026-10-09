package auth_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"

	"github.com/pkg/browser"
)

func TestMigrationNativeBrowserProviders(t *testing.T) {
	if runtime.GOOS != "linux" && runtime.GOOS != "darwin" {
		t.Skip("synthetic POSIX providers only; no native browser is launched")
	}
	body, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/native_browser.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		URL   string
		Linux []struct {
			Name      string
			Providers []string
			Selected  string
			Exit      int
		}
	}
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS == "darwin" {
		fixture.Linux = nil
		fixture.Linux = append(fixture.Linux, struct {
			Name      string
			Providers []string
			Selected  string
			Exit      int
		}{"darwin open", []string{"open"}, "open", 0})
		fixture.Linux = append(fixture.Linux, struct {
			Name      string
			Providers []string
			Selected  string
			Exit      int
		}{"darwin failure", []string{"open"}, "open", 19})
	}
	for _, c := range fixture.Linux {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			bin := filepath.Join(home, "bin")
			if err := os.Mkdir(bin, 0700); err != nil {
				t.Fatal(err)
			}
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("PATH", bin)
			t.Setenv("BROWSER", "must-not-be-used")
			t.Setenv("SYNTHETIC_BROWSER_ENV", "inherited")
			t.Chdir(home)
			var stdout, stderr bytes.Buffer
			oldOut, oldErr := browser.Stdout, browser.Stderr
			browser.Stdout, browser.Stderr = &stdout, &stderr
			t.Cleanup(func() { browser.Stdout, browser.Stderr = oldOut, oldErr })
			for _, provider := range c.Providers {
				script := "#!/bin/sh\nprintf '%s\\n' '" + provider + "' \"$#\" \"$1\" \"$PWD\" \"$HOME\" \"$SYNTHETIC_BROWSER_ENV\" > \"$HOME/invocation\"\nif read value; then printf 'stdin-data\\n' >> \"$HOME/invocation\"; else printf 'stdin-eof\\n' >> \"$HOME/invocation\"; fi\nprintf 'provider-output\\n'\nprintf 'provider-error\\n' >&2\nprintf 'finished\\n' >> \"$HOME/invocation\"\nexit "
				if provider == c.Selected {
					script += strconv.Itoa(c.Exit)
				} else {
					script += "0"
				}
				if err := os.WriteFile(filepath.Join(bin, provider), []byte(script+"\n"), 0700); err != nil {
					t.Fatal(err)
				}
			}
			err := browser.OpenURL(fixture.URL)
			if c.Selected == "" {
				var missing *exec.Error
				if !errors.As(err, &missing) || !errors.Is(err, exec.ErrNotFound) || missing.Name != "xdg-open,x-www-browser,www-browser" {
					t.Fatalf("missing provider error = %v", err)
				}
				if _, e := os.Stat(filepath.Join(home, "invocation")); !errors.Is(e, os.ErrNotExist) {
					t.Fatalf("provider unexpectedly invoked: %v", e)
				}
				return
			}
			if c.Exit == 0 && err != nil {
				t.Fatal(err)
			}
			if c.Exit != 0 {
				var exited *exec.ExitError
				if !errors.As(err, &exited) || exited.ExitCode() != c.Exit {
					t.Fatalf("exit error = %v", err)
				}
			}
			got, e := os.ReadFile(filepath.Join(home, "invocation"))
			if e != nil {
				t.Fatal(e)
			}
			want := strings.Join([]string{c.Selected, "1", fixture.URL, home, home, "inherited", "stdin-eof", "finished", ""}, "\n")
			if string(got) != want {
				t.Fatalf("invocation = %q, want %q", got, want)
			}
			if stdout.String() != "provider-output\n" || stderr.String() != "provider-error\n" {
				t.Fatalf("stdio = %q / %q", stdout.String(), stderr.String())
			}
		})
	}
}

func TestMigrationNativeBrowserFirstLaunchFailureDoesNotFallThrough(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("Linux provider lookup contract")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", home)
	for name, script := range map[string]string{
		"xdg-open":      "#!/synthetic/interpreter/does-not-exist\n",
		"x-www-browser": "#!/bin/sh\nprintf 'fallback' > \"$HOME/invocation\"\n",
	} {
		if err := os.WriteFile(filepath.Join(home, name), []byte(script), 0700); err != nil {
			t.Fatal(err)
		}
	}
	err := browser.OpenURL("https://synthetic.invalid/")
	var pathError *os.PathError
	if !errors.As(err, &pathError) {
		t.Fatalf("launch error = %v", err)
	}
	want := "fork/exec " + filepath.Join(home, "xdg-open") + ": no such file or directory"
	if err.Error() != want {
		t.Fatalf("launch diagnostic = %q, want %q", err.Error(), want)
	}
	if _, e := os.Stat(filepath.Join(home, "invocation")); !errors.Is(e, os.ErrNotExist) {
		t.Fatalf("fallback invoked: %v", e)
	}
}

func TestMigrationNativeBrowserSkipsNonExecutableProvider(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("Linux provider lookup contract")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", home)
	if err := os.WriteFile(filepath.Join(home, "xdg-open"), []byte("not executable"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "www-browser"), []byte("#!/bin/sh\nprintf 'selected' > \"$HOME/invocation\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := browser.OpenURL("https://synthetic.invalid/"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(filepath.Join(home, "invocation"))
	if err != nil || string(got) != "selected" {
		t.Fatalf("selection = %q, %v", got, err)
	}
}

func TestMigrationNativeBrowserSkipsRelativePATHMatch(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("Linux provider lookup contract")
	}
	home := t.TempDir()
	absolute := filepath.Join(home, "absolute")
	if err := os.Mkdir(absolute, 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("PATH", "."+string(os.PathListSeparator)+absolute)
	t.Chdir(home)
	relative := filepath.Join(home, "xdg-open")
	if err := os.WriteFile(relative, []byte("#!/bin/sh\nprintf 'relative' > \"$HOME/invocation\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	_, err := exec.LookPath("xdg-open")
	if !errors.Is(err, exec.ErrDot) {
		t.Fatalf("relative provider lookup = %v; requires default execerrdot policy", err)
	}
	if err := os.WriteFile(filepath.Join(absolute, "www-browser"), []byte("#!/bin/sh\nprintf 'absolute' > \"$HOME/invocation\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := browser.OpenURL("https://synthetic.invalid/"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(filepath.Join(home, "invocation"))
	if err != nil || string(got) != "absolute" {
		t.Fatalf("selection = %q, %v", got, err)
	}
}
