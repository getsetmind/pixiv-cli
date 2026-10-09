package auth

import (
	"bytes"
	"fmt"
	"github.com/AlecAivazis/survey/v2/core"
	"io"
	"runtime"
	"strings"
	"testing"
)

type surveyContractTerminal struct {
	input          *strings.Reader
	response       bytes.Buffer
	output         bytes.Buffer
	firstReadLimit int
}

func (s *surveyContractTerminal) Fd() uintptr { return ^uintptr(0) }
func (s *surveyContractTerminal) Read(p []byte) (int, error) {
	if s.response.Len() > 0 {
		return s.response.Read(p)
	}
	if s.firstReadLimit > 0 {
		limit := s.firstReadLimit
		s.firstReadLimit = 0
		if len(p) > limit {
			p = p[:limit]
		}
	}
	return s.input.Read(p)
}
func (s *surveyContractTerminal) Write(p []byte) (int, error) {
	if bytes.Contains(p, []byte("\x1b[6n")) {
		s.response.WriteString("\x1b[24;80R")
	}
	return s.output.Write(p)
}

func TestMigrationTerminalSelectContract(t *testing.T) {
	t.Setenv("CLICOLOR_FORCE", "1")
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX; native Windows console remains unverified")
	}
	cases := []struct{ name, input, want, err string }{
		{"first", "\r", "11 Alice", ""},
		{"down", "\x1b[B\r", "22 Bob", ""},
		{"up wraps", "\x1b[A\r", "33 キャロル", ""},
		{"tab", "\t\r", "22 Bob", ""},
		{"vim down", "\x1bj\r", "22 Bob", ""},
		{"vim up", "\x1bk\r", "33 キャロル", ""},
		{"case insensitive filter", "BOB\r", "22 Bob", ""},
		{"unicode filter", "キャ\r", "33 キャロル", ""},
		{"unicode backspace", "キャx\x7f\r", "33 キャロル", ""},
		{"clear word", "BOB\x17\r", "11 Alice", ""},
		{"clear line is control x", "BOB\x18\r", "11 Alice", ""},
		{"control u ignored", "BOB\x15\r", "22 Bob", ""},
		{"end transmission accepts", "\x1b[B\x04", "22 Bob", ""},
		{"interrupt", "\x03", "", "interrupt"},
		{"eof", "", "", "EOF"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			stream := &surveyContractTerminal{input: strings.NewReader(c.input)}
			if strings.HasPrefix(c.name, "vim ") {
				stream.firstReadLimit = 1
			}
			got, err := terminalPromptSelect(stream, stream, io.Discard, "Select account", []string{"11 Alice", "22 Bob", "33 キャロル"})
			if got != c.want {
				t.Fatalf("answer %q, want %q", got, c.want)
			}
			if c.err == "" && err != nil || c.err != "" && (err == nil || err.Error() != c.err) {
				t.Fatalf("error %v, want %q", err, c.err)
			}
			if !strings.Contains(stream.output.String(), "\x1b[?25h") {
				t.Fatal("cursor must be restored")
			}
		})
	}
}
func TestMigrationTerminalConfirmContract(t *testing.T) {
	t.Setenv("CLICOLOR_FORCE", "1")
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX; native Windows console remains unverified")
	}
	cases := []struct {
		name, input string
		want        bool
		err         string
	}{
		{"default false", "\r", false, ""}, {"yes", "YeS\r", true, ""}, {"no", "NO\r", false, ""},
		{"reject whitespace then retry", " yes \rY\r", true, ""}, {"invalid then retry", "maybe\rn\r", false, ""},
		{"left insert", "ys\x1b[De\r", true, ""}, {"backspace", "yesx\x7f\r", true, ""},
		{"home delete", "xyes\x1b[H\x1b[3~\r", true, ""},
		{"end transmission default", "\x04", false, ""}, {"interrupt", "\x03", false, "interrupt"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			stream := &surveyContractTerminal{input: strings.NewReader(c.input)}
			got, err := terminalPromptConfirm(stream, stream, io.Discard, "Remove?", false)
			if got != c.want {
				t.Fatalf("answer %v, want %v", got, c.want)
			}
			if c.err == "" && err != nil || c.err != "" && (err == nil || err.Error() != c.err) {
				t.Fatalf("error %v, want %q", err, c.err)
			}
		})
	}
}

func TestMigrationTerminalSimpleUnicodeFilterContract(t *testing.T) {
	t.Setenv("CLICOLOR_FORCE", "1")
	previousColor := core.DisableColor
	core.DisableColor = false
	defer func() { core.DisableColor = previousColor }()
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX")
	}
	stream := &surveyContractTerminal{input: strings.NewReader("ipek\r")}
	got, err := terminalPromptSelect(stream, stream, io.Discard, "Select account", []string{"İpek", "Bob"})
	if err != nil || got != "İpek" {
		t.Fatalf("answer %q, error %v", got, err)
	}
	expected := "\x1b[1;92m? \x1b[0m\x1b[1;99mSelect account\x1b[0m  \x1b[36m[Use arrows to move, type to filter]\x1b[0m\n\x1b[1;36m> İpek\x1b[0m\n\x1b[39m  Bob\x1b[0m\n"
	if !strings.Contains(stream.output.String(), expected) {
		t.Fatalf("missing frozen initial ANSI fragment: %q", stream.output.String())
	}
}

func TestMigrationTerminalPaginationContract(t *testing.T) {
	t.Setenv("CLICOLOR_FORCE", "1")
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX")
	}
	for _, input := range []string{"\x1b[A\r", strings.Repeat("\x1b[B", 7) + "\r"} {
		stream := &surveyContractTerminal{input: strings.NewReader(input)}
		got, err := terminalPromptSelect(stream, stream, io.Discard, "Select account", []string{"1", "2", "3", "4", "5", "6", "7", "8"})
		if err != nil || got != "8" {
			t.Fatalf("answer %q, error %v", got, err)
		}
		expected := "\x1b[1;36m> 1\x1b[0m\n"
		for _, option := range []string{"2", "3", "4", "5", "6", "7"} {
			expected += "\x1b[39m  " + option + "\x1b[0m\n"
		}
		if !strings.Contains(stream.output.String(), expected) {
			t.Fatalf("initial page must contain first seven options: %q", stream.output.String())
		}
		if !strings.Contains(stream.output.String(), "\x1b[1;36m> 8\x1b[0m\n") {
			t.Fatal("last option must be rendered on navigation")
		}
	}
}
func TestMigrationTerminalEndTransmissionWithoutMatchesPanics(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX")
	}
	stream := &surveyContractTerminal{input: strings.NewReader("ZZZ\x04")}
	defer func() {
		failure := recover()
		if failure == nil || fmt.Sprint(failure) != "runtime error: index out of range [0] with length 0" {
			t.Fatalf("frozen panic: %v", failure)
		}
		if !strings.Contains(stream.output.String(), "\x1b[?25h") {
			t.Fatal("panic still restores cursor")
		}
	}()
	_, _ = terminalPromptSelect(stream, stream, io.Discard, "Select account", []string{"Alice", "Bob"})
}
