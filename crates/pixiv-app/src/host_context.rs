use crate::{
    host_process::{
        HostProcess, HostProcessError, ProcessOutput, ProcessStdio, SystemHostProcess,
        collect_child_output, prepare_command,
    },
    lifecycle::Context,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    process::Child,
    thread,
    time::Duration,
};

pub trait ContextHostProcess: HostProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError>;
}

fn wait_context(
    child: &mut Child,
    context: &Context,
) -> Result<(std::process::ExitStatus, Option<HostProcessError>), io::Error> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, None));
        }
        if let Some(reason) = context.error() {
            let canceled = match child.kill() {
                Ok(()) => Some(HostProcessError::Context(reason)),
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => None,
                #[cfg(unix)]
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => None,
                Err(error) => Some(HostProcessError::Native(io::Error::other(format!(
                    "exec: canceling Cmd: {error}"
                )))),
            };
            return child.wait().map(|status| (status, canceled));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

impl ContextHostProcess for SystemHostProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        let (executable, mut command) = prepare_command(self, program, args, stdio)?;
        if let Some(reason) = context.error() {
            return Err(HostProcessError::Context(reason));
        }
        let child = command.spawn().map_err(|source| HostProcessError::Spawn {
            program: executable.into_os_string(),
            source,
        })?;
        collect_child_output(program, stdio, child, |child| wait_context(child, context))
    }
}
