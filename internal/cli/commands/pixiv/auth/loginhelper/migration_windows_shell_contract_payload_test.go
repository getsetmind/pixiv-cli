//go:build windows_shell_contract_payload

package loginhelper

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"syscall"
	"testing"
	"time"
	"unicode/utf16"
	"unsafe"

	windows "MOCK_IMPORT"
)

func resetShell(t *testing.T) *[]string {
	t.Helper()
	trace := []string{}
	windows.Convert = func(value string) { trace = append(trace, "utf16:"+value) }
	windows.Invoke = func(unsafe.Pointer) (uintptr, uintptr, error) {
		trace = append(trace, "call")
		return 1, 0, windows.ERROR_SUCCESS
	}
	return &trace
}

func readUTF16(pointer *uint16) string {
	var units []uint16
	for offset := uintptr(0); ; offset += 2 {
		unit := *(*uint16)(unsafe.Add(unsafe.Pointer(pointer), offset))
		if unit == 0 {
			return string(utf16.Decode(units))
		}
		units = append(units, unit)
	}
}

func TestContextPrecedesBothUTF16Conversions(t *testing.T) {
	for _, deadline := range []bool{false, true} {
		t.Run(fmt.Sprint(deadline), func(t *testing.T) {
			trace := resetShell(t)
			ctx, cancel := context.WithCancel(context.Background())
			wanted := context.Canceled
			if deadline {
				ctx, cancel = context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
				wanted = context.DeadlineExceeded
			} else {
				cancel()
			}
			defer cancel()
			if err := windowsShellOpenClass(ctx, "class\x00", "url\x00"); err != wanted {
				t.Fatalf("context error = %v, want %v", err, wanted)
			}
			if len(*trace) != 0 {
				t.Fatalf("native boundary trace = %q", *trace)
			}
		})
	}
}

func TestClassNULPrecedesURLNULAndNeitherInvokesShell(t *testing.T) {
	for _, test := range []struct {
		class string
		url   string
		trace []string
	}{
		{"class\x00", "url\x00", []string{"utf16:class\x00"}},
		{"class", "url\x00", []string{"utf16:class", "utf16:url\x00"}},
	} {
		trace := resetShell(t)
		err := windowsShellOpenClass(context.Background(), test.class, test.url)
		if !errors.Is(err, syscall.EINVAL) {
			t.Fatalf("NUL error = %v, want EINVAL", err)
		}
		if !reflect.DeepEqual(*trace, test.trace) {
			t.Fatalf("trace = %q, want %q", *trace, test.trace)
		}
	}
}

func TestCompleteShellExecuteExWABIAndClassSelection(t *testing.T) {
	if windows.DLLName != "shell32.dll" || windows.ProcName != "ShellExecuteExW" {
		t.Fatalf("native binding = %q/%q", windows.DLLName, windows.ProcName)
	}
	fields := []string{"CBSize", "FMask", "HWND", "Verb", "File", "Parameters", "Directory", "Show", "HInstApp", "IDList", "Class", "HKeyClass", "HotKey", "Icon", "Process"}
	offsets := []uintptr{0, 4, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104}
	size := uintptr(112)
	if unsafe.Sizeof(uintptr(0)) == 4 {
		offsets = []uintptr{0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56}
		size = 60
	}
	abi := reflect.TypeOf(shellExecuteInfo{})
	if abi.NumField() != len(fields) || abi.Size() != size {
		t.Fatalf("ABI fields/size = %d/%d, want %d/%d", abi.NumField(), abi.Size(), len(fields), size)
	}
	for index, name := range fields {
		if field := abi.Field(index); field.Name != name || field.Offset != offsets[index] {
			t.Fatalf("ABI field %d = %s/%d, want %s/%d", index, field.Name, field.Offset, name, offsets[index])
		}
	}
	for _, test := range []struct{ class, url string }{
		{"Previous.Handler.🐈", "  pixiv://unlisted?x=a%20b&text=絵🐈#fragment  "},
		{"", ""},
	} {
		trace := resetShell(t)
		windows.Invoke = func(pointer unsafe.Pointer) (uintptr, uintptr, error) {
			*trace = append(*trace, "call")
			info := (*shellExecuteInfo)(pointer)
			if info.CBSize != uint32(size) || info.FMask != 1 || info.Show != 1 {
				t.Fatalf("size/mask/show = %d/%d/%d", info.CBSize, info.FMask, info.Show)
			}
			if info.Class == nil || info.File == nil || readUTF16(info.Class) != test.class || readUTF16(info.File) != test.url {
				t.Fatalf("class/raw URL UTF16 arguments differ")
			}
			if info.HWND != 0 || info.Verb != nil || info.Parameters != nil || info.Directory != nil || info.HInstApp != 0 || info.IDList != 0 || info.HKeyClass != 0 || info.HotKey != 0 || info.Icon != 0 || info.Process != 0 {
				t.Fatalf("unrequested ShellExecuteExW fields are nonzero: %+v", info)
			}
			return 1, 0, windows.ERROR_SUCCESS
		}
		if err := windowsShellOpenClass(context.Background(), test.class, test.url); err != nil {
			t.Fatal(err)
		}
		wanted := []string{"utf16:" + test.class, "utf16:" + test.url, "call"}
		if !reflect.DeepEqual(*trace, wanted) {
			t.Fatalf("trace = %q, want %q", *trace, wanted)
		}
	}
}

func TestBooleanResultHasPriorityOverLastError(t *testing.T) {
	wrappedSuccess := fmt.Errorf("wrapped: %w", windows.ERROR_SUCCESS)
	wrappedFailure := fmt.Errorf("wrapped: %w", syscall.Errno(5))
	for _, test := range []struct {
		name    string
		result  uintptr
		last    error
		wanted  error
		message string
	}{
		{"false-nil", 0, nil, nil, "ShellExecuteExW failed"},
		{"false-success", 0, windows.ERROR_SUCCESS, nil, "ShellExecuteExW failed"},
		{"false-wrapped-success", 0, wrappedSuccess, nil, "ShellExecuteExW failed"},
		{"false-last-error", 0, syscall.Errno(5), syscall.Errno(5), ""},
		{"false-wrapped-last-error", 0, wrappedFailure, wrappedFailure, ""},
		{"true-nil", 1, nil, nil, ""},
		{"true-success", 1, windows.ERROR_SUCCESS, nil, ""},
		{"true-stale-error", 1, syscall.Errno(5), nil, ""},
		{"noncanonical-true", 2, syscall.Errno(5), nil, ""},
		{"shell-execute-w-threshold-is-not-used", 32, syscall.Errno(5), nil, ""},
	} {
		t.Run(test.name, func(t *testing.T) {
			resetShell(t)
			windows.Invoke = func(unsafe.Pointer) (uintptr, uintptr, error) { return test.result, 9, test.last }
			err := windowsShellOpenClass(context.Background(), "Previous.Class", "pixiv://unlisted")
			if test.message != "" {
				if err == nil || err.Error() != test.message {
					t.Fatalf("error = %v, want %q", err, test.message)
				}
			} else if err != test.wanted {
				t.Fatalf("error = %v, want %v", err, test.wanted)
			}
		})
	}
}

func TestCancellationAfterPreflightDoesNotReplaceShellResult(t *testing.T) {
	trace := resetShell(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	windows.Convert = func(value string) {
		*trace = append(*trace, "utf16:"+value)
		cancel()
	}
	if err := windowsShellOpenClass(ctx, "class", "url"); err != nil {
		t.Fatalf("post-preflight cancellation changed native result: %v", err)
	}
	if wanted := []string{"utf16:class", "utf16:url", "call"}; !reflect.DeepEqual(*trace, wanted) {
		t.Fatalf("trace = %q, want %q", *trace, wanted)
	}
}
