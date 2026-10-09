//go:build windows

package auth_test

import (
	"encoding/json"
	"os"
	"testing"

	"golang.org/x/sys/windows"
)

func TestMigrationNativeBrowserWindowsNULPanicsBeforeNativeCall(t *testing.T) {
	body, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/native_browser.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		WindowsNUL struct{ Message string } `json:"windows_nul"`
	}
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	defer func() {
		if got := recover(); got != fixture.WindowsNUL.Message {
			t.Fatalf("UTF16 NUL panic = %v, want %q", got, fixture.WindowsNUL.Message)
		}
	}()
	// OpenURL uses this conversion before ShellExecuteW; calling only the conversion never launches a browser.
	windows.StringToUTF16Ptr("synthetic\x00url")
}
