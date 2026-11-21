//! Bluetooth sockets: thin wrappers over the kernel's `AF_BLUETOOTH`
//! families.  The address layouts are those of Linux 7.3
//! (`include/net/bluetooth/`).  Every socket is non-blocking and
//! close-on-exec.

use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::Addr;

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_L2CAP: libc::c_int = 0;
const BTPROTO_HCI: libc::c_int = 1;
const BTPROTO_SCO: libc::c_int = 2;
const BTPROTO_RFCOMM: libc::c_int = 3;

const HCI_CHANNEL_RAW: u16 = 0;
const HCI_CHANNEL_CONTROL: u16 = 3;
const HCI_DEV_NONE: u16 = 0xffff;

const SOL_BLUETOOTH: libc::c_int = 274;
const BT_SECURITY: libc::c_int = 4;
const BT_SECURITY_MEDIUM: u8 = 2;

#[repr(C)]
struct SockaddrHci {
    family: libc::sa_family_t,
    dev: u16,
    channel: u16,
}

#[repr(C)]
struct SockaddrL2 {
    family: libc::sa_family_t,
    psm: u16, // little-endian
    bdaddr: [u8; 6],
    cid: u16,
    bdaddr_type: u8,
}

#[repr(C)]
struct SockaddrRc {
    family: libc::sa_family_t,
    bdaddr: [u8; 6],
    channel: u8,
}

#[repr(C)]
struct SockaddrSco {
    family: libc::sa_family_t,
    bdaddr: [u8; 6],
}

fn cvt(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

fn socket(kind: libc::c_int, proto: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: plain socket(2); the result is checked before it is owned.
    let fd = cvt(unsafe { libc::socket(AF_BLUETOOTH, kind | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, proto) })?;
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn bind<T>(fd: &OwnedFd, addr: &T) -> io::Result<()> {
    // SAFETY: `addr` is a live sockaddr structure of the stated size.
    cvt(unsafe { libc::bind(fd.as_raw_fd(), (addr as *const T).cast(), size_of::<T>() as libc::socklen_t) })?;
    Ok(())
}

/// Start a connection.  "In progress" is success: the socket becomes
/// writable when the connection is up or has failed (see [`take_error`]).
fn connect<T>(fd: &OwnedFd, addr: &T) -> io::Result<()> {
    // SAFETY: as in `bind`.
    let r = unsafe { libc::connect(fd.as_raw_fd(), (addr as *const T).cast(), size_of::<T>() as libc::socklen_t) };
    if r < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
    }
    Ok(())
}

fn listen(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: plain listen(2).
    cvt(unsafe { libc::listen(fd.as_raw_fd(), 2) })?;
    Ok(())
}

/// Require an authenticated, encrypted link (pairing) on this socket.
fn require_pairing(fd: &OwnedFd) -> io::Result<()> {
    let sec: [u8; 2] = [BT_SECURITY_MEDIUM, 0];
    // SAFETY: `struct bt_security` is two bytes: level and key size.
    cvt(unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_SECURITY, sec.as_ptr().cast(), 2) })?;
    Ok(())
}

/// The pending error of a socket whose connect finished.
pub fn take_error(fd: RawFd) -> io::Result<()> {
    let mut err: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `err` and `len` are valid for the call.
    cvt(unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, (&mut err as *mut libc::c_int).cast(), &mut len) })?;
    if err == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(err)) }
}

/// Read once; `Ok(None)` when nothing is waiting, `Ok(Some(0))` at end of
/// stream.
pub fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<Option<usize>> {
    // SAFETY: `buf` is valid for `buf.len()` bytes.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(None),
            _ => Err(e),
        };
    }
    Ok(Some(n as usize))
}

/// Write all of `data`.  The sockets here carry a few short packets, so a
/// full buffer is treated as an error instead of being queued.
pub fn write(fd: RawFd, data: &[u8]) -> io::Result<()> {
    let mut data = data;
    while !data.is_empty() {
        // SAFETY: `data` is valid for `data.len()` bytes.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// The management interface (all controllers).
pub fn mgmt() -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_RAW, BTPROTO_HCI)?;
    bind(&fd, &SockaddrHci { family: AF_BLUETOOTH as _, dev: HCI_DEV_NONE, channel: HCI_CHANNEL_CONTROL })?;
    Ok(fd)
}

/// Send one HCI command to controller `index` through a raw HCI socket and
/// close it.  The reply is not read; the controller's command queue in the
/// kernel is bypassed, which is acceptable for a one-off vendor command sent
/// while the controller is otherwise idle.
pub fn send_hci_command(index: u16, opcode: u16, params: &[u8]) -> io::Result<()> {
    let fd = socket(libc::SOCK_RAW, BTPROTO_HCI)?;
    bind(&fd, &SockaddrHci { family: AF_BLUETOOTH as _, dev: index, channel: HCI_CHANNEL_RAW })?;
    let mut packet = vec![0x01]; // HCI command packet
    packet.extend_from_slice(&opcode.to_le_bytes());
    packet.push(params.len() as u8);
    packet.extend_from_slice(params);
    write(fd.as_raw_fd(), &packet)
}

fn l2(addr: Addr, psm: u16) -> SockaddrL2 {
    SockaddrL2 { family: AF_BLUETOOTH as _, psm: psm.to_le(), bdaddr: addr.0, cid: 0, bdaddr_type: 0 }
}

/// Listen for L2CAP connections on `psm` (service discovery: no pairing).
pub fn l2cap_listen(psm: u16) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_SEQPACKET, BTPROTO_L2CAP)?;
    bind(&fd, &l2(Addr::ANY, psm))?;
    listen(&fd)?;
    Ok(fd)
}

pub fn l2cap_connect(addr: Addr, psm: u16) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_SEQPACKET, BTPROTO_L2CAP)?;
    connect(&fd, &l2(addr, psm))?;
    Ok(fd)
}

/// Listen for RFCOMM connections on `channel`, from paired devices only.
pub fn rfcomm_listen(channel: u8) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_STREAM, BTPROTO_RFCOMM)?;
    require_pairing(&fd)?;
    bind(&fd, &SockaddrRc { family: AF_BLUETOOTH as _, bdaddr: [0; 6], channel })?;
    listen(&fd)?;
    Ok(fd)
}

pub fn rfcomm_connect(addr: Addr, channel: u8) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_STREAM, BTPROTO_RFCOMM)?;
    require_pairing(&fd)?;
    connect(&fd, &SockaddrRc { family: AF_BLUETOOTH as _, bdaddr: addr.0, channel })?;
    Ok(fd)
}

/// Listen for voice (SCO) connections.
pub fn sco_listen() -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_SEQPACKET, BTPROTO_SCO)?;
    bind(&fd, &SockaddrSco { family: AF_BLUETOOTH as _, bdaddr: [0; 6] })?;
    listen(&fd)?;
    Ok(fd)
}

/// The socket option that sets a voice connection's air coding
/// (`BT_VOICE`), and its value for data the controller passes as it is
/// (`BT_VOICE_TRANSPARENT`): what wide-band speech takes.
const BT_VOICE: libc::c_int = 11;
const BT_VOICE_TRANSPARENT: u16 = 0x0003;

/// Open a voice (SCO) connection to a device that already has a link.
/// `transparent` asks for the connection wide-band speech uses, whose air
/// coding is the codec's own and not the controller's CVSD.
pub fn sco_connect(addr: Addr, transparent: bool) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_SEQPACKET, BTPROTO_SCO)?;
    if transparent {
        // SAFETY: `struct bt_voice` is one 16-bit setting.
        cvt(unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_VOICE, (&BT_VOICE_TRANSPARENT as *const u16).cast(), 2) })?;
    }
    connect(&fd, &SockaddrSco { family: AF_BLUETOOTH as _, bdaddr: addr.0 })?;
    Ok(fd)
}

/// Accept one connection; `Ok(None)` when none is waiting.  Returns the
/// peer's address, read from the front of its socket address (L2CAP has the
/// PSM first, so its offset differs).
pub fn accept(listener: RawFd, l2cap: bool) -> io::Result<Option<(OwnedFd, Addr)>> {
    let mut storage = [0u8; 32];
    let mut len = storage.len() as libc::socklen_t;
    // SAFETY: `storage` is valid for `len` bytes.
    let fd = unsafe {
        libc::accept4(listener, storage.as_mut_ptr().cast(), &mut len, libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK)
    };
    if fd < 0 {
        let e = io::Error::last_os_error();
        return match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(None),
            _ => Err(e),
        };
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let at = if l2cap { 4 } else { 2 };
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&storage[at..at + 6]);
    Ok(Some((fd, Addr(addr))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel rejects a socket address of the wrong size, so the
    /// structures must have the kernel's layout.
    #[test]
    fn socket_address_layouts() {
        assert_eq!(size_of::<SockaddrHci>(), 6);
        assert_eq!(size_of::<SockaddrL2>(), 14);
        assert_eq!(size_of::<SockaddrRc>(), 10);
        assert_eq!(size_of::<SockaddrSco>(), 8);
        assert_eq!(std::mem::offset_of!(SockaddrL2, bdaddr), 4);
        assert_eq!(std::mem::offset_of!(SockaddrRc, bdaddr), 2);
        assert_eq!(std::mem::offset_of!(SockaddrRc, channel), 8);
    }

    /// Against a running kernel with Bluetooth support (any controller or
    /// none): the management socket opens, takes a command, and answers in
    /// the layout `mgmt` parses.  Run with `--ignored`.
    #[test]
    #[ignore = "needs a kernel with Bluetooth support"]
    fn management_socket_answers() {
        use crate::mgmt::{self, MgmtEvent, op};

        let fd = mgmt().expect("management socket");
        write(fd.as_raw_fd(), &mgmt::command(op::READ_INDEX_LIST, mgmt::INDEX_NONE, &[])).unwrap();
        let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        assert_eq!(unsafe { libc::poll(&mut pfd, 1, 2000) }, 1);
        let mut buf = [0u8; 512];
        let n = read(fd.as_raw_fd(), &mut buf).unwrap().unwrap();
        let (index, event) = mgmt::parse_event(&buf[..n]).unwrap();
        assert_eq!(index, mgmt::INDEX_NONE);
        let MgmtEvent::Complete { opcode, status, params } = event else { panic!("{event:?}") };
        assert_eq!((opcode, status), (op::READ_INDEX_LIST, 0));
        let list = mgmt::parse_index_list(&params).unwrap();
        println!("controllers: {list:?}");
    }
}
