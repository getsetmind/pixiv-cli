use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, OnceLock, Weak},
};

const OWNED_HELPER_SOURCE: &str = r#"package main
import (
 "bytes"
 "context"
 "encoding/json"
 "errors"
 "fmt"
 "io"
 "os"
 "strconv"
 "strings"
 "time"
)
type ownedWriter struct {
 mode string
 output *bytes.Buffer
 cancel context.CancelFunc
}
func (w *ownedWriter) Write(p []byte) (int, error) {
 switch w.mode {
 case "error": return 0, errors.New("owned writer failure")
 case "short": return len(p)-1, nil
 case "oversize": return len(p)+1, nil
 }
 n, err := w.output.Write(p)
 if w.cancel != nil && bytes.Contains(p, []byte("started\n")) { w.cancel() }
 return n, err
}
func oracle(args []string) {
 ctx, cancel := context.WithCancel(context.Background())
 defer cancel()
 if args[1] == "precanceled" { cancel() }
 var out, errOut bytes.Buffer
 var stdout io.Writer = &ownedWriter{mode: args[0], output: &out}
 var stderr io.Writer = &ownedWriter{output: &errOut}
 if args[0] == "stderr-error" { stderr = &ownedWriter{mode: "error", output: &errOut} }
 if args[0] == "nil" { stdout, stderr = nil, nil }
 if args[0] == "combined" { stderr = stdout }
 if args[1] == "cancel-after-start" { stdout.(*ownedWriter).cancel = cancel }
 err := NewCommandRunner(stdout, stderr).Run(ctx, Command{Name: args[2], Args: args[3:]})
 message := ""
 if err != nil { message = err.Error() }
 json.NewEncoder(os.Stdout).Encode(map[string]interface{}{
  "error": message, "stdout": out.String(), "stderr": errOut.String(),
  "canceled_cause": errors.Is(err, context.Canceled),
  "deadline_cause": errors.Is(err, context.DeadlineExceeded),
 })
}
func main() {
 switch os.Args[1] {
 case "oracle": oracle(os.Args[2:])
 case "args":
  json.NewEncoder(os.Stdout).Encode(os.Args[2:])
  fmt.Fprint(os.Stderr, "owned stderr\n")
 case "large":
  fmt.Fprint(os.Stdout, strings.Repeat("o", 131072))
  fmt.Fprint(os.Stderr, strings.Repeat("e", 131072))
 case "exit":
  fmt.Fprint(os.Stdout, "owned output\n")
  fmt.Fprint(os.Stderr, "owned error\n")
  code, _ := strconv.Atoi(os.Args[2])
  os.Exit(code)
 case "hold":
  os.WriteFile(os.Args[2], []byte("0"), 0600)
  fmt.Fprint(os.Stdout, "started\n")
  for i := 1; ; i++ {
   os.WriteFile(os.Args[2], []byte(strconv.Itoa(i)), 0600)
   time.Sleep(2 * time.Millisecond)
  }
 case "write":
  fmt.Fprint(os.Stdout, "owned output\n")
  time.Sleep(20 * time.Millisecond)
  fmt.Fprint(os.Stderr, "owned error\n")
  time.Sleep(20 * time.Millisecond)
  code, _ := strconv.Atoi(os.Args[2])
  os.Exit(code)
 }
}
"#;

pub struct OwnedCommandHelper {
    pub directory: tempfile::TempDir,
    pub executable: PathBuf,
    _compiled: Arc<CompiledHelper>,
}

struct CompiledHelper {
    _directory: tempfile::TempDir,
    executable: PathBuf,
}

impl OwnedCommandHelper {
    pub fn build() -> Self {
        static COMPILED: OnceLock<Mutex<Weak<CompiledHelper>>> = OnceLock::new();
        let mut cached = COMPILED
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .unwrap();
        let compiled = match cached.upgrade() {
            Some(compiled) => compiled,
            None => {
                let compiled = Arc::new(Self::compile());
                *cached = Arc::downgrade(&compiled);
                compiled
            }
        };
        let directory = tempfile::tempdir().unwrap();
        let executable = compiled.executable.clone();
        Self {
            directory,
            executable,
            _compiled: compiled,
        }
    }

    pub fn oracle(
        &self,
        writer: &str,
        context: &str,
        name: &str,
        args: &[String],
    ) -> serde_json::Value {
        let output = Command::new(&self.executable)
            .args(["oracle", writer, context, name])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "owned Go command oracle exited: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn compile() -> CompiledHelper {
        let toolchain = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("go-toolchain/go");
        let compiler = toolchain.join("bin/go");
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../pixiv-cli/tests/fixtures/updater-owned-preflight.json"
        ))
        .unwrap();
        let digest = Sha256::digest(fs::read(&compiler).unwrap());
        let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(digest, fixture["helper"]["compiler_sha256"]);
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.go");
        let executable = directory.path().join("owned-command-helper");
        fs::write(&source, OWNED_HELPER_SOURCE).unwrap();
        let runner = fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../internal/update/process/process_runner.go"),
        )
        .unwrap();
        let digest: String = Sha256::digest(&runner)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            digest,
            "e73c4687ee16ea1f18901915908c83e2ce3d663e26501226bd61754e73c87245"
        );
        fs::write(
            directory.path().join("process_runner.go"),
            String::from_utf8(runner)
                .unwrap()
                .replacen("package process", "package main", 1),
        )
        .unwrap();
        let output = Command::new(&compiler)
            .current_dir(directory.path())
            .args([
                "build",
                "-trimpath",
                "-buildvcs=false",
                "-ldflags=-buildid=",
                "-o",
            ])
            .arg(&executable)
            .arg(".")
            .env("GOTOOLCHAIN", "local")
            .env("GOROOT", &toolchain)
            .env("GOENV", "off")
            .env("GOPROXY", "off")
            .env("GOOS", "linux")
            .env("GOARCH", "amd64")
            .env("GOWORK", "off")
            .env("GO111MODULE", "off")
            .env("CGO_ENABLED", "0")
            .env("GOCACHE", directory.path().join("go-cache"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "compile visible owned command helper: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o555)).unwrap();
        CompiledHelper {
            _directory: directory,
            executable,
        }
    }
}
