use pixiv_app::{
    browser_cookies::{BrowserDirEntry, BrowserFiles, BrowserMetadata},
    browser_dpapi::{DpapiBlob, WindowsDpapiApi},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::{Context, ContextError},
};
use pixiv_sdk::context::{ContextFuture, RequestContext};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString, c_void},
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

pub fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(&mut text, "{byte:02x}").unwrap();
        text
    })
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

#[cfg(windows)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code as u32)
}

#[derive(Clone, Debug)]
pub struct ProcessCall {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub stdio: ProcessStdio,
}

pub struct SecretProcess {
    platform: HostPlatform,
    mode: String,
    output: Vec<u8>,
    pub calls: Mutex<Vec<ProcessCall>>,
    pub lookups: Mutex<Vec<OsString>>,
}

impl SecretProcess {
    pub fn new(platform: HostPlatform, mode: &str, output: Vec<u8>) -> Self {
        Self {
            platform,
            mode: mode.into(),
            output,
            calls: Mutex::new(Vec::new()),
            lookups: Mutex::new(Vec::new()),
        }
    }

    pub fn started_args(&self) -> Value {
        let calls = self.calls.lock().unwrap();
        if let Some(call) = calls.first() {
            assert_eq!(calls.len(), 1);
            assert_eq!(call.stdio, ProcessStdio::CaptureStdout);
            json!(
                call.args
                    .iter()
                    .map(|arg| arg.to_str().unwrap())
                    .collect::<Vec<_>>()
            )
        } else {
            json!([])
        }
    }

    fn exit_error(&self, program: &OsStr, stderr: Vec<u8>, code: i32) -> HostProcessError {
        HostProcessError::Exit {
            program: program.into(),
            output: ProcessOutput {
                status: exit_status(code),
                stdout: Vec::new(),
                stderr,
            },
        }
    }
}

impl HostProcess for SecretProcess {
    fn platform(&self) -> HostPlatform {
        self.platform
    }

    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        self.lookups.lock().unwrap().push(program.into());
        if matches!(self.mode.as_str(), "missing" | "missing-command") {
            Err(HostProcessError::Lookup {
                program: program.into(),
                source: io::Error::new(io::ErrorKind::NotFound, "synthetic-private-path"),
            })
        } else {
            Ok(PathBuf::from("/owned-synthetic-bin").join(program))
        }
    }

    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("secret wrappers must preserve the caller context")
    }

    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("secret wrappers must not open a browser")
    }
}

impl ContextHostProcess for SecretProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        self.look_path(program)?;
        if let Some(reason) = context.error() {
            return Err(HostProcessError::Context(reason));
        }
        self.calls.lock().unwrap().push(ProcessCall {
            program: program.into(),
            args: args.into(),
            stdio,
        });
        match self.mode.as_str() {
            "success" | "empty" => Ok(ProcessOutput {
                status: exit_status(0),
                stdout: self.output.clone(),
                stderr: Vec::new(),
            }),
            "success-cancels" => {
                context.cancel();
                Ok(ProcessOutput {
                    status: exit_status(0),
                    stdout: self.output.clone(),
                    stderr: Vec::new(),
                })
            }
            "item-not-found" => Err(self.exit_error(
                program,
                b"synthetic-private-diagnostic: item could not be found\n".to_vec(),
                44,
            )),
            "case-mismatch" => Err(self.exit_error(
                program,
                b"synthetic-private-diagnostic: item Could not be found\n".to_vec(),
                44,
            )),
            "wait" => {
                context.cancel();
                Err(self.exit_error(program, b"synthetic-private-cancellation".to_vec(), 1))
            }
            "failure" | "fail" => Err(self.exit_error(
                program,
                b"synthetic-private-diagnostic synthetic-password".to_vec(),
                2,
            )),
            other => panic!("unsupported synthetic process mode: {other}"),
        }
    }
}

pub struct StateFiles {
    root: PathBuf,
    state: Option<Vec<u8>>,
    pub reads: Mutex<Vec<PathBuf>>,
}

impl StateFiles {
    pub fn new(root: PathBuf, state: Option<Vec<u8>>) -> Self {
        Self {
            root,
            state,
            reads: Mutex::new(Vec::new()),
        }
    }
}

impl BrowserFiles for StateFiles {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push(path.into());
        assert_eq!(path, self.root.join("Local State"));
        self.state
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "synthetic-private-state-path"))
    }
    fn read_dir(&self, _: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        panic!("native key lookup must not discover profiles")
    }
    fn metadata(&self, _: &Path) -> io::Result<BrowserMetadata> {
        panic!("native key lookup must not inspect profile metadata")
    }
}

#[derive(Debug)]
pub struct CheckedContext {
    pub base: Context,
    pub checks: AtomicUsize,
    cancel_on_check: Option<usize>,
}

impl CheckedContext {
    pub fn new(cancel: &str, check: usize) -> Self {
        let base = Context::new();
        if cancel == "before" {
            base.cancel();
        }
        Self {
            base,
            checks: AtomicUsize::new(0),
            cancel_on_check: (cancel == "after-copy").then_some(check),
        }
    }
}

impl RequestContext for CheckedContext {
    fn error(&self) -> Option<ContextError> {
        let count = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        if self
            .cancel_on_check
            .is_some_and(|threshold| count >= threshold)
        {
            self.base.cancel();
        }
        self.base.error()
    }
    fn deadline(&self) -> Option<Instant> {
        self.base.deadline()
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        RequestContext::cancelled(&self.base)
    }
}

#[derive(Default)]
struct NativeObservation {
    native_calls: usize,
    free_calls: usize,
    allocated: bool,
    allocation_overwritten: bool,
    native_input: Vec<u8>,
    native_input_size: u32,
    optional_arguments_null: bool,
    flags_zero: bool,
    input_pointer_nonnull: bool,
    input_points_to_caller: bool,
    input_pointer_checked: bool,
    free_handle_matches: bool,
}

struct NativeState {
    observation: NativeObservation,
    allocation: Option<Box<[u8]>>,
}

pub struct NativeDpapi {
    mode: String,
    output: Vec<u8>,
    caller_pointer: Option<usize>,
    cancel_on_return: Option<Context>,
    state: Mutex<NativeState>,
}

impl NativeDpapi {
    pub fn new(
        mode: &str,
        output: Vec<u8>,
        caller_pointer: Option<*const u8>,
        cancel_on_return: Option<Context>,
    ) -> Self {
        Self {
            mode: mode.into(),
            output,
            caller_pointer: caller_pointer.map(|pointer| pointer as usize),
            cancel_on_return,
            state: Mutex::new(NativeState {
                observation: NativeObservation::default(),
                allocation: None,
            }),
        }
    }

    pub fn observation(&self) -> Value {
        let state = self.state.lock().unwrap();
        let observed = &state.observation;
        json!({
            "native_calls": observed.native_calls,
            "free_calls": observed.free_calls,
            "allocated": observed.allocated,
            "allocation_overwritten": observed.allocation_overwritten,
            "native_input_hex": encode_hex(&observed.native_input),
            "native_input_size": observed.native_input_size,
            "optional_arguments_null": observed.optional_arguments_null,
            "flags_zero": observed.flags_zero,
            "input_pointer_nonnull": observed.input_pointer_nonnull,
            "input_points_to_caller": observed.input_points_to_caller,
            "input_pointer_checked": observed.input_pointer_checked,
            "free_handle_matches": observed.free_handle_matches,
            "pointer_width": usize::BITS,
            "data_blob_size": std::mem::size_of::<DpapiBlob>(),
            "data_blob_data_offset": std::mem::offset_of!(DpapiBlob, data),
        })
    }

    pub fn residual_allocation_is_owned(&self) -> bool {
        self.state.lock().unwrap().allocation.is_some()
    }
}

// The boxes keep early-return allocations alive for test teardown, without inventing a LocalFree call.
unsafe impl WindowsDpapiApi for NativeDpapi {
    unsafe fn crypt_unprotect_data(
        &self,
        input: *const DpapiBlob,
        description: *mut *mut u16,
        entropy: *const DpapiBlob,
        reserved: *mut c_void,
        prompt: *const c_void,
        flags: u32,
        output: *mut DpapiBlob,
    ) -> bool {
        let input = unsafe { &*input };
        let output = unsafe { &mut *output };
        let mut state = self.state.lock().unwrap();
        let observed = &mut state.observation;
        observed.native_calls += 1;
        observed.native_input_size = input.size;
        observed.input_pointer_nonnull = !input.data.is_null();
        observed.native_input =
            unsafe { std::slice::from_raw_parts(input.data, input.size as usize) }.to_vec();
        observed.optional_arguments_null =
            description.is_null() && entropy.is_null() && reserved.is_null() && prompt.is_null();
        observed.flags_zero = flags == 0;
        if let Some(caller_pointer) = self.caller_pointer {
            observed.input_pointer_checked = true;
            observed.input_points_to_caller = input.data as usize == caller_pointer;
        }
        if self.mode == "failure" {
            return false;
        }
        if self.mode == "null-output" {
            output.size = self.output.len() as u32;
            return true;
        }
        let bytes = if self.output.is_empty() {
            vec![0x5a]
        } else {
            self.output.clone()
        };
        let mut allocation = bytes.into_boxed_slice();
        output.data = allocation.as_mut_ptr();
        output.size = if self.mode == "zero-size-allocation" {
            0
        } else {
            self.output.len() as u32
        };
        state.observation.allocated = true;
        state.allocation = Some(allocation);
        if let Some(context) = &self.cancel_on_return {
            context.cancel();
        }
        self.mode != "failure-with-allocation"
    }

    unsafe fn local_free(&self, memory: *mut c_void) -> *mut c_void {
        let mut state = self.state.lock().unwrap();
        state.observation.free_calls += 1;
        let Some(mut allocation) = state.allocation.take() else {
            return memory;
        };
        state.observation.free_handle_matches = memory == allocation.as_mut_ptr().cast();
        assert!(state.observation.free_handle_matches);
        allocation.fill(0xa5);
        state.observation.allocation_overwritten = true;
        drop(allocation);
        std::ptr::null_mut()
    }
}
