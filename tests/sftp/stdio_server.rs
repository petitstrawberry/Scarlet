//! Real SFTP engine on host files, for OpenSSH's direct-server validation mode.
#[path = "../../user/bin/src/sshd/sftp.rs"]
mod sftp;
use std::io::{Read, Write};
fn main() {
    let home = std::env::var("SCARLET_SFTP_TEST_HOME").expect("fixture home required");
    let mut server = sftp::Server::new(&home);
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut buf = [0u8; 4096];
    loop {
        let n = input.read(&mut buf).unwrap();
        if n == 0 {
            assert!(!server.has_partial_request());
            break;
        }
        server.feed(&buf[..n]).unwrap();
        while let Some(response) = server.next_response().unwrap() {
            output.write_all(&response).unwrap();
            output.flush().unwrap();
        }
    }
}
