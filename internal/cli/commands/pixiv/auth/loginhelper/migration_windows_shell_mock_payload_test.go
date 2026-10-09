//go:build windows_shell_contract_payload

package mockwindows

import (
	"strings"
	"syscall"
	"unicode/utf16"
	"unsafe"
)

const ERROR_SUCCESS = syscall.Errno(0)

var Convert func(string)
var Invoke func(unsafe.Pointer) (uintptr, uintptr, error)
var DLLName string
var ProcName string

type DLL struct{}
type Proc struct{}

func NewLazySystemDLL(name string) *DLL {
	DLLName = name
	return &DLL{}
}

func (*DLL) NewProc(name string) *Proc {
	ProcName = name
	return &Proc{}
}

func UTF16PtrFromString(value string) (*uint16, error) {
	Convert(value)
	if strings.IndexByte(value, 0) >= 0 {
		return nil, syscall.EINVAL
	}
	body := append(utf16.Encode([]rune(value)), 0)
	return &body[0], nil
}

//go:uintptrescapes
func (*Proc) Call(arguments ...uintptr) (uintptr, uintptr, error) {
	if len(arguments) != 1 || arguments[0] == 0 {
		panic("ShellExecuteExW requires exactly one non-null argument")
	}
	return Invoke(*(*unsafe.Pointer)(unsafe.Pointer(&arguments[0])))
}
