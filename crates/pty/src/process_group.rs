//! Process-group helpers shared by the pipe and PTY backends.
//!
//! They ensure a spawned command can be cleaned up reliably: the child starts
//! its own process group (and session, for pipes), and signals target the whole
//! group rather than a single PID.

use std::io;

#[cfg(target_os = "linux")]
/// Ensure the child receives SIGTERM when the original parent dies.
///
/// This runs in `pre_exec` and uses `parent_pid` captured before spawn to avoid
/// a race where the parent exits between fork and exec.
pub fn set_parent_death_signal(parent_pid: libc::pid_t) -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } == -1 {
        return Err(io::Error::last_os_error());
    }

    if unsafe { libc::getppid() } != parent_pid {
        unsafe {
            libc::raise(libc::SIGTERM);
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
/// No-op on non-Linux platforms.
pub fn set_parent_death_signal(_parent_pid: i32) -> io::Result<()> {
    Ok(())
}

/// Detach from the controlling TTY by starting a new session.
pub fn detach_from_tty() -> io::Result<()> {
    let result = unsafe { libc::setsid() };
    if result == -1 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EPERM) {
            return set_process_group();
        }
        return Err(err);
    }
    Ok(())
}

/// Put the calling process into its own process group.
///
/// Intended for use in `pre_exec` so the child becomes the group leader.
fn set_process_group() -> io::Result<()> {
    let result = unsafe { libc::setpgid(0, 0) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn signal_process_group_id(pgid: libc::pid_t, signal: libc::c_int) -> io::Result<bool> {
    let result = unsafe { libc::killpg(pgid, signal) };
    if result == -1 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(err);
    }

    Ok(true)
}

#[cfg(target_os = "macos")]
fn signal_process_id(pid: libc::pid_t, signal: libc::c_int) -> io::Result<bool> {
    if unsafe { libc::kill(pid, signal) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(error);
    }

    Ok(true)
}

#[cfg(target_os = "macos")]
fn signal_process_group_with_member_fallback(
    process_group_id: u32,
    signal: libc::c_int,
    signal_group: impl FnOnce(libc::pid_t, libc::c_int) -> io::Result<bool>,
    mut signal_member: impl FnMut(libc::pid_t, libc::c_int) -> io::Result<bool>,
) -> io::Result<bool> {
    let process_group_id = libc::pid_t::try_from(process_group_id)
        .ok()
        .filter(|process_group_id| *process_group_id > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process group ID"))?;

    match signal_group(process_group_id, signal) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {}
        result => return result,
    }

    let mut process_ids: Vec<libc::pid_t> = vec![0; 16];
    loop {
        let buffer_size = libc::c_int::try_from(std::mem::size_of_val(process_ids.as_slice()))
            .map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "process group is too large")
            })?;
        let count = unsafe {
            libc::proc_listpgrppids(
                process_group_id,
                process_ids.as_mut_ptr().cast(),
                buffer_size,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let count = count as usize;
        if count < process_ids.len() {
            process_ids.truncate(count);
            break;
        }
        let capacity = process_ids.len().checked_mul(2).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "process group is too large")
        })?;
        process_ids.resize(capacity, 0);
    }
    process_ids.sort_unstable_by_key(|process_id| *process_id == process_group_id);

    let mut signalled = false;
    let mut first_error = None;
    for process_id in process_ids {
        if process_id <= 0 {
            continue;
        }
        let current_group_id = unsafe { libc::getpgid(process_id) };
        if current_group_id == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) && first_error.is_none() {
                first_error = Some(error);
            }
            continue;
        }
        if current_group_id != process_group_id {
            continue;
        }
        match signal_member(process_id, signal) {
            Ok(delivered) => signalled |= delivered,
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }

    if signalled {
        Ok(true)
    } else {
        first_error.map_or(Ok(false), Err)
    }
}

/// Send SIGINT to a specific process group ID (best-effort).
pub fn interrupt_process_group(process_group_id: u32) -> io::Result<()> {
    signal_process_group_id(process_group_id as libc::pid_t, libc::SIGINT).map(|_| ())
}

/// Kill a specific process group ID (best-effort).
pub fn kill_process_group(process_group_id: u32) -> io::Result<()> {
    signal_process_group_id(process_group_id as libc::pid_t, libc::SIGKILL).map(|_| ())
}

#[cfg(target_os = "macos")]
/// Retry a denied SIGTERM against the exact group's individual members.
pub fn terminate_process_group_with_member_fallback(process_group_id: u32) -> io::Result<bool> {
    signal_process_group_with_member_fallback(
        process_group_id,
        libc::SIGTERM,
        signal_process_group_id,
        signal_process_id,
    )
}

#[cfg(target_os = "macos")]
/// Retry a denied SIGKILL against the exact group's individual members.
pub fn kill_process_group_with_member_fallback(process_group_id: u32) -> io::Result<()> {
    signal_process_group_with_member_fallback(
        process_group_id,
        libc::SIGKILL,
        signal_process_group_id,
        signal_process_id,
    )
    .map(|_| ())
}

#[cfg(all(test, target_os = "macos"))]
#[path = "process_group_tests.rs"]
mod tests;
