#![allow(non_snake_case, non_camel_case_types)]

mod lifecycle {
    use std::{
        fmt,
        sync::atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ContextError {
        Canceled,
    }
    impl fmt::Display for ContextError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("context canceled")
        }
    }
    impl std::error::Error for ContextError {}

    #[derive(Debug, Default)]
    pub struct Context(AtomicBool);
    impl Context {
        pub fn error(&self) -> Option<ContextError> {
            self.0
                .load(Ordering::SeqCst)
                .then_some(ContextError::Canceled)
        }
        pub fn cancel(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    size: u32,
    mask: u32,
    show: i32,
    class: Vec<u16>,
    file: Vec<u16>,
    omitted: [usize; 10],
}

#[derive(Debug, Default)]
struct State {
    result: i32,
    last_error: u32,
    last_error_reads: usize,
    calls: Vec<Snapshot>,
}
static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State {
    result: 1,
    last_error: 0,
    last_error_reads: 0,
    calls: Vec::new(),
});

mod windows_sys {
    pub mod Win32 {
        pub mod Foundation {
            pub unsafe fn GetLastError() -> u32 {
                let mut state = crate::STATE.lock().unwrap();
                state.last_error_reads += 1;
                state.last_error
            }
        }
        pub mod UI {
            pub mod WindowsAndMessaging {
                pub const SW_SHOWNORMAL: i32 = 1;
            }
            pub mod Shell {
                use std::ffi::c_void;

                pub const SEE_MASK_CLASSNAME: u32 = 1;

                #[repr(C)]
                #[derive(Clone, Copy)]
                pub union SHELLEXECUTEINFOW_0 {
                    pub hIcon: *mut c_void,
                    pub hMonitor: *mut c_void,
                }

                #[repr(C)]
                pub struct SHELLEXECUTEINFOW {
                    pub cbSize: u32,
                    pub fMask: u32,
                    pub hwnd: *mut c_void,
                    pub lpVerb: *const u16,
                    pub lpFile: *const u16,
                    pub lpParameters: *const u16,
                    pub lpDirectory: *const u16,
                    pub nShow: i32,
                    pub hInstApp: *mut c_void,
                    pub lpIDList: *mut c_void,
                    pub lpClass: *const u16,
                    pub hkeyClass: *mut c_void,
                    pub dwHotKey: u32,
                    pub Anonymous: SHELLEXECUTEINFOW_0,
                    pub hProcess: *mut c_void,
                }
                impl Default for SHELLEXECUTEINFOW {
                    fn default() -> Self {
                        unsafe { std::mem::zeroed() }
                    }
                }

                unsafe fn read_utf16(pointer: *const u16) -> Vec<u16> {
                    assert!(!pointer.is_null());
                    let mut units = Vec::new();
                    for offset in 0..4096 {
                        let unit = unsafe { *pointer.add(offset) };
                        units.push(unit);
                        if unit == 0 {
                            return units;
                        }
                    }
                    panic!("UTF16 argument has no terminator");
                }

                pub unsafe fn ShellExecuteExW(pointer: *mut SHELLEXECUTEINFOW) -> i32 {
                    assert!(!pointer.is_null());
                    let info = unsafe { &*pointer };
                    let snapshot = crate::Snapshot {
                        size: info.cbSize,
                        mask: info.fMask,
                        show: info.nShow,
                        class: unsafe { read_utf16(info.lpClass) },
                        file: unsafe { read_utf16(info.lpFile) },
                        omitted: [
                            info.hwnd as usize,
                            info.lpVerb as usize,
                            info.lpParameters as usize,
                            info.lpDirectory as usize,
                            info.hInstApp as usize,
                            info.lpIDList as usize,
                            info.hkeyClass as usize,
                            info.dwHotKey as usize,
                            unsafe { info.Anonymous.hIcon as usize },
                            info.hProcess as usize,
                        ],
                    };
                    let mut state = crate::STATE.lock().unwrap();
                    state.calls.push(snapshot);
                    state.result
                }
            }
        }
    }
}

fn configure(result: i32, last_error: u32) {
    *STATE.lock().unwrap() = State {
        result,
        last_error,
        ..State::default()
    };
}

#[test]
fn native_branch_matches_frozen_class_shell_contracts() {
    use lifecycle::{Context, ContextError};
    use windows_shell::{SystemWindowsShell, WindowsShell, WindowsShellError};
    use windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW;

    let size = if std::mem::size_of::<usize>() == 8 {
        112
    } else {
        60
    };
    assert_eq!(std::mem::size_of::<SHELLEXECUTEINFOW>(), size);
    let offsets = [
        std::mem::offset_of!(SHELLEXECUTEINFOW, cbSize),
        std::mem::offset_of!(SHELLEXECUTEINFOW, fMask),
        std::mem::offset_of!(SHELLEXECUTEINFOW, hwnd),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpVerb),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpFile),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpParameters),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpDirectory),
        std::mem::offset_of!(SHELLEXECUTEINFOW, nShow),
        std::mem::offset_of!(SHELLEXECUTEINFOW, hInstApp),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpIDList),
        std::mem::offset_of!(SHELLEXECUTEINFOW, lpClass),
        std::mem::offset_of!(SHELLEXECUTEINFOW, hkeyClass),
        std::mem::offset_of!(SHELLEXECUTEINFOW, dwHotKey),
        std::mem::offset_of!(SHELLEXECUTEINFOW, Anonymous),
        std::mem::offset_of!(SHELLEXECUTEINFOW, hProcess),
    ];
    assert_eq!(
        offsets,
        if size == 112 {
            [0, 4, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104]
        } else {
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56]
        }
    );

    for (class, file) in [
        (
            "Previous.Handler.🐈",
            "  pixiv://unlisted?x=a%20b&text=絵🐈#fragment  ",
        ),
        ("", ""),
    ] {
        configure(1, 5);
        SystemWindowsShell
            .open_class(&Context::default(), class, file)
            .unwrap();
        let state = STATE.lock().unwrap();
        assert_eq!(state.last_error_reads, 0);
        assert_eq!(
            state.calls,
            [Snapshot {
                size: size as u32,
                mask: 1,
                show: 1,
                class: class.encode_utf16().chain(Some(0)).collect(),
                file: file.encode_utf16().chain(Some(0)).collect(),
                omitted: [0; 10],
            }]
        );
    }

    for (result, last_error) in [
        (0, 0),
        (0, 5),
        (0, 0x8000_0001),
        (1, 0),
        (1, 5),
        (2, 5),
        (32, 5),
        (-1, 5),
    ] {
        configure(result, last_error);
        let outcome = SystemWindowsShell.open_class(&Context::default(), "class", "url");
        if result == 0 {
            let error = outcome.unwrap_err();
            if last_error == 0 {
                assert_eq!(error.to_string(), "ShellExecuteExW failed");
                assert!(matches!(error, WindowsShellError::Message(_)));
            } else {
                assert!(
                    matches!(error, WindowsShellError::Native(source) if source.raw_os_error() == Some(last_error as i32))
                );
            }
        } else {
            outcome.unwrap();
        }
        let state = STATE.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.last_error_reads, usize::from(result == 0));
    }

    for (canceled, class, url) in [
        (true, "class\0", "url\0"),
        (false, "class\0", "url\0"),
        (false, "class", "url\0"),
    ] {
        configure(1, 0);
        let context = Context::default();
        if canceled {
            context.cancel();
        }
        let error = SystemWindowsShell
            .open_class(&context, class, url)
            .unwrap_err();
        if canceled {
            assert!(matches!(
                error,
                WindowsShellError::Context(ContextError::Canceled)
            ));
        } else {
            assert!(
                matches!(error, WindowsShellError::Native(source) if source.kind() == std::io::ErrorKind::InvalidInput)
            );
        }
        let state = STATE.lock().unwrap();
        assert!(state.calls.is_empty());
        assert_eq!(state.last_error_reads, 0);
    }
}
