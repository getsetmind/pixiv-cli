use std::{
    io,
    process::{Child, Command, Output},
    time::{Duration, Instant},
};

struct OwnedChild(Option<Child>);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct Outcome {
    pub timed_out: bool,
    pub output: Output,
}

pub fn fifo_reader_is_blocked() -> io::Result<bool> {
    for entry in std::fs::read_dir("/proc/self/task")? {
        match std::fs::read_to_string(entry?.path().join("wchan")) {
            Ok(channel) if channel.trim() == "wait_for_partner" => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub async fn run(command: &mut Command, timeout: Duration) -> io::Result<Outcome> {
    let mut owner = OwnedChild(Some(command.spawn()?));
    let deadline = Instant::now() + timeout;
    loop {
        let child = owner.0.as_mut().unwrap();
        if child.try_wait()?.is_some() {
            return Ok(Outcome {
                timed_out: false,
                output: owner.0.take().unwrap().wait_with_output()?,
            });
        }
        if Instant::now() >= deadline {
            child.kill()?;
            return Ok(Outcome {
                timed_out: true,
                output: owner.0.take().unwrap().wait_with_output()?,
            });
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
