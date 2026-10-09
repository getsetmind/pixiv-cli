package loginpage_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginpage"
)

type pageContract struct {
	Name   string `json:"name"`
	URL    string `json:"url"`
	Href   string `json:"href"`
	SHA256 string `json:"sha256"`
	Writes []int  `json:"writes"`
}
type pageWriter struct {
	bytes.Buffer
	writes []int
}

func (w *pageWriter) Write(p []byte) (int, error) {
	w.writes = append(w.writes, len(p))
	return w.Buffer.Write(p)
}

type errorPageWriter struct{ err error }

func (w errorPageWriter) Write([]byte) (int, error) { return 0, w.err }

type shortPageWriter struct{}

func (shortPageWriter) Write([]byte) (int, error) { return 0, nil }
func renderContract(w io.Writer, name, url string) error {
	switch name {
	case "callback":
		return loginpage.WriteCallbackRelay(w)
	case "success":
		return loginpage.WriteResult(w, true)
	case "failure":
		return loginpage.WriteResult(w, false)
	default:
		return loginpage.WriteManual(w, url)
	}
}
func TestRustLoginPageContracts(t *testing.T) {
	urls := []string{"", "https://app-api.pixiv.net/web/v1/login?state=one&code_challenge=two", "javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,<script>alert(1)</script>", "pixiv://account/login?code=fake", "http://example.test/a?x=\"'<>+&y=(x)", "HTTPS://example.test/a", "mailto:user@example.test?subject=a+b&body=test", "/relative/path?x=a&y=b", "//example.test/a", "relative/path:allowed", "relative:blocked", "#fragment", "?x=日本語&y=é", "https://example.test/%2f/%AF/%zz/%", " https://example.test/a", "https://example.test/a\x00\t\r\n b", "https://example.test/!#$&*+,/:;=?@[]-._~", "https://example.test/?x=&amp;", "httpſ://example.test", "Kttp://example.test", "https://example.test/{{.Title}}?x={{.Stylesheet}}&y={{template \"content\" .}}"}
	var actual []pageContract
	for _, url := range urls {
		var out pageWriter
		if err := renderContract(&out, "manual", url); err != nil {
			t.Fatal(err)
		}
		body := out.String()
		_, tail, ok := strings.Cut(body, `id="pixiv-login-link" href="`)
		if !ok {
			t.Fatal("missing login href")
		}
		href, _, ok := strings.Cut(tail, `"`)
		if !ok {
			t.Fatal("missing quote")
		}
		sum := sha256.Sum256(out.Bytes())
		actual = append(actual, pageContract{Name: "manual", URL: url, Href: href, SHA256: hex.EncodeToString(sum[:]), Writes: out.writes})
	}
	for _, name := range []string{"callback", "success", "failure"} {
		var out pageWriter
		if err := renderContract(&out, name, ""); err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(out.Bytes())
		actual = append(actual, pageContract{Name: name, SHA256: hex.EncodeToString(sum[:]), Writes: out.writes})
	}
	path := filepath.Join("..", "..", "..", "..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "login_page.json")
	if os.Getenv("PIXIV_LOGIN_PAGE_UPDATE") == "1" {
		data, err := json.MarshalIndent(actual, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, append(data, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []pageContract
	if err = json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(actual, expected) {
		t.Fatalf("Go login pages changed: actual=%+v", actual)
	}
}
func TestRustLoginPageWriterFailures(t *testing.T) {
	sentinel := errors.New("page writer failed")
	for _, name := range []string{"manual", "callback", "success", "failure"} {
		if err := renderContract(errorPageWriter{sentinel}, name, "https://example.test/"); !errors.Is(err, sentinel) {
			t.Fatalf("%s error = %v", name, err)
		}
		if err := renderContract(shortPageWriter{}, name, "https://example.test/"); err != nil {
			t.Fatalf("%s short write = %v", name, err)
		}
	}
}

type partialErrorPageWriter struct {
	calls, failAt int
	err           error
}

func (w *partialErrorPageWriter) Write(p []byte) (int, error) {
	w.calls++
	if w.calls == w.failAt {
		return len(p) / 2, w.err
	}
	return len(p), nil
}

type pageWriterErrorContract struct {
	Name  string `json:"name"`
	Write int    `json:"write"`
	Error string `json:"error"`
}

func TestRustLoginPageStopsOnPartialWriteError(t *testing.T) {
	var actual []pageWriterErrorContract
	sentinel := errors.New("partial page write failed")
	for _, name := range []string{"manual", "callback", "success", "failure"} {
		var capture pageWriter
		if err := renderContract(&capture, name, "https://example.test/?x=a+b&y=c"); err != nil {
			t.Fatal(err)
		}
		for index := range capture.writes {
			writer := partialErrorPageWriter{failAt: index + 1, err: sentinel}
			err := renderContract(&writer, name, "https://example.test/?x=a+b&y=c")
			if err != sentinel || !errors.Is(err, sentinel) {
				t.Fatalf("%s write %d error = %v", name, index, err)
			}
			actual = append(actual, pageWriterErrorContract{Name: name, Write: index + 1, Error: err.Error()})
			if writer.calls != index+1 {
				t.Fatalf("%s continued after write %d failure", name, index)
			}
		}
	}
	path := filepath.Join("..", "..", "..", "..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "login_page_writer_errors.json")
	if os.Getenv("PIXIV_LOGIN_PAGE_UPDATE") == "1" {
		data, err := json.MarshalIndent(actual, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, append(data, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []pageWriterErrorContract
	if err = json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(actual, expected) {
		t.Fatalf("Go writer errors changed: actual=%+v", actual)
	}
}
