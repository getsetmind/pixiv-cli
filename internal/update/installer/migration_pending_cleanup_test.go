package installer

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

var updatePendingCleanup = flag.Bool("migration-update-pending-cleanup", false, "capture frozen pending Windows update cleanup contracts")

type pendingCleanupEntry struct {
	Path   string `json:"path"`
	Kind   string `json:"kind"`
	Target string `json:"target,omitempty"`
}

type pendingCleanupCase struct {
	Name            string                `json:"name"`
	GOOS            string                `json:"goos"`
	Executable      string                `json:"executable"`
	Entries         []pendingCleanupEntry `json:"entries"`
	ExecutableError string                `json:"executable_error,omitempty"`
	RemoveError     string                `json:"remove_error,omitempty"`
	Trace           []string              `json:"trace"`
	ErrorStage      string                `json:"error_stage"`
	ErrorPrefix     string                `json:"error_prefix"`
	ErrorKind       string                `json:"error_kind"`
	Surviving       []string              `json:"surviving"`
}

type pendingCleanupFixture struct {
	Reference   string               `json:"reference"`
	SourceSHA   string               `json:"source_sha256"`
	ResolverSHA string               `json:"resolver_sha256"`
	Evidence    string               `json:"evidence"`
	Cases       []pendingCleanupCase `json:"cases"`
}

func TestMigrationPendingWindowsCleanupFrozenContracts(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("frozen host filesystem fixtures require a Unix runner; native Windows remains separately unverified")
	}
	const sourceSHA = "bfc9fd4ef560745c0cdbe5219320bcf76f82bd2f79f4adee6cb94923763c1d45"
	const resolverSHA = "c299a189ccd1dc7de2bc820b49566a5be244dfa6bc08add4fea0e5af07eb4d2c"
	body, err := os.ReadFile("pending_cleanup.go")
	if err != nil {
		t.Fatal(err)
	}
	if fmt.Sprintf("%x", sha256.Sum256(body)) != sourceSHA {
		t.Fatal("pending cleanup source changed from pinned Go reference")
	}
	body, err = os.ReadFile("release_installer.go")
	if err != nil {
		t.Fatal(err)
	}
	start := bytes.Index(body, []byte("func resolveReleaseExecutablePath("))
	if start < 0 {
		t.Fatal("executable resolver missing")
	}
	end := bytes.Index(body[start:], []byte("\n}\n"))
	if end < 0 || fmt.Sprintf("%x", sha256.Sum256(body[start:start+end+3])) != resolverSHA {
		t.Fatal("executable resolver changed from pinned Go reference")
	}
	file := pendingCleanupEntry{Path: "bin/pixiv.exe", Kind: "file"}
	old := pendingCleanupEntry{Path: "bin/pixiv.exe.old", Kind: "file"}
	inputs := []pendingCleanupCase{
		{Name: "linux_no_operations", GOOS: "linux", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}, ExecutableError: "executable denied", RemoveError: "remove denied"},
		{Name: "darwin_no_operations", GOOS: "darwin", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}},
		{Name: "executable_failure", GOOS: "windows", Executable: "", Entries: []pendingCleanupEntry{}, ExecutableError: "executable denied"},
		{Name: "missing_working_directory", GOOS: "windows", Executable: "pixiv.exe", Entries: []pendingCleanupEntry{}},
		{Name: "existing_executable", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}},
		{Name: "missing_executable", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{old}},
		{Name: "missing_old", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file}},
		{Name: "missing_parent", GOOS: "windows", Executable: "${ROOT}/absent/pixiv.exe", Entries: []pendingCleanupEntry{}},
		{Name: "relative_executable", GOOS: "windows", Executable: "${RELATIVE_ROOT}/bin/../bin/./pixiv.exe", Entries: []pendingCleanupEntry{file, old}},
		{Name: "quoted_executable", GOOS: "windows", Executable: "${ROOT}/bin/pixiv\"\n.exe", Entries: []pendingCleanupEntry{{Path: "bin/pixiv\"\n.exe.old", Kind: "file"}}, RemoveError: "remove denied"},
		{Name: "empty_old_directory", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, {Path: "bin/pixiv.exe.old", Kind: "directory"}}},
		{Name: "nonempty_old_directory", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, {Path: "bin/pixiv.exe.old", Kind: "directory"}, {Path: "bin/pixiv.exe.old/keep", Kind: "file"}}},
		{Name: "old_symlink", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, {Path: "bin/keep", Kind: "file"}, {Path: "bin/pixiv.exe.old", Kind: "symlink", Target: "keep"}}},
		{Name: "dangling_old_symlink", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, {Path: "bin/pixiv.exe.old", Kind: "symlink", Target: "absent"}}},
		{Name: "final_executable_symlink", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{file, old, {Path: "bin/alias.exe", Kind: "symlink", Target: "pixiv.exe"}, {Path: "bin/alias.exe.old", Kind: "file"}}},
		{Name: "relative_symlink_chain", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{file, old, {Path: "links/first", Kind: "symlink", Target: "../bin/pixiv.exe"}, {Path: "bin/alias.exe", Kind: "symlink", Target: "../links/first"}}},
		{Name: "intermediate_directory_symlink", GOOS: "windows", Executable: "${ROOT}/alias/pixiv.exe", Entries: []pendingCleanupEntry{file, old, {Path: "alias", Kind: "symlink", Target: "bin"}}},
		{Name: "clean_before_symlink_resolution", GOOS: "windows", Executable: "${ROOT}/alias/../bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old, {Path: "alias", Kind: "symlink", Target: "absent"}}},
		{Name: "executable_symlink_target_trailing_separator", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{file, old, {Path: "bin/alias.exe", Kind: "symlink", Target: "pixiv.exe/"}}},
		{Name: "executable_symlink_target_trailing_dot", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{file, old, {Path: "bin/alias.exe", Kind: "symlink", Target: "pixiv.exe/."}}},
		{Name: "dangling_executable_symlink", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{{Path: "bin/alias.exe", Kind: "symlink", Target: "absent"}, {Path: "bin/alias.exe.old", Kind: "file"}}},
		{Name: "cyclic_executable_symlink", GOOS: "windows", Executable: "${ROOT}/bin/alias.exe", Entries: []pendingCleanupEntry{{Path: "bin/alias.exe", Kind: "symlink", Target: "alias.exe"}, {Path: "bin/alias.exe.old", Kind: "file"}}},
		{Name: "lstat_not_directory_is_ignored", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe/child", Entries: []pendingCleanupEntry{file}},
		{Name: "remove_failure", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}, RemoveError: "remove denied"},
		{Name: "remove_not_exist", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}, RemoveError: "not_exist"},
		{Name: "remove_wrapped_not_exist", GOOS: "windows", Executable: "${ROOT}/bin/pixiv.exe", Entries: []pendingCleanupEntry{file, old}, RemoveError: "wrapped_not_exist"},
	}
	long := pendingCleanupCase{Name: "long_executable_symlink_chain", GOOS: "windows", Executable: "${ROOT}/bin/link0", Entries: []pendingCleanupEntry{file, old}}
	for i := 0; i < 64; i++ {
		target := fmt.Sprintf("link%d", i+1)
		if i == 63 {
			target = "pixiv.exe"
		}
		long.Entries = append(long.Entries, pendingCleanupEntry{Path: fmt.Sprintf("bin/link%d", i), Kind: "symlink", Target: target})
	}
	inputs = append(inputs, long)
	fixture := pendingCleanupFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", SourceSHA: sourceSHA, ResolverSHA: resolverSHA, Evidence: "Pinned production cleanup helper and executable resolver, genuine executable/remove ports, real isolated Unix temporary files. Native Windows filesystem calls and OS-specific source error text are unverified.", Cases: []pendingCleanupCase{}}
	for _, input := range inputs {
		t.Run(input.Name, func(t *testing.T) {
			root := t.TempDir()
			cwd, err := os.Getwd()
			if err != nil {
				t.Fatal(err)
			}
			relative, err := filepath.Rel(cwd, root)
			if err != nil {
				t.Fatal(err)
			}
			executable := strings.NewReplacer("${ROOT}", root, "${RELATIVE_ROOT}", relative).Replace(input.Executable)
			normalize := func(value string) string { return strings.ReplaceAll(value, root, "${ROOT}") }
			for _, entry := range input.Entries {
				path := filepath.Join(root, entry.Path)
				if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
					t.Fatal(err)
				}
				switch entry.Kind {
				case "file":
					err = os.WriteFile(path, []byte("keep"), 0600)
				case "directory":
					err = os.MkdirAll(path, 0700)
				case "symlink":
					err = os.Symlink(entry.Target, path)
				default:
					t.Fatal("unknown entry kind")
				}
				if err != nil {
					t.Fatal(err)
				}
			}
			if input.Name == "missing_working_directory" {
				t.Chdir(root)
				if err := os.Remove(root); err != nil {
					t.Fatal(err)
				}
			}
			input.Trace = []string{}
			err = cleanupPendingWindowsUpdate(input.GOOS, func() (string, error) {
				input.Trace = append(input.Trace, "executable")
				if input.ExecutableError != "" {
					return "", errors.New(input.ExecutableError)
				}
				return executable, nil
			}, func(path string) error {
				input.Trace = append(input.Trace, "remove:"+normalize(path))
				switch input.RemoveError {
				case "not_exist":
					return os.ErrNotExist
				case "wrapped_not_exist":
					return &os.PathError{Op: "remove", Path: path, Err: os.ErrNotExist}
				case "":
					return os.Remove(path)
				default:
					return errors.New(input.RemoveError)
				}
			})
			input.ErrorStage, input.ErrorPrefix, input.ErrorKind = "", "", ""
			if err != nil {
				message := normalize(err.Error())
				switch {
				case strings.HasPrefix(message, "locate executable"):
					input.ErrorStage = "executable"
				case strings.HasPrefix(message, "resolve current"):
					input.ErrorStage = "absolute"
				case strings.HasPrefix(message, "resolve executable symlink target"):
					input.ErrorStage = "symlink_absolute"
				case strings.HasPrefix(message, "resolve executable symlink"):
					input.ErrorStage = "symlink"
				case strings.HasPrefix(message, "remove pending"):
					input.ErrorStage = "remove"
				default:
					t.Fatalf("unrecognized error %v", err)
				}
				if input.ExecutableError != "" || input.RemoveError != "" {
					input.ErrorPrefix = message
					input.ErrorKind = "other"
				} else {
					cut := strings.Index(message, ": ")
					input.ErrorPrefix = message[:cut+2]
					switch {
					case errors.Is(err, os.ErrNotExist):
						input.ErrorKind = "not_exist"
					case strings.Contains(message, "too many links"):
						input.ErrorKind = "too_many_links"
					case strings.Contains(message, "directory not empty"):
						input.ErrorKind = "directory_not_empty"
					case strings.Contains(message, "not a directory"):
						input.ErrorKind = "not_directory"
					default:
						input.ErrorKind = "other"
					}
				}
			}
			input.Surviving = []string{}
			for _, entry := range input.Entries {
				if _, err := os.Lstat(filepath.Join(root, entry.Path)); err == nil {
					input.Surviving = append(input.Surviving, entry.Path)
				} else if !os.IsNotExist(err) {
					t.Fatal(err)
				}
			}
			fixture.Cases = append(fixture.Cases, input)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "pending_update_cleanup.json")
	if *updatePendingCleanup {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("pending Windows update cleanup differs from pinned Go reference")
	}
}
