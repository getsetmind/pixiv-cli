use crate::{CommandError, auth_accounts::AccountPrompts};
use std::io::{self, IsTerminal, Stderr, Stdin, Stdout, Write};

pub struct TerminalPrompts {
    input: Stdin,
    output: Stdout,
    _error: Stderr,
    color: bool,
}

impl TerminalPrompts {
    pub fn new(input: Stdin, output: Stdout, error: Stderr) -> Self {
        Self {
            input,
            output,
            _error: error,
            color: colors_enabled(),
        }
    }

    fn begin(&mut self) -> Result<platform::Mode, CommandError> {
        if !self.can_prompt() {
            return Err(CommandError::Message(
                "interactive prompt is only available on a TTY",
            ));
        }
        let mode = platform::Mode::new(&self.input, &self.output)?;
        if let Err(error) = self
            .output
            .write_all(b"\x1b7\x1b[?25l")
            .and_then(|()| self.output.flush())
        {
            let _ = self.output.write_all(b"\x1b[?25h");
            return Err(error.into());
        }
        Ok(mode)
    }

    fn finish<T>(
        &mut self,
        mode: platform::Mode,
        result: Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let cursor = self
            .output
            .write_all(b"\x1b[?25h")
            .and_then(|()| self.output.flush());
        let restored = mode.restore();
        match result {
            Err(error) => Err(error),
            Ok(value) => {
                cursor?;
                restored?;
                Ok(value)
            }
        }
    }

    fn draw(&mut self, body: &str) -> Result<(), CommandError> {
        self.output.write_all(b"\x1b8\x1b[J")?;
        let mut body = body.to_owned();
        if !self.color {
            for code in [
                "\x1b[1;92m",
                "\x1b[1;99m",
                "\x1b[36m",
                "\x1b[1;36m",
                "\x1b[39m",
                "\x1b[37m",
                "\x1b[31m",
                "\x1b[0m",
            ] {
                body = body.replace(code, "");
            }
        }
        self.output.write_all(body.as_bytes())?;
        self.output.flush()?;
        Ok(())
    }

    fn choose(&mut self, message: &str, options: &[String]) -> Result<String, CommandError> {
        let mut filter = String::new();
        let mut selected = 0usize;
        let mut vim = false;
        loop {
            let lowered = simple_lowercase(&filter);
            let visible: Vec<&String> = options
                .iter()
                .filter(|option| simple_lowercase(option).contains(&lowered))
                .collect();
            if !visible.is_empty() {
                selected = selected.min(visible.len() - 1);
            }
            let suffix = if filter.is_empty() {
                String::new()
            } else {
                format!(" {filter}")
            };
            let mut body = format!(
                "\x1b[1;92m? \x1b[0m\x1b[1;99m{message}{suffix}\x1b[0m  \x1b[36m[Use arrows to move, type to filter]\x1b[0m\n"
            );
            let start = if visible.len() <= 7 || selected < 3 {
                0
            } else if visible.len() - selected - 1 < 3 {
                visible.len() - 7
            } else {
                selected - 3
            };
            for (index, option) in visible.iter().enumerate().skip(start).take(7) {
                body.push_str(&format!(
                    "{}{option}\x1b[0m\n",
                    if index == selected {
                        "\x1b[1;36m> "
                    } else {
                        "\x1b[39m  "
                    }
                ));
            }
            self.draw(&body)?;
            let key = platform::read_key(&mut self.input)?;
            match key {
                '\x03' => return Err(CommandError::Message("interrupt")),
                '\r' | '\n' | '\x04' => {
                    if let Some(value) = visible.get(selected) {
                        let answer = value.trim().to_owned();
                        self.draw(&format!(
                            "\x1b[1;92m? \x1b[0m\x1b[1;99m{message}\x1b[0m\x1b[36m {value}\x1b[0m\n"
                        ))?;
                        return Ok(answer);
                    }
                    if key == '\x04' {
                        return Err(CommandError::Message("no available options"));
                    }
                }
                '\x10' if !visible.is_empty() => {
                    selected = if selected == 0 {
                        visible.len() - 1
                    } else {
                        selected - 1
                    }
                }
                '\x0e' | '\t' if !visible.is_empty() => selected = (selected + 1) % visible.len(),
                'k' if vim && !visible.is_empty() => {
                    selected = if selected == 0 {
                        visible.len() - 1
                    } else {
                        selected - 1
                    }
                }
                'j' if vim && !visible.is_empty() => selected = (selected + 1) % visible.len(),
                '\x1b' => vim = !vim,
                '\x17' | '\x18' => filter.clear(),
                '\x08' | '\x7f' => {
                    filter.pop();
                }
                key if key >= ' ' => {
                    filter.push(key);
                    vim = false;
                }
                _ => {}
            }
        }
    }

    fn read_line(&mut self, prefix: &str, secret: bool) -> Result<String, CommandError> {
        let mut line = Vec::<char>::new();
        let mut position = 0usize;
        loop {
            let text = if secret {
                "*".repeat(line.len())
            } else {
                line.iter().collect()
            };
            self.draw(&format!("{prefix}{text}"))?;
            let remaining: usize = if secret {
                line.len() - position
            } else {
                line[position..]
                    .iter()
                    .map(|character| {
                        unicode_width::UnicodeWidthChar::width(*character)
                            .unwrap_or(0)
                            .max(1)
                    })
                    .sum()
            };
            if remaining > 0 {
                write!(self.output, "\x1b[{remaining}D")?;
                self.output.flush()?;
            }
            match platform::read_key(&mut self.input)? {
                '\x03' => return Err(CommandError::Message("interrupt")),
                '\r' | '\n' | '\x04' => return Ok(line.iter().collect()),
                '\x08' | '\x7f' if position > 0 => {
                    position -= 1;
                    line.remove(position);
                }
                '\x02' if position > 0 => position -= 1,
                '\x06' if position < line.len() => position += 1,
                '\x01' => position = 0,
                '\x11' => position = line.len(),
                '\x12' if position < line.len() => {
                    line.remove(position);
                }
                key if !key.is_control() => {
                    line.insert(position, key);
                    position += 1;
                }
                _ => {}
            }
        }
    }

    fn ask_secret(&mut self, message: &str) -> Result<String, CommandError> {
        let prefix = format!("\x1b[1;92m? \x1b[0m\x1b[1;99m{message} \x1b[0m");
        let mut validation = String::new();
        loop {
            let answer = self.read_line(&format!("{validation}{prefix}"), true)?;
            if answer.trim().is_empty() {
                validation =
                    "\x1b[31mX Sorry, your reply was invalid: value cannot be empty\x1b[0m\n"
                        .into();
                continue;
            }
            self.draw(&format!("{prefix}{}\n", "*".repeat(answer.chars().count())))?;
            return Ok(answer.trim().to_owned());
        }
    }

    fn ask_input(&mut self, message: &str, default: &str) -> Result<String, CommandError> {
        let prefix = format!("\x1b[1;92m? \x1b[0m\x1b[1;99m{message} \x1b[0m");
        let mut validation = String::new();
        loop {
            let shown_default = if default.is_empty() {
                String::new()
            } else {
                format!("\x1b[37m({default}) \x1b[0m")
            };
            let answer = self.read_line(&format!("{validation}{prefix}{shown_default}"), false)?;
            let answer = if answer.is_empty() { default } else { &answer };
            if answer.trim().is_empty() {
                validation =
                    "\x1b[31mX Sorry, your reply was invalid: value cannot be empty\x1b[0m\n"
                        .into();
                continue;
            }
            self.draw(&format!("{prefix}\x1b[36m{answer}\x1b[0m"))?;
            return Ok(answer.trim().to_owned());
        }
    }

    fn ask_confirmation(&mut self, message: &str, default: bool) -> Result<bool, CommandError> {
        let mut validation = String::new();
        loop {
            let prefix = format!(
                "\x1b[1;92m? \x1b[0m\x1b[1;99m{message} \x1b[0m\x1b[37m{} \x1b[0m",
                if default { "(Y/n)" } else { "(y/N)" }
            );
            let text = self.read_line(&format!("{validation}{prefix}"), false)?;
            let answer = match text.to_ascii_lowercase().as_str() {
                "" => default,
                "y" | "yes" => true,
                "n" | "no" => false,
                _ => {
                    validation = format!(
                        "\x1b[31mX Sorry, your reply was invalid: {} is not a valid answer, please try again.\x1b[0m\n",
                        crate::search::quote(&text)
                    );
                    continue;
                }
            };
            self.draw(&format!(
                "\x1b[1;92m? \x1b[0m\x1b[1;99m{message} \x1b[0m\x1b[36m{}\x1b[0m\n",
                if answer { "Yes" } else { "No" }
            ))?;
            return Ok(answer);
        }
    }
}

fn colors_enabled() -> bool {
    let forced = std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0");
    let disabled = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
        || std::env::var_os("CLICOLOR").is_some_and(|value| value == "0");
    forced || !disabled
}

fn simple_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect()
}

impl AccountPrompts for TerminalPrompts {
    fn can_prompt(&self) -> bool {
        self.input.is_terminal() && self.output.is_terminal()
    }
    fn select(&mut self, message: &str, options: &[String]) -> Result<String, CommandError> {
        if options.is_empty() {
            return Err(CommandError::Message("no available options"));
        }
        let mode = self.begin()?;
        let result = self.choose(message, options);
        self.finish(mode, result)
    }
    fn input(&mut self, message: &str, default: &str) -> Result<String, CommandError> {
        let mode = self.begin()?;
        let result = self.ask_input(message, default);
        self.finish(mode, result)
    }
    fn secret(&mut self, message: &str) -> Result<String, CommandError> {
        let mode = self.begin()?;
        let result = self.ask_secret(message);
        self.finish(mode, result)
    }
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, CommandError> {
        let mode = self.begin()?;
        let result = self.ask_confirmation(message, default);
        self.finish(mode, result)
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::fd::AsRawFd;

    pub struct Mode {
        fd: i32,
        previous: libc::termios,
        active: bool,
    }
    impl Mode {
        pub fn new(input: &Stdin, _output: &Stdout) -> io::Result<Self> {
            let fd = input.as_raw_fd();
            let mut previous = std::mem::MaybeUninit::<libc::termios>::uninit();
            if unsafe { libc::tcgetattr(fd, previous.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            let previous = unsafe { previous.assume_init() };
            let mut raw = previous;
            raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                fd,
                previous,
                active: true,
            })
        }
        pub fn restore(mut self) -> io::Result<()> {
            let result = self.reset();
            if result.is_ok() {
                self.active = false;
            }
            result
        }
        fn reset(&self) -> io::Result<()> {
            if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.previous) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
    impl Drop for Mode {
        fn drop(&mut self) {
            if self.active {
                let _ = self.reset();
            }
        }
    }

    fn read_exact(fd: i32, mut bytes: &mut [u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
            }
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            bytes = &mut bytes[count as usize..];
        }
        Ok(())
    }
    fn character(input: &mut Stdin) -> io::Result<char> {
        let mut bytes = [0; 4];
        read_exact(input.as_raw_fd(), &mut bytes[..1])?;
        let count = match bytes[0] {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stdin value is not valid UTF-8",
                ));
            }
        };
        read_exact(input.as_raw_fd(), &mut bytes[1..count])?;
        std::str::from_utf8(&bytes[..count])
            .ok()
            .and_then(|text| text.chars().next())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "stdin value is not valid UTF-8")
            })
    }
    pub fn read_key(input: &mut Stdin) -> Result<char, CommandError> {
        let key = character(input)?;
        if key != '\x1b' {
            return Ok(key);
        }
        let mut poll = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let pending = unsafe { libc::poll(&mut poll, 1, 25) };
        if pending < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if pending == 0 {
            return Ok(key);
        }
        let keypad = character(input)?;
        if keypad != '[' && keypad != 'O' {
            return Err(CommandError::MessageText(format!(
                "unexpected escape sequence from terminal: {}",
                crate::search::quote(&format!("\x1b{keypad}"))
            )));
        }
        let final_key = character(input)?;
        Ok(match final_key {
            'A' => '\x10',
            'B' => '\x0e',
            'C' => '\x06',
            'D' => '\x02',
            'F' => '\x11',
            'H' => '\x01',
            '3' if keypad == '[' => {
                let _ = character(input)?;
                '\x12'
            }
            _ => {
                let _ = character(input)?;
                '\0'
            }
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::HANDLE, System::Console::*};
    pub struct Mode {
        input: HANDLE,
        output: HANDLE,
        input_mode: u32,
        output_mode: u32,
        active: bool,
    }
    impl Mode {
        pub fn new(input: &Stdin, output: &Stdout) -> io::Result<Self> {
            let mut mode = Self {
                input: input.as_raw_handle(),
                output: output.as_raw_handle(),
                input_mode: 0,
                output_mode: 0,
                active: false,
            };
            if unsafe { GetConsoleMode(mode.input, &mut mode.input_mode) } == 0
                || unsafe { GetConsoleMode(mode.output, &mut mode.output_mode) } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if unsafe {
                SetConsoleMode(
                    mode.input,
                    mode.input_mode
                        & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            mode.active = true;
            if unsafe {
                SetConsoleMode(
                    mode.output,
                    mode.output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(mode)
        }
        pub fn restore(mut self) -> io::Result<()> {
            let input = unsafe { SetConsoleMode(self.input, self.input_mode) };
            let output = unsafe { SetConsoleMode(self.output, self.output_mode) };
            if input == 0 || output == 0 {
                return Err(io::Error::last_os_error());
            }
            self.active = false;
            Ok(())
        }
    }
    impl Drop for Mode {
        fn drop(&mut self) {
            if self.active {
                unsafe {
                    SetConsoleMode(self.input, self.input_mode);
                    SetConsoleMode(self.output, self.output_mode);
                }
            }
        }
    }
    pub fn read_key(input: &mut Stdin) -> Result<char, CommandError> {
        let mut surrogate = None;
        loop {
            let mut record = std::mem::MaybeUninit::<INPUT_RECORD>::uninit();
            let mut count = 0;
            if unsafe {
                ReadConsoleInputW(input.as_raw_handle(), record.as_mut_ptr(), 1, &mut count)
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            let record = unsafe { record.assume_init() };
            if record.EventType != KEY_EVENT as u16 {
                continue;
            }
            let event = unsafe { record.Event.KeyEvent };
            if event.bKeyDown == 0 {
                continue;
            }
            let unit = unsafe { event.uChar.UnicodeChar };
            if event.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0
                && unit == u16::from(b'C')
            {
                return Ok('\x03');
            }
            if unit == 0 {
                let key = match event.wVirtualKeyCode {
                    0x26 => '\x10',
                    0x28 => '\x0e',
                    0x25 => '\x02',
                    0x27 => '\x06',
                    0x24 => '\x01',
                    0x23 => '\x11',
                    0x2e => '\x12',
                    _ => continue,
                };
                return Ok(key);
            }
            if (0xd800..=0xdbff).contains(&unit) {
                surrogate = Some(unit);
                continue;
            }
            let point = if let Some(high) = surrogate.take() {
                if !(0xdc00..=0xdfff).contains(&unit) {
                    return Err(CommandError::Message("stdin value is not valid UTF-16"));
                }
                0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00
            } else {
                u32::from(unit)
            };
            return char::from_u32(point)
                .ok_or(CommandError::Message("stdin value is not valid UTF-16"));
        }
    }
}
