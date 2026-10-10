use std::io;

pub fn deny_network() -> bool {
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
    for number in [libc::SYS_socket, libc::SYS_socketpair, libc::SYS_connect] {
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
        "owned process-wide network isolation: {}",
        io::Error::last_os_error()
    );
    assert_eq!(
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    let mut pair = [-1; 2];
    assert_eq!(
        unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    let address = libc::sockaddr {
        sa_family: libc::AF_INET as _,
        sa_data: [0; 14],
    };
    assert_eq!(
        unsafe { libc::connect(-1, &address, std::mem::size_of_val(&address) as _) },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    true
}
