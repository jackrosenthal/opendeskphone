//! Thin wrappers over the Linux system calls the init role and the system
//! commands need.  Kept free of any other dependency than `libc`.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

pub fn cstring(s: impl AsRef<[u8]>) -> io::Result<CString> {
    CString::new(s.as_ref()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in string"))
}

pub fn path_cstring(p: &Path) -> io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    cstring(p.as_os_str().as_bytes())
}

fn check(ret: libc::c_long) -> io::Result<libc::c_long> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

/// `mount(2)`.
pub fn mount(source: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
    let source = cstring(source)?;
    let target = cstring(target)?;
    let fstype = cstring(fstype)?;
    let data_c = cstring(data)?;
    let data_ptr = if data.is_empty() { std::ptr::null() } else { data_c.as_ptr() as *const libc::c_void };
    let fstype_ptr = if fstype.as_bytes().is_empty() { std::ptr::null() } else { fstype.as_ptr() };
    // SAFETY: all pointers are valid NUL-terminated strings or null.
    check(unsafe { libc::mount(source.as_ptr(), target.as_ptr(), fstype_ptr, flags, data_ptr) } as libc::c_long)?;
    Ok(())
}

/// `umount2(2)`.
pub fn umount(target: &str, flags: libc::c_int) -> io::Result<()> {
    let target = cstring(target)?;
    // SAFETY: valid C string.
    check(unsafe { libc::umount2(target.as_ptr(), flags) } as libc::c_long)?;
    Ok(())
}

/// `finit_module(2)`: load a module from an open file.
pub fn finit_module(fd: RawFd, params: &str, flags: libc::c_int) -> io::Result<()> {
    let params = cstring(params)?;
    // SAFETY: fd is a caller-owned descriptor; params is a valid C string.
    check(unsafe { libc::syscall(libc::SYS_finit_module, fd, params.as_ptr(), flags) })?;
    Ok(())
}

/// Kernel-side decompression flag for `finit_module`.
pub const MODULE_INIT_COMPRESSED_FILE: libc::c_int = 4;

/// `delete_module(2)`.
pub fn delete_module(name: &str, flags: libc::c_int) -> io::Result<()> {
    let name = cstring(name)?;
    // SAFETY: valid C string.
    check(unsafe { libc::syscall(libc::SYS_delete_module, name.as_ptr(), flags) })?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    Reboot,
    Halt,
    PowerOff,
    /// Jump into the kernel image loaded with [`kexec_load`] (`dphone
    /// ramboot`).  Only the init role asks for this, on [`kexec_signal`].
    Kexec,
}

/// `reboot(2)`.  Does not sync; the caller does.
pub fn reboot(kind: Shutdown) -> io::Error {
    let cmd = match kind {
        Shutdown::Reboot => libc::RB_AUTOBOOT,
        Shutdown::Halt => libc::RB_HALT_SYSTEM,
        Shutdown::PowerOff => libc::RB_POWER_OFF,
        Shutdown::Kexec => libc::RB_KEXEC,
    };
    // SAFETY: plain system call.
    unsafe { libc::reboot(cmd) };
    io::Error::last_os_error()
}

/// Reads "1" while a kernel image is loaded for [`Shutdown::Kexec`];
/// absent when the kernel has no kexec.
pub const KEXEC_LOADED: &str = "/sys/kernel/kexec_loaded";

/// One piece of a kernel image for [`kexec_load`]: `bufsz` bytes at `buf`
/// go to physical address `mem`; the rest of `memsz` is cleared.  `mem`
/// and `memsz` are multiples of the page size.  (`struct kexec_segment`.)
#[repr(C)]
pub struct KexecSegment {
    pub buf: *const libc::c_void,
    pub bufsz: usize,
    pub mem: usize,
    pub memsz: usize,
}

/// `kexec_load(2)`: hand the kernel an image to start at physical address
/// `entry` on [`Shutdown::Kexec`].  The kernel copies the data during the
/// call.  Nothing else changes until that reboot call.
///
/// # Safety
///
/// Every segment's `buf` must be readable for `bufsz` bytes.
pub unsafe fn kexec_load(entry: usize, segments: &[KexecSegment]) -> io::Result<()> {
    // KEXEC_ARCH_ARM: EM_ARM in the upper half of the flags.
    const KEXEC_ARCH_ARM: libc::c_ulong = 40 << 16;
    // SAFETY: the segment array is valid for its length; the caller
    // vouches for the buffers.
    check(unsafe { libc::syscall(libc::SYS_kexec_load, entry, segments.len(), segments.as_ptr(), KEXEC_ARCH_ARM) })?;
    Ok(())
}

/// Drop a loaded kernel image.
pub fn kexec_unload() {
    // SAFETY: no segments, no pointers.
    unsafe { libc::syscall(libc::SYS_kexec_load, 0usize, 0usize, std::ptr::null::<KexecSegment>(), 0 as libc::c_ulong) };
}

/// The signal that asks PID 1 for [`Shutdown::Kexec`]: `SIGRTMIN+6`, the
/// one systemd takes for the same request.
pub fn kexec_signal() -> libc::c_int {
    libc::SIGRTMIN() + 6
}

pub fn sync() {
    // SAFETY: plain system call.
    unsafe { libc::sync() };
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

/// Set the wall clock.  Uses the 64-bit time system call directly on 32-bit
/// targets so the C library's `time_t` width does not matter.
pub fn set_clock(secs: i64) -> io::Result<()> {
    #[cfg(target_pointer_width = "32")]
    {
        // struct __kernel_timespec { s64 tv_sec; s64 tv_nsec; }
        let ts: [i64; 2] = [secs, 0];
        const SYS_CLOCK_SETTIME64: libc::c_long = 404;
        // SAFETY: ts outlives the call and has the kernel's layout.
        check(unsafe { libc::syscall(SYS_CLOCK_SETTIME64, libc::CLOCK_REALTIME, ts.as_ptr()) })?;
        Ok(())
    }
    #[cfg(not(target_pointer_width = "32"))]
    {
        let ts = libc::timespec { tv_sec: secs as libc::time_t, tv_nsec: 0 };
        // SAFETY: ts is a valid timespec.
        check(unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } as libc::c_long)?;
        Ok(())
    }
}

/// Monotonic seconds, unaffected by wall-clock changes.
pub fn monotonic_secs() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs()
}

pub fn kill(pid: libc::pid_t, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain system call.
    check(unsafe { libc::kill(pid, sig) } as libc::c_long)?;
    Ok(())
}

pub fn getpid() -> libc::pid_t {
    // SAFETY: plain system call.
    unsafe { libc::getpid() }
}

/// `ioctl` with an integer argument passed by pointer.
pub fn ioctl_int(fd: &impl AsRawFd, request: libc::c_ulong, value: &mut libc::c_int) -> io::Result<()> {
    // SAFETY: value points to a live c_int.
    check(unsafe { libc::ioctl(fd.as_raw_fd(), request as _, value as *mut libc::c_int) } as libc::c_long)?;
    Ok(())
}

/// Set or clear `O_NONBLOCK`.
pub fn set_nonblocking(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: fcntl on a caller-owned descriptor.
    let flags = check(unsafe { libc::fcntl(fd, libc::F_GETFL) } as libc::c_long)? as libc::c_int;
    let flags = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    // SAFETY: as above.
    check(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } as libc::c_long)?;
    Ok(())
}

/// Read the kernel log buffer (`klogctl` read-all, optionally clearing).
pub fn read_kernel_log(clear: bool) -> io::Result<Vec<u8>> {
    const SYSLOG_ACTION_READ_ALL: libc::c_int = 3;
    const SYSLOG_ACTION_READ_CLEAR: libc::c_int = 4;
    const SYSLOG_ACTION_SIZE_BUFFER: libc::c_int = 10;
    // SAFETY: size query takes no buffer.
    let size = check(unsafe { libc::klogctl(SYSLOG_ACTION_SIZE_BUFFER, std::ptr::null_mut(), 0) } as libc::c_long)?;
    let mut buf = vec![0u8; size.max(4096) as usize];
    let action = if clear { SYSLOG_ACTION_READ_CLEAR } else { SYSLOG_ACTION_READ_ALL };
    // SAFETY: buf is writable for its whole length.
    let n = check(unsafe { libc::klogctl(action, buf.as_mut_ptr() as *mut libc::c_char, buf.len() as libc::c_int) }
        as libc::c_long)?;
    buf.truncate(n as usize);
    Ok(buf)
}

/// Kernel release string (`uname -r`).
pub fn kernel_release() -> String {
    // SAFETY: utsname is plain data; uname fills it.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return String::new();
        }
        CStr::from_ptr(u.release.as_ptr()).to_string_lossy().into_owned()
    }
}

/// Mount flag names accepted in `-o` lists.  Unknown options are passed to
/// the filesystem as data.
pub fn parse_mount_options(opts: &str) -> (libc::c_ulong, String) {
    let mut flags: libc::c_ulong = 0;
    let mut data: Vec<&str> = Vec::new();
    for opt in opts.split(',').filter(|o| !o.is_empty()) {
        match opt {
            "ro" => flags |= libc::MS_RDONLY,
            "rw" => flags &= !libc::MS_RDONLY,
            "nosuid" => flags |= libc::MS_NOSUID,
            "nodev" => flags |= libc::MS_NODEV,
            "noexec" => flags |= libc::MS_NOEXEC,
            "sync" => flags |= libc::MS_SYNCHRONOUS,
            "remount" => flags |= libc::MS_REMOUNT,
            "bind" => flags |= libc::MS_BIND,
            "noatime" => flags |= libc::MS_NOATIME,
            "nodiratime" => flags |= libc::MS_NODIRATIME,
            "relatime" => flags |= libc::MS_RELATIME,
            "defaults" => {}
            other => data.push(other),
        }
    }
    (flags, data.join(","))
}

/// Where the kernel takes whether to write its messages to the console
/// UART (the `console_output` switch of the PL011 driver, kernel patch
/// 0019).  The kernel starts with it off.
const CONSOLE_OUTPUT: &str = "/sys/module/amba_pl011/parameters/console_output";

/// Turn kernel messages on the console UART on or off.  Off while the EHS
/// headset adapter has the UART.  The other kernel consoles (the pstore
/// crash log) are not affected.
///
/// The D80's kernel has no such switch and needs none: its console UART is
/// the console's alone, and the kernel always writes to it.
pub fn serial_console_output(on: bool) {
    if !std::path::Path::new(CONSOLE_OUTPUT).exists() {
        return;
    }
    if let Err(e) = std::fs::write(CONSOLE_OUTPUT, if on { "1" } else { "0" }) {
        log::warn!("{CONSOLE_OUTPUT}: {e}");
    }
}

/// Point this process's standard output and error at the console UART
/// (`on`), or at nothing.  The main role starts with nothing, so that its
/// log copies and a panic message never reach an EHS adapter.
pub fn stdio_to_console(on: bool) {
    let path = if on { c"/dev/console" } else { c"/dev/null" };
    // SAFETY: plain descriptor calls on a path literal.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NOCTTY | libc::O_CLOEXEC);
        if fd < 0 {
            log::warn!("{}: {}", path.to_string_lossy(), std::io::Error::last_os_error());
            return;
        }
        libc::dup2(fd, 1);
        libc::dup2(fd, 2);
        if fd > 2 {
            libc::close(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_options() {
        let (flags, data) = parse_mount_options("ro,noatime,mode=755,size=4m");
        assert_eq!(flags, libc::MS_RDONLY | libc::MS_NOATIME);
        assert_eq!(data, "mode=755,size=4m");
        let (flags, data) = parse_mount_options("ro,rw,defaults");
        assert_eq!(flags, 0);
        assert_eq!(data, "");
    }
}
