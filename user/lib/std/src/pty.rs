//! Pseudo-terminal helpers for Scarlet native user space.

use crate::{
    format,
    fs::{File, OpenOptions},
    io::{Error, ErrorKind, Read, Result, Write},
    string::String,
    tty::{Terminal, WindowSize},
};

const O_RDWR: usize = 0x2;
const O_NOCTTY: usize = 0x100;

const SCTL_PTY_GET_NUMBER: u32 = 0x5350_0001;
const SCTL_PTY_SET_LOCKED: u32 = 0x5350_0002;
const SCTL_PTY_GET_LOCKED: u32 = 0x5350_0003;

/// PTY master endpoint.
pub struct PtyMaster {
    file: File,
}

impl PtyMaster {
    /// Open `/dev/ptmx` and return a PTY master.
    ///
    /// # Returns
    ///
    /// PTY master endpoint on success.
    pub fn open() -> Result<Self> {
        Ok(Self {
            file: File::open_with_flags("/dev/ptmx", O_RDWR | O_NOCTTY)?,
        })
    }

    /// Unlock the slave endpoint.
    pub fn unlock_slave(&self) -> Result<()> {
        self.control(SCTL_PTY_SET_LOCKED, 0).map(|_| ())
    }

    /// Lock the slave endpoint.
    pub fn lock_slave(&self) -> Result<()> {
        self.control(SCTL_PTY_SET_LOCKED, 1).map(|_| ())
    }

    /// Return whether the slave endpoint is locked.
    ///
    /// # Returns
    ///
    /// `true` when the slave is locked.
    pub fn is_slave_locked(&self) -> Result<bool> {
        self.control(SCTL_PTY_GET_LOCKED, 0)
            .map(|locked| locked != 0)
    }

    /// Return the slave path for this PTY master.
    ///
    /// # Returns
    ///
    /// Path such as `/dev/pts/0`.
    pub fn slave_path(&self) -> Result<String> {
        let number = self.control(SCTL_PTY_GET_NUMBER, 0)?;
        Ok(format!("/dev/pts/{}", number))
    }

    /// Open the connected slave endpoint.
    ///
    /// The slave must be unlocked first.
    ///
    /// # Returns
    ///
    /// PTY slave endpoint on success.
    pub fn open_slave(&self) -> Result<PtySlave> {
        let path = self.slave_path()?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.as_str())?;
        Ok(PtySlave { file })
    }

    /// Set the terminal window size for the connected slave TTY.
    ///
    /// # Arguments
    ///
    /// * `cols` - Terminal columns.
    /// * `rows` - Terminal rows.
    pub fn set_winsize(&self, cols: u16, rows: u16) -> Result<()> {
        Terminal::from_file(&self.file).set_winsize(WindowSize::new(cols, rows))
    }

    /// Return the terminal window size of the connected slave TTY.
    ///
    /// # Returns
    ///
    /// `(columns, rows)` for the connected slave TTY.
    pub fn winsize(&self) -> Result<(u16, u16)> {
        let size = Terminal::from_file(&self.file).winsize()?;
        Ok((size.columns, size.rows))
    }

    /// Borrow the underlying file.
    pub fn as_file(&self) -> &File {
        &self.file
    }

    /// Mutably borrow the underlying file.
    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Consume the endpoint and return the underlying file.
    pub fn into_file(self) -> File {
        self.file
    }

    fn control(&self, command: u32, arg: usize) -> Result<i32> {
        self.file
            .as_handle()
            .control(command, arg)
            .map_err(|_| Error::new(ErrorKind::Other, "PTY control failed"))
    }
}

impl Read for PtyMaster {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.file.read(buf)
    }
}

impl Write for PtyMaster {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.file.flush()
    }
}

/// PTY slave endpoint.
pub struct PtySlave {
    file: File,
}

impl PtySlave {
    /// Borrow the underlying file.
    pub fn as_file(&self) -> &File {
        &self.file
    }

    /// Mutably borrow the underlying file.
    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Consume the endpoint and return the underlying file.
    pub fn into_file(self) -> File {
        self.file
    }
}

impl Read for PtySlave {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.file.read(buf)
    }
}

impl Write for PtySlave {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.file.flush()
    }
}

/// Open PTY master/slave endpoints together.
pub struct PtyPair {
    /// Master endpoint.
    pub master: PtyMaster,
    /// Slave endpoint.
    pub slave: PtySlave,
    /// Slave path, such as `/dev/pts/0`.
    pub slave_path: String,
}

impl PtyPair {
    /// Open a PTY master, unlock its slave, and open the slave.
    ///
    /// # Returns
    ///
    /// PTY pair on success.
    pub fn open() -> Result<Self> {
        let master = PtyMaster::open()?;
        master.unlock_slave()?;
        let slave_path = master.slave_path()?;
        let slave = master.open_slave()?;
        Ok(Self {
            master,
            slave,
            slave_path,
        })
    }
}
