//! RFB 3.3/3.7/3.8 version negotiation and None/VNC authentication.

use des::{
    Des,
    cipher::{BlockEncrypt, KeyInit},
};
use std::io::{self, ErrorKind, Read, Write};
use subtle::ConstantTimeEq;

const PROTOCOL_VERSION: &[u8; 12] = b"RFB 003.008\n";
const SERVER_NAME: &[u8] = b"Scarlet Remote Desktop";

/// Classic VNC authentication key, with each password byte's bits reversed.
pub(crate) struct VncPassword {
    key: [u8; 8],
}

impl VncPassword {
    /// Load 1-8 printable ASCII password bytes, allowing a trailing LF or CRLF.
    pub(crate) fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let bytes = bytes
            .strip_suffix(b"\r\n")
            .or_else(|| bytes.strip_suffix(b"\n"))
            .unwrap_or(bytes);
        if bytes.is_empty()
            || bytes.len() > 8
            || !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte))
        {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "VNC password must contain 1-8 printable ASCII bytes",
            ));
        }
        let mut key = [0; 8];
        for (key_byte, byte) in key.iter_mut().zip(bytes) {
            *key_byte = byte.reverse_bits();
        }
        Ok(Self { key })
    }

    fn response(&self, mut challenge: [u8; 16]) -> [u8; 16] {
        let cipher = Des::new((&self.key).into());
        for block in challenge.chunks_exact_mut(8) {
            cipher.encrypt_block(block.into());
        }
        challenge
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProtocolVersion {
    V3_3,
    V3_7,
    V3_8,
}

impl ProtocolVersion {
    fn parse(bytes: &[u8; 12]) -> io::Result<Self> {
        if &bytes[..8] != b"RFB 003."
            || !bytes[8..11].iter().all(u8::is_ascii_digit)
            || bytes[11] != b'\n'
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid or unsupported RFB protocol version",
            ));
        }
        // RFC 6143 section 7.1.1: unknown 3.x versions use the 3.3 handshake.
        Ok(match &bytes[8..11] {
            b"007" => Self::V3_7,
            b"008" => Self::V3_8,
            _ => Self::V3_3,
        })
    }
}

/// Complete version negotiation, authentication, and desktop initialization.
///
/// # Arguments
///
/// * `stream` - Newly accepted RFB connection.
/// * `width` - Initial framebuffer width.
/// * `height` - Initial framebuffer height.
/// * `password` - Optional password requiring VNC Authentication instead of None.
///
/// # Returns
///
/// Success after `ServerInit` has been sent. RFB 3.3 (including macOS Screen
/// Sharing), 3.7, and 3.8 each use their own security handshake.
pub(crate) fn handshake(
    stream: &mut (impl Read + Write),
    width: u32,
    height: u32,
    password: Option<&VncPassword>,
) -> io::Result<()> {
    let width = u16::try_from(width)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "framebuffer width exceeds RFB"))?;
    let height = u16::try_from(height)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "framebuffer height exceeds RFB"))?;

    stream.write_all(PROTOCOL_VERSION)?;
    let mut client_version = [0; 12];
    stream.read_exact(&mut client_version)?;
    let version = ProtocolVersion::parse(&client_version)?;

    let security_type = if password.is_some() { 2u8 } else { 1u8 };
    if version == ProtocolVersion::V3_3 {
        // 3.3 sends a single U32 security type instead of a selection list.
        stream.write_all(&u32::from(security_type).to_be_bytes())?;
    } else {
        stream.write_all(&[1, security_type])?;
        let mut selected_security = [0; 1];
        stream.read_exact(&mut selected_security)?;
        if selected_security[0] != security_type {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "client selected an unoffered RFB security type",
            ));
        }
    }

    if let Some(password) = password {
        let mut challenge = [0; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut challenge)?;
        stream.write_all(&challenge)?;
        let mut response = [0; 16];
        stream.read_exact(&mut response)?;
        if password.response(challenge).ct_eq(&response).unwrap_u8() == 0 {
            stream.write_all(&1u32.to_be_bytes())?; // SecurityResult: failed
            if version == ProtocolVersion::V3_8 {
                let reason = b"VNC authentication failed";
                stream.write_all(&(reason.len() as u32).to_be_bytes())?;
                stream.write_all(reason)?;
            }
            stream.flush()?;
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "VNC authentication failed",
            ));
        }
    }
    // VNC Authentication always has a result; None only has one in 3.8.
    if password.is_some() || version == ProtocolVersion::V3_8 {
        stream.write_all(&0u32.to_be_bytes())?;
    }

    let mut shared_flag = [0; 1];
    stream.read_exact(&mut shared_flag)?;

    let mut server_init = Vec::with_capacity(24 + SERVER_NAME.len());
    server_init.extend_from_slice(&width.to_be_bytes());
    server_init.extend_from_slice(&height.to_be_bytes());
    // Native 32-bit little-endian BGR true color, with 24 bits of color depth.
    server_init.extend_from_slice(&[32, 24, 0, 1]);
    server_init.extend_from_slice(&255u16.to_be_bytes());
    server_init.extend_from_slice(&255u16.to_be_bytes());
    server_init.extend_from_slice(&255u16.to_be_bytes());
    server_init.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
    server_init.extend_from_slice(&(SERVER_NAME.len() as u32).to_be_bytes());
    server_init.extend_from_slice(SERVER_NAME);
    stream.write_all(&server_init)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    fn connect_with_password(
        password: Option<VncPassword>,
    ) -> (TcpStream, JoinHandle<io::Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        for stream in [&client, &server] {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        let worker = thread::spawn(move || handshake(&mut server, 640, 480, password.as_ref()));
        let mut banner = [0; 12];
        client.read_exact(&mut banner).unwrap();
        assert_eq!(&banner, b"RFB 003.008\n");
        (client, worker)
    }

    fn connect() -> (TcpStream, JoinHandle<io::Result<()>>) {
        connect_with_password(None)
    }

    fn assert_server_init(client: &mut TcpStream, worker: JoinHandle<io::Result<()>>) {
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        worker.join().unwrap().unwrap();
        // Checking the entire message catches stray/missing SecurityResult bytes.
        let mut expected = vec![
            2, 128, 1, 224, // 640 x 480
            32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0, 0, 0, 0,
            22, // desktop name length
        ];
        expected.extend_from_slice(b"Scarlet Remote Desktop");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn rfb_3_3_initializes_without_security_selection_or_result() {
        let (mut client, worker) = connect();
        client.write_all(b"RFB 003.003\n").unwrap();
        let mut security = [0; 4];
        client.read_exact(&mut security).unwrap();
        assert_eq!(security, [0, 0, 0, 1]);
        client.write_all(&[1]).unwrap(); // ClientInit, no security selection
        assert_server_init(&mut client, worker);
    }

    #[test]
    fn rfb_3_7_skips_none_security_result() {
        let (mut client, worker) = connect();
        client.write_all(b"RFB 003.007\n").unwrap();
        let mut security = [0; 2];
        client.read_exact(&mut security).unwrap();
        assert_eq!(security, [1, 1]);
        client.write_all(&[1, 0]).unwrap(); // selection, then ClientInit
        assert_server_init(&mut client, worker);
    }

    #[test]
    fn rfb_3_8_keeps_security_result_before_client_init() {
        let (mut client, worker) = connect();
        client.write_all(b"RFB 003.008\n").unwrap();
        let mut security = [0; 2];
        client.read_exact(&mut security).unwrap();
        assert_eq!(security, [1, 1]);
        client.write_all(&[1]).unwrap();
        let mut result = [255; 4];
        client.read_exact(&mut result).unwrap();
        assert_eq!(result, [0; 4]);
        client.write_all(&[1]).unwrap();
        assert_server_init(&mut client, worker);
    }

    #[test]
    fn unknown_3_x_version_uses_3_3_handshake() {
        let (mut client, worker) = connect();
        client.write_all(b"RFB 003.889\n").unwrap();
        let mut security = [0; 4];
        client.read_exact(&mut security).unwrap();
        assert_eq!(security, [0, 0, 0, 1]);
        client.write_all(&[1]).unwrap();
        assert_server_init(&mut client, worker);
    }

    #[test]
    fn rejects_malformed_and_unsupported_protocol_versions() {
        for version in [
            b"BAD 003.008\n",
            b"RFB 004.008\n",
            b"RFB 003.00x\n",
            b"RFB 003.008!",
        ] {
            let (mut client, worker) = connect();
            client.write_all(version).unwrap();
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn rejects_unoffered_security_types() {
        for version in [b"RFB 003.007\n", b"RFB 003.008\n"] {
            let (mut client, worker) = connect();
            client.write_all(version).unwrap();
            let mut security = [0; 2];
            client.read_exact(&mut security).unwrap();
            client.write_all(&[2]).unwrap();
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn rejects_dimensions_that_do_not_fit_rfb() {
        let mut stream = io::Cursor::new(Vec::new());
        for (width, height) in [(65536, 480), (640, 65536)] {
            assert_eq!(
                handshake(&mut stream, width, height, None)
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidData
            );
            assert!(stream.get_ref().is_empty());
        }
    }

    #[test]
    fn vnc_response_matches_independent_des_test_vector() {
        // DES-ECB, reversed password-byte bits, challenge bytes 0 through 15.
        let password = VncPassword::from_bytes(b"password").unwrap();
        assert_eq!(
            password.response(core::array::from_fn(|i| i as u8)),
            [
                0xb8, 0x66, 0x92, 0x41, 0x25, 0xc8, 0xee, 0xbb, 0x9d, 0xeb, 0xc1, 0xdb, 0x61, 0xc5,
                0x38, 0xe2,
            ]
        );
    }

    #[test]
    fn validates_password_files_without_silently_truncating() {
        for bytes in [b"password".as_slice(), b"password\n", b"password\r\n"] {
            assert!(VncPassword::from_bytes(bytes).is_ok());
        }
        for bytes in [
            b"".as_slice(),
            b"\n",
            b"123456789",
            b"pass\nword",
            b"pass\0",
            b"\xff",
        ] {
            assert!(VncPassword::from_bytes(bytes).is_err());
        }
    }

    fn start_vnc_auth(version: &[u8; 12]) -> (TcpStream, JoinHandle<io::Result<()>>, [u8; 16]) {
        let (mut client, worker) =
            connect_with_password(Some(VncPassword::from_bytes(b"password").unwrap()));
        client.write_all(version).unwrap();
        if version == b"RFB 003.003\n" {
            let mut security = [0; 4];
            client.read_exact(&mut security).unwrap();
            assert_eq!(security, [0, 0, 0, 2]);
        } else {
            let mut security = [0; 2];
            client.read_exact(&mut security).unwrap();
            assert_eq!(security, [1, 2]); // Never offer None with a password.
            client.write_all(&[2]).unwrap();
        }
        let mut challenge = [0; 16];
        client.read_exact(&mut challenge).unwrap();
        (client, worker, challenge)
    }

    #[test]
    fn vnc_auth_initializes_all_supported_versions() {
        for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
            let (mut client, worker, challenge) = start_vnc_auth(version);
            let password = VncPassword::from_bytes(b"password").unwrap();
            client.write_all(&password.response(challenge)).unwrap();
            let mut result = [255; 4];
            client.read_exact(&mut result).unwrap();
            assert_eq!(result, [0; 4]);
            client.write_all(&[1]).unwrap();
            assert_server_init(&mut client, worker);
        }
    }

    #[test]
    fn wrong_password_fails_without_sending_server_init() {
        for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
            let (mut client, worker, challenge) = start_vnc_auth(version);
            let password = VncPassword::from_bytes(b"password").unwrap();
            let mut response = password.response(challenge);
            response[0] ^= 1;
            client.write_all(&response).unwrap();
            let mut failure = Vec::new();
            client.read_to_end(&mut failure).unwrap();
            let mut expected = vec![0, 0, 0, 1];
            if version == b"RFB 003.008\n" {
                expected.extend_from_slice(&25u32.to_be_bytes());
                expected.extend_from_slice(b"VNC authentication failed");
            }
            assert_eq!(failure, expected);
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn password_cannot_be_bypassed_by_selecting_none() {
        for version in [b"RFB 003.007\n", b"RFB 003.008\n"] {
            let (mut client, worker) =
                connect_with_password(Some(VncPassword::from_bytes(b"password").unwrap()));
            client.write_all(version).unwrap();
            let mut security = [0; 2];
            client.read_exact(&mut security).unwrap();
            assert_eq!(security, [1, 2]);
            client.write_all(&[1]).unwrap();
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
        }
    }
}
