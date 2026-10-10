package installer

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"
)

var updaterWindowsCapture = flag.Bool("migration-capture-updater-windows", false, "capture source-driven frozen Windows replacement evidence")

func TestMigrationUpdaterWindowsReplacementSourceDriven(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("owned Linux source-driven oracle; native Windows runtime remains separately unverified")
	}
	sources := map[string]string{"replace.go": "c22fbe94545a3a4ee2166af4aa1345512bc66c349cde3aa104aae0a0c237c3cc", "replace_windows.go": "3b12dfceef7b011d9aa99a9d6a49cfe3e7bd2fa53a33f41cd65ef6f47d06637b", "replace_recovery.go": "53e1d52b2cdad910f32d6c065d1841dca9dd33ad42351af68e670b4f24ed9309", "replacement_error.go": "e6346582f605366188f60861dad1025133117de02847ccfc844704ef3d39270d"}
	root := t.TempDir()
	for _, dir := range []string{"replace", "mockwindows", "cmd/capture"} {
		if err := os.MkdirAll(filepath.Join(root, dir), 0700); err != nil {
			t.Fatal(err)
		}
	}
	write := func(name string, data []byte) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(root, name), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	write("go.mod", []byte("module witness\n\ngo 1.27.1\n"))
	for name, expected := range sources {
		data, err := os.ReadFile(filepath.Join("../../storage/file/replace", name))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != expected {
			t.Fatalf("frozen source changed: %s", name)
		}
		if name == "replace_windows.go" {
			data = bytes.Replace(data, []byte("//go:build windows\n\n"), nil, 1)
			data = bytes.Replace(data, []byte("\"golang.org/x/sys/windows\""), []byte("windows \"witness/mockwindows\""), 1)
		}
		write("replace/"+strings.ReplaceAll(name, "_windows.go", "_adapted.go"), data)
	}
	write("replace/sync_stub.go", []byte("package replace\nfunc syncParentDirectory(string) error { return nil }\n"))
	write("mockwindows/windows.go", []byte(updaterWindowsMockSource))
	write("cmd/capture/main.go", []byte(updaterWindowsWitnessSource))
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, filepath.Join(os.Getenv("GOROOT"), "bin/go"), "run", "./cmd/capture")
	cmd.Dir = root
	cmd.Env = append(os.Environ(), "GOWORK=off", "GOPROXY=off", "GOSUMDB=off", "CGO_ENABLED=0")
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	data, err := cmd.Output()
	if err != nil {
		t.Fatalf("owned source-driven witness failed: %v\n%s", err, stderr.String())
	}
	var cases any
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	rows, ok := cases.([]any)
	if !ok || len(rows) != 26 {
		t.Fatal("owned witness did not execute all 26 source-driven cases")
	}
	seen := map[string]bool{}
	for _, raw := range rows {
		row := raw.(map[string]any)
		input := row["input"].(map[string]any)
		name := input["name"].(string)
		if seen[name] {
			t.Fatalf("duplicate owned witness case: %s", name)
		}
		seen[name] = true
		t.Run(name, func(t *testing.T) {
			for _, rawTrace := range row["trace"].([]any) {
				trace := rawTrace.(map[string]any)
				if trace["op"] == "replace" {
					if trace["proc"] != "ReplaceFileW" || trace["flags"] != float64(0) || trace["reserved_a"] != float64(0) || trace["reserved_b"] != float64(0) {
						t.Fatal("replacement ABI must keep flags/reserved fields zero")
					}
					for _, field := range []string{"source", "target", "backup"} {
						if trace[field] == "" {
							t.Fatal("replacement pointer argument is missing")
						}
					}
				}
				if trace["op"] == "move" && trace["flags"] != float64(0) {
					t.Fatal("creation and restoration must not overwrite existing targets")
				}
			}
			method := input["method"]
			if method != "private" && row["result"] != nil {
				t.Fatal("error-only public APIs must not invent a committed result")
			}
			if input["replace_code"] == float64(1177) && row["is_1177"] != true {
				t.Fatal("1177 identity must survive recovery and wrapping")
			}
			if input["restore_error"] == true || input["recovery_conflict"] == true {
				if row["preserve_replacement_source"] != true {
					t.Fatal("failed recovery must retain source material")
				}
			}
			if input["source_nul"] == true || input["target_nul"] == true || input["backup_nul"] == true {
				if row["is_invalid_argument"] != true {
					t.Fatal("NUL conversion must stop before mutation")
				}
			}
			files := map[string]any{}
			for _, f := range row["files"].([]any) {
				file := f.(map[string]any)
				files[file["path"].(string)] = file["bytes"]
			}
			switch name {
			case "retained_success", "retained_success_ignores_stale_last_error":
				if !reflect.DeepEqual(files, map[string]any{"pixiv.exe": "candidate", "pixiv.exe.old": "old"}) || len(row["error_messages"].([]any)) != 0 {
					t.Fatal("committed retained backup must survive for later cleanup")
				}
			case "disposable_success", "private_success", "retained_empty_backup_uses_disposable", "retained_missing_target_move", "private_missing_target_move":
				if !reflect.DeepEqual(files, map[string]any{"pixiv.exe": "candidate"}) || len(row["error_messages"].([]any)) != 0 {
					t.Fatal("successful disposable/create must consume source without retaining backup")
				}
			case "retained_error1177_restored", "retained_wrapped1177_restored", "disposable_error1177_restored":
				if !reflect.DeepEqual(files, map[string]any{"candidate": "candidate", "pixiv.exe": "old"}) || row["preserve_replacement_source"] != false {
					t.Fatal("successful recovery must restore old target and keep new source unchanged")
				}
			case "private_committed_backup_cleanup_fault":
				if !reflect.DeepEqual(row["result"], map[string]any{"committed": true, "preserve_source": false}) || files["pixiv.exe"] != "candidate" || files["candidate.recovery/old"] != "old" || row["preserve_replacement_source"] != false {
					t.Fatal("backup cleanup failure must retain committed state, not claim rollback")
				}
			}
		})
	}
	fixture := map[string]any{"reference": "4b4426487ef18bed276706daec385e0d0a6979f9", "evidence": "exact frozen Windows replacement/recovery function bodies compiled on Linux with an owned mock windows import; not native Windows compilation, ABI or runtime", "source_sha256": sources, "adaptations": []string{"remove Windows build tag only in owned temporary copy", "replace golang.org/x/sys/windows import with owned mock codec/API package", "add uncalled SyncParentDirectory dependency stub; frozen function bodies unchanged"}, "normalization": []string{"owned temporary root becomes ${ROOT}", "native pointer fields resolve through owned codec map to their strings; scalar ABI arguments remain exact"}, "limitations": []string{"real Windows system error strings, DLL loader, ABI execution, privilege/ACL/race and OS behavior remain unverified", "Lstat/Remove execute owned Linux filesystem and are not individually instrumented; their Linux diagnostics are not native Windows diagnostics", "mock Windows API effects are controlled owned filesystem operations; no live registry, browser or user environment"}, "cases": cases}
	path := "../../../crates/pixiv-cli/tests/fixtures/updater-windows-replacement.json"
	if t.Failed() {
		t.Fatal("failed source-driven observations cannot be captured")
	}
	if *updaterWindowsCapture {
		body, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected any
	if err := json.Unmarshal(body, &expected); err != nil {
		t.Fatal(err)
	}
	actualBody, err := json.Marshal(fixture)
	if err != nil {
		t.Fatal(err)
	}
	var actual any
	if err := json.Unmarshal(actualBody, &actual); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(actual, expected) {
		t.Fatalf("source-driven Windows replacement observations differ; capture is explicit only\n%s", actualBody)
	}
}

const updaterWindowsMockSource = `package mockwindows
import("errors";"fmt";"os";"syscall";"unicode/utf16";"unsafe")
const ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 syscall.Errno = 1177
type Config struct { Root,Source,Target,Backup string; ReplaceCode int; WrappedCode,FindError,MoveError,RestoreError,RecoveryConflict,BackupDirectory,StaleLastError bool }
var C Config
var Trace []map[string]any
var pointers map[uintptr]string
var holds [][]uint16
func Reset(c Config){C=c;Trace=[]map[string]any{};pointers=map[uintptr]string{};holds=nil}
func record(m map[string]any){Trace=append(Trace,m)}
func UTF16PtrFromString(s string)(*uint16,error){ record(map[string]any{"op":"utf16","value":s});for _,r:=range s{if r==0{return nil,syscall.EINVAL}};v:=append(utf16.Encode([]rune(s)),0);holds=append(holds,v);p:=&v[0];pointers[uintptr(unsafe.Pointer(p))]=s;return p,nil }
type DLL struct{Name string};type Proc struct{Name string}
func NewLazySystemDLL(s string)*DLL{return &DLL{Name:s}}
func(d *DLL)NewProc(s string)*Proc{return &Proc{Name:s}}
func(p *Proc)Find()error{record(map[string]any{"op":"find","proc":p.Name});if C.FindError{return errors.New("find denied")};return nil}
func(p *Proc)Call(args ...uintptr)(uintptr,uintptr,error){
 if len(args)!=6{panic("unexpected ABI count")};to,from,backup:=pointers[args[0]],pointers[args[1]],pointers[args[2]]
 record(map[string]any{"op":"replace","proc":p.Name,"target":to,"source":from,"backup":backup,"flags":args[3],"reserved_a":args[4],"reserved_b":args[5]})
 if C.ReplaceCode!=0 { err:=error(syscall.Errno(C.ReplaceCode));if C.WrappedCode{err=fmt.Errorf("mock wrapped: %w",err)};if C.ReplaceCode==1177{if e:=os.Rename(to,backup);e!=nil{panic(e)};if C.RecoveryConflict{if e:=os.WriteFile(to,[]byte("racing-target"),0600);e!=nil{panic(e)}}};return 0,0,err }
 if _,e:=os.Lstat(from);e!=nil{return 0,0,e}
 if e:=os.Rename(to,backup);e!=nil{return 0,0,e};if e:=os.Rename(from,to);e!=nil{panic(e)}
 if C.BackupDirectory {old,e:=os.ReadFile(backup);if e!=nil{panic(e)};if e=os.Remove(backup);e!=nil{panic(e)};if e=os.Mkdir(backup,0700);e!=nil{panic(e)};if e=os.WriteFile(backup+"/old",old,0600);e!=nil{panic(e)}}
 if C.StaleLastError{return 1,0,errors.New("stale last error")};return 1,0,nil
}
func MoveFileEx(from,to *uint16,flags uint32)error{
 src,dst:=pointers[uintptr(unsafe.Pointer(from))],pointers[uintptr(unsafe.Pointer(to))];record(map[string]any{"op":"move","source":src,"target":dst,"flags":flags})
 if (src==C.Backup && C.RestoreError)||(src!=C.Backup&&C.MoveError){return errors.New("move denied")}
 if _,e:=os.Lstat(dst);e==nil{return os.ErrExist}else if !os.IsNotExist(e){return e};return os.Rename(src,dst)
}
`
const updaterWindowsWitnessSource = `package main
import("encoding/json";"errors";"fmt";"os";"path/filepath";"strings";"syscall";"witness/replace";windows "witness/mockwindows")
type Spec struct{Name string ` + "`" + `json:"name"` + "`" + `;Method string ` + "`" + `json:"method"` + "`" + `;MissingTarget bool ` + "`" + `json:"missing_target,omitempty"` + "`" + `;MissingSource bool ` + "`" + `json:"missing_source,omitempty"` + "`" + `;EmptyBackup bool ` + "`" + `json:"empty_backup,omitempty"` + "`" + `;SourceNul bool ` + "`" + `json:"source_nul,omitempty"` + "`" + `;TargetNul bool ` + "`" + `json:"target_nul,omitempty"` + "`" + `;BackupNul bool ` + "`" + `json:"backup_nul,omitempty"` + "`" + `;Unicode bool ` + "`" + `json:"unicode,omitempty"` + "`" + `;BlockedParent bool ` + "`" + `json:"blocked_parent,omitempty"` + "`" + `;ReplaceCode int ` + "`" + `json:"replace_code,omitempty"` + "`" + `;WrappedCode bool ` + "`" + `json:"wrapped_code,omitempty"` + "`" + `;FindError bool ` + "`" + `json:"find_error,omitempty"` + "`" + `;MoveError bool ` + "`" + `json:"move_error,omitempty"` + "`" + `;RestoreError bool ` + "`" + `json:"restore_error,omitempty"` + "`" + `;RecoveryConflict bool ` + "`" + `json:"recovery_conflict,omitempty"` + "`" + `;BackupDirectory bool ` + "`" + `json:"backup_directory,omitempty"` + "`" + `;StaleLastError bool ` + "`" + `json:"stale_last_error,omitempty"` + "`" + `}
func main(){
 specs:=[]Spec{
 {Name:"retained_success",Method:"retained"},{Name:"retained_success_ignores_stale_last_error",Method:"retained",StaleLastError:true},{Name:"retained_missing_target_move",Method:"retained",MissingTarget:true},{Name:"retained_empty_backup_uses_disposable",Method:"retained",EmptyBackup:true},
 {Name:"disposable_success",Method:"disposable"},{Name:"private_success",Method:"private"},{Name:"private_missing_target_move",Method:"private",MissingTarget:true},{Name:"retained_find_failure",Method:"retained",FindError:true},
 {Name:"retained_error1175_unchanged",Method:"retained",ReplaceCode:1175},{Name:"retained_error1176_unchanged",Method:"retained",ReplaceCode:1176},{Name:"retained_other_error_unchanged",Method:"retained",ReplaceCode:5},
 {Name:"retained_error1177_restored",Method:"retained",ReplaceCode:1177},{Name:"retained_wrapped1177_restored",Method:"retained",ReplaceCode:1177,WrappedCode:true},{Name:"retained_error1177_restore_failure",Method:"retained",ReplaceCode:1177,RestoreError:true},{Name:"retained_error1177_restore_conflict",Method:"retained",ReplaceCode:1177,RecoveryConflict:true},
 {Name:"disposable_error1177_restored",Method:"disposable",ReplaceCode:1177},{Name:"private_error1177_restore_failure",Method:"private",ReplaceCode:1177,RestoreError:true},{Name:"private_committed_backup_cleanup_fault",Method:"private",BackupDirectory:true},{Name:"disposable_committed_backup_cleanup_fault",Method:"disposable",BackupDirectory:true},
 {Name:"retained_create_move_failure",Method:"retained",MissingTarget:true,MoveError:true},{Name:"retained_missing_source",Method:"retained",MissingSource:true},{Name:"retained_source_nul",Method:"retained",SourceNul:true},{Name:"retained_target_nul",Method:"retained",TargetNul:true},{Name:"retained_backup_nul_after_find",Method:"retained",BackupNul:true},{Name:"retained_unicode_utf16",Method:"retained",Unicode:true},{Name:"retained_lstat_not_directory",Method:"retained",BlockedParent:true},
 }
 out:=[]map[string]any{}
 for _,s:=range specs{
  root,e:=os.MkdirTemp("","updater-windows-witness-");if e!=nil{panic(e)}
  src,tgt,bak:=filepath.Join(root,"candidate"),filepath.Join(root,"pixiv.exe"),filepath.Join(root,"pixiv.exe.old")
  if s.Unicode{src=filepath.Join(root,"新😀");tgt=filepath.Join(root,"日本語😀.exe");bak=filepath.Join(root,"旧😀.old")}
  if !s.MissingSource{if e=os.WriteFile(src,[]byte("candidate"),0600);e!=nil{panic(e)}}
  if !s.MissingTarget{if e=os.WriteFile(tgt,[]byte("old"),0600);e!=nil{panic(e)}}
  if s.BlockedParent{if e=os.WriteFile(filepath.Join(root,"blocked"),[]byte("block"),0600);e!=nil{panic(e)};tgt=filepath.Join(root,"blocked","pixiv.exe")}
  if s.SourceNul{src+="\x00x"};if s.TargetNul{tgt+="\x00x"};if s.BackupNul{bak+="\x00x"};if s.EmptyBackup{bak=""}
  effectiveBackup:=bak;if s.Method!="retained"||bak==""{effectiveBackup=src+".recovery"}
  windows.Reset(windows.Config{Root:root,Source:src,Target:tgt,Backup:effectiveBackup,ReplaceCode:s.ReplaceCode,WrappedCode:s.WrappedCode,FindError:s.FindError,MoveError:s.MoveError,RestoreError:s.RestoreError,RecoveryConflict:s.RecoveryConflict,BackupDirectory:s.BackupDirectory,StaleLastError:s.StaleLastError})
  var err error;var result any
  switch s.Method{case "retained":err=replace.ReplaceFileWithBackup(src,tgt,bak);case "disposable":err=replace.ReplaceFile(src,tgt);case "private":var r replace.Result;r,err=replace.ReplacePrivateFile(src,tgt);result=map[string]any{"committed":r.Committed,"preserve_source":r.PreserveSource}}
  files:=[]map[string]any{};if e=filepath.WalkDir(root,func(p string,d os.DirEntry,walkErr error)error{if walkErr!=nil{return walkErr};if p==root{return nil};rel,_:=filepath.Rel(root,p);v:=map[string]any{"path":filepath.ToSlash(rel)};info,e:=d.Info();if e!=nil{return e};v["mode"]=fmt.Sprintf("%04o",info.Mode().Perm());if d.IsDir(){v["kind"]="directory"}else{v["kind"]="file";body,e:=os.ReadFile(p);if e!=nil{return e};v["bytes"]=string(body)};files=append(files,v);return nil});e!=nil{panic(e)}
  messages:=[]string{};if err!=nil{messages=append(messages,err.Error())}
  observation:=map[string]any{"input":s,"paths":map[string]string{"source":src,"target":tgt,"backup":bak},"trace":windows.Trace,"result":result,"error_messages":messages,"preserve_replacement_source":replace.MustPreserveReplacementSource(err),"is_1177":errors.Is(err,syscall.Errno(1177)),"is_invalid_argument":errors.Is(err,syscall.EINVAL),"is_not_exist":errors.Is(err,os.ErrNotExist),"is_already_exist":errors.Is(err,os.ErrExist),"files":files}
  body,e:=json.Marshal(observation);if e!=nil{panic(e)};body=[]byte(strings.ReplaceAll(string(body),root,"${ROOT}"));var normalized map[string]any;if e=json.Unmarshal(body,&normalized);e!=nil{panic(e)};out=append(out,normalized);if e=os.RemoveAll(root);e!=nil{panic(e)}
 }
 if e:=json.NewEncoder(os.Stdout).Encode(out);e!=nil{panic(e)}
}
`
