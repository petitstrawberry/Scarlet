//! Scarlet Native socket compatibility facade.

pub use scarlet_os::socket::*;

impl crate::io::Write for Socket {
    fn write(&mut self, buf: &[u8]) -> crate::io::Result<usize> {
        let stream = self.as_stream().map_err(|_| {
            crate::io::Error::new(crate::io::ErrorKind::Other, "Failed to get stream")
        })?;
        stream.write(buf).map_err(|error| {
            use crate::handle::capability::StreamError;
            match error {
                StreamError::Interrupted => crate::io::Error::new(
                    crate::io::ErrorKind::Interrupted,
                    "Operation interrupted",
                ),
                StreamError::WouldBlock => {
                    crate::io::Error::new(crate::io::ErrorKind::WouldBlock, "Would block")
                }
                StreamError::EndOfStream => {
                    crate::io::Error::new(crate::io::ErrorKind::UnexpectedEof, "End of stream")
                }
                StreamError::PermissionDenied => crate::io::Error::new(
                    crate::io::ErrorKind::PermissionDenied,
                    "Permission denied",
                ),
                StreamError::InvalidParameter => {
                    crate::io::Error::new(crate::io::ErrorKind::InvalidInput, "Invalid parameter")
                }
                StreamError::Unsupported => {
                    crate::io::Error::new(crate::io::ErrorKind::Unsupported, "Unsupported")
                }
                _ => crate::io::Error::new(crate::io::ErrorKind::Other, "Failed to write"),
            }
        })
    }

    fn flush(&mut self) -> crate::io::Result<()> {
        Ok(())
    }
}

impl crate::io::Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> crate::io::Result<usize> {
        let stream = self.as_stream().map_err(|_| {
            crate::io::Error::new(crate::io::ErrorKind::Other, "Failed to get stream")
        })?;
        stream.read(buf).map_err(|error| {
            use crate::handle::capability::StreamError;
            match error {
                StreamError::Interrupted => crate::io::Error::new(
                    crate::io::ErrorKind::Interrupted,
                    "Operation interrupted",
                ),
                StreamError::WouldBlock => {
                    crate::io::Error::new(crate::io::ErrorKind::WouldBlock, "Would block")
                }
                StreamError::EndOfStream => {
                    crate::io::Error::new(crate::io::ErrorKind::UnexpectedEof, "End of stream")
                }
                _ => crate::io::Error::new(crate::io::ErrorKind::Other, "Failed to read"),
            }
        })
    }
}
