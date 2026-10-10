use std::{ffi::CString, io};

pub fn deny_external() -> (bool, bool) {
    fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }
    let denied = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let mut filter = vec![
        instruction(0x20, 0, 0, 4),
        instruction(0x15, 1, 0, 0xc000003e),
        instruction(0x06, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        instruction(0x20, 0, 0, 0),
        instruction(0x35, 0, 1, 0x40000000),
        instruction(0x06, 0, 0, denied),
    ];
    for number in [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_execve,
        libc::SYS_execveat,
    ] {
        filter.push(instruction(0x15, 0, 1, number as u32));
        filter.push(instruction(0x06, 0, 0, denied));
    }
    filter.push(instruction(0x06, 0, 0, libc::SECCOMP_RET_ALLOW));
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                libc::SECCOMP_FILTER_FLAG_TSYNC,
                &program,
            )
        },
        0,
        "owned process-wide syscall isolation: {}",
        io::Error::last_os_error()
    );
    assert_eq!(
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    let executable = CString::new("/proc/self/exe").unwrap();
    let argument = CString::new("owned-download-isolation-probe").unwrap();
    let arguments = [argument.as_ptr(), std::ptr::null()];
    let environment = [std::ptr::null()];
    assert_eq!(
        unsafe {
            libc::execve(
                executable.as_ptr(),
                arguments.as_ptr(),
                environment.as_ptr(),
            )
        },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    (true, true)
}

pub fn network_filter() -> Vec<libc::sock_filter> {
    let mut filter = vec![
        libc::sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 4,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 1,
            jf: 0,
            k: 0xc000003e,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_KILL_PROCESS,
        },
        libc::sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: 0x35,
            jt: 0,
            jf: 1,
            k: 0x40000000,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        },
    ];
    for number in [libc::SYS_socket, libc::SYS_connect] {
        filter.push(libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: number as u32,
        });
        filter.push(libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        });
    }
    filter.extend([
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 4,
            k: libc::SYS_socketpair as u32,
        },
        libc::sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 16,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 1,
            jf: 0,
            k: libc::AF_UNIX as u32,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ]);
    filter
}
pub fn apply_network_filter(filter: &[libc::sock_filter]) -> io::Result<()> {
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0,
            &program,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut pair = [-1; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            pair.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        libc::close(pair[0]);
        libc::close(pair[1]);
    }
    for family in [libc::AF_INET, libc::AF_INET6] {
        let fd = unsafe { libc::socket(family, libc::SOCK_STREAM, 0) };
        if fd != -1 {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
            return Err(io::Error::last_os_error());
        }
        let address = libc::sockaddr {
            sa_family: family as libc::sa_family_t,
            sa_data: [0; 14],
        };
        if unsafe {
            libc::connect(
                -1,
                &address,
                std::mem::size_of_val(&address) as libc::socklen_t,
            )
        } != -1
        {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
