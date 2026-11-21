//! Watchdog, hardware RNG, and the EHS serial port.

use super::sys::*;
use crate::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

pub(crate) const WDIOC_KEEPALIVE: u32 = ior::<i32>(b'W', 5);
pub(crate) const WDIOC_SETTIMEOUT: u32 = iowr::<i32>(b'W', 6);
pub(crate) const WDIOC_GETTIMELEFT: u32 = ior::<i32>(b'W', 10);

/// `/dev/watchdog` (SP805).  Opening the device arms it, so the file is
/// opened on [`Watchdog::start`], not at construction.
#[derive(Default)]
pub struct PhoneWatchdog {
    file: Option<File>,
}

impl PhoneWatchdog {
    pub fn new() -> PhoneWatchdog {
        PhoneWatchdog { file: None }
    }

    /// Adopts an already open watchdog descriptor (the init role opens the
    /// device early and may hand it over).
    pub fn from_file(file: File) -> PhoneWatchdog {
        PhoneWatchdog { file: Some(file) }
    }

    fn fd(&self) -> Result<i32> {
        self.file
            .as_ref()
            .map(|f| f.as_raw_fd())
            .ok_or_else(|| HalError::Invalid("watchdog not started".into()))
    }
}

impl Watchdog for PhoneWatchdog {
    fn start(&mut self, timeout_secs: u32) -> Result<u32> {
        if self.file.is_none() {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/watchdog")
                .map_err(|e| HalError::io("/dev/watchdog", e))?;
            self.file = Some(f);
        }
        let mut t = timeout_secs as i32;
        ioctl(self.fd()?, WDIOC_SETTIMEOUT, &mut t).map_err(|e| HalError::io("WDIOC_SETTIMEOUT", e))?;
        self.feed()?;
        Ok(t as u32)
    }

    fn feed(&mut self) -> Result<()> {
        let mut dummy = 0i32;
        ioctl(self.fd()?, WDIOC_KEEPALIVE, &mut dummy).map_err(|e| HalError::io("WDIOC_KEEPALIVE", e))?;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut f) = self.file.take() {
            // Magic close.
            f.write_all(b"V").map_err(|e| HalError::io("watchdog magic close", e))?;
        }
        Ok(())
    }

    fn time_left(&mut self) -> Result<u32> {
        let mut t = 0i32;
        ioctl(self.fd()?, WDIOC_GETTIMELEFT, &mut t).map_err(|e| HalError::io("WDIOC_GETTIMELEFT", e))?;
        Ok(t.max(0) as u32)
    }
}

/// `/dev/hwrng`, falling back to the kernel pool (`/dev/urandom`).
pub struct PhoneRng {
    file: File,
}

impl PhoneRng {
    pub fn open() -> Result<PhoneRng> {
        let file = File::open("/dev/hwrng")
            .or_else(|_| File::open("/dev/urandom"))
            .map_err(|e| HalError::io("/dev/hwrng and /dev/urandom", e))?;
        Ok(PhoneRng { file })
    }
}

impl Rng for PhoneRng {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        self.file.read_exact(buf).map_err(|e| HalError::io("read random", e))
    }
}

/// The EHS UART.
pub struct PhoneEhs {
    path: &'static str,
}

impl PhoneEhs {
    /// The port of the D40 thru D70: UART A, which the serial console
    /// shares.
    pub fn new() -> PhoneEhs {
        PhoneEhs { path: "/dev/ttyAMA0" }
    }

    /// The port of `model`.  The D80's is UART2, which is the EHS jack's
    /// alone (its console is UART1).
    pub fn for_model(model: Model) -> PhoneEhs {
        if model.is_touch() { PhoneEhs { path: "/dev/ttymxc1" } } else { PhoneEhs::new() }
    }
}

impl Default for PhoneEhs {
    fn default() -> Self {
        Self::new()
    }
}

fn baud_const(baud: u32) -> Result<libc::speed_t> {
    Ok(match baud {
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        _ => return Err(HalError::Invalid(format!("unsupported baud rate {baud}"))),
    })
}

struct TtyPort {
    file: File,
    saved: libc::termios,
}

fn apply(fd: i32, config: SerialConfig) -> Result<()> {
    // SAFETY: termios calls on an open descriptor with a valid struct.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) < 0 {
            return Err(HalError::last_os("tcgetattr"));
        }
        libc::cfmakeraw(&mut t);
        let speed = baud_const(config.baud)?;
        libc::cfsetispeed(&mut t, speed);
        libc::cfsetospeed(&mut t, speed);
        t.c_cflag &= !(libc::CSIZE | libc::CSTOPB | libc::PARENB | libc::PARODD | libc::CRTSCTS);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        t.c_cflag |= if config.data_bits == 7 { libc::CS7 } else { libc::CS8 };
        if config.stop_bits == 2 {
            t.c_cflag |= libc::CSTOPB;
        }
        match config.parity {
            Parity::None => {}
            Parity::Even => t.c_cflag |= libc::PARENB,
            Parity::Odd => t.c_cflag |= libc::PARENB | libc::PARODD,
        }
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        // As the stock daemon does: apply after output drains, discarding
        // pending input.
        if libc::tcsetattr(fd, libc::TCSAFLUSH, &t) < 0 {
            return Err(HalError::last_os("tcsetattr"));
        }
    }
    Ok(())
}

impl Ehs for PhoneEhs {
    fn open(&mut self, config: SerialConfig) -> Result<Box<dyn EhsPort>> {
        let file = open_rw(Path::new(self.path), libc::O_NOCTTY | libc::O_NONBLOCK)
            .map_err(|e| HalError::io(self.path, e))?;
        // SAFETY: valid descriptor and struct.
        let saved = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(file.as_raw_fd(), &mut t) < 0 {
                return Err(HalError::last_os("tcgetattr"));
            }
            t
        };
        apply(file.as_raw_fd(), config)?;
        Ok(Box::new(TtyPort { file, saved }))
    }
}

impl EhsPort for TtyPort {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let ready = poll_readable(&[self.file.as_raw_fd()], ms).map_err(|e| HalError::io("poll tty", e))?;
        if !ready[0] {
            return Ok(0);
        }
        match self.file.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(HalError::io("read tty", e)),
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        let mut done = 0;
        while done < data.len() {
            match self.file.write(&data[done..]) {
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(HalError::io("write tty", e)),
            }
        }
        Ok(())
    }

    fn configure(&mut self, config: SerialConfig) -> Result<()> {
        apply(self.file.as_raw_fd(), config)
    }

    fn set_lines(&mut self, dtr: bool, rts: bool) -> Result<()> {
        for (bit, on) in [(libc::TIOCM_DTR, dtr), (libc::TIOCM_RTS, rts)] {
            let mut v: libc::c_int = bit;
            let req = if on { libc::TIOCMBIS } else { libc::TIOCMBIC };
            ioctl(self.file.as_raw_fd(), req as u32, &mut v).map_err(|e| HalError::io("TIOCMBIS/BIC", e))?;
        }
        Ok(())
    }
}

impl Drop for TtyPort {
    fn drop(&mut self) {
        // SAFETY: restoring the settings we saved, on our own descriptor.
        unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.saved) };
    }
}
