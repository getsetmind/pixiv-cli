package auth

import (
	"io"
	"runtime"
	"strings"
	"testing"
)

func TestMigrationTerminalSecretPreservesMaskedInputAndValidation(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI input; native Windows console remains unverified")
	}
	for _, c := range []struct{ name, input, want, err string }{
		{"plain", "synthetic-secret\r", "synthetic-secret", ""},
		{"trim", "  synthetic-secret  \r", "synthetic-secret", ""},
		{"unicode backspace", "合成x\x7f-token\r", "合成-token", ""},
		{"left insert", "synthetc\x1b[Di\r", "synthetic", ""},
		{"home delete", "xsynthetic\x1b[H\x1b[3~\r", "synthetic", ""},
		{"buffered empty then secret", "\rsynthetic-secret\r", "", "EOF"},
		{"empty retry", "\rsynthetic-secret\r", "synthetic-secret", ""},
		{"whitespace retry", " \rsynthetic-secret\r", "synthetic-secret", ""},
		{"control ignored", "synthetic\x17\x18\x15-secret\r", "synthetic-secret", ""},
		{"end transmission", "synthetic-secret\x04", "synthetic-secret", ""},
		{"interrupt", "\x03", "", "interrupt"},
		{"eof", "", "", "EOF"},
	} {
		t.Run(c.name, func(t *testing.T) {
			stream := &surveyContractTerminal{input: strings.NewReader(c.input)}
			if strings.Contains(c.name, "retry") {
				stream.firstReadLimit = strings.Index(c.input, "\r") + 1
			}
			got, err := terminalPromptSecret(stream, stream, io.Discard, "Refresh token")
			if got != c.want {
				t.Fatalf("answer %q want %q", got, c.want)
			}
			if c.err == "" && err != nil || c.err != "" && (err == nil || err.Error() != c.err) {
				t.Fatalf("error %v want %q", err, c.err)
			}
			output := stream.output.String()
			if strings.Contains(output, "synthetic") || strings.Contains(output, "合成") {
				t.Fatal("secret leaked through terminal output")
			}
			if c.want != "" && !strings.Contains(output, "*") {
				t.Fatal("secret input must be masked")
			}
		})
	}
}
