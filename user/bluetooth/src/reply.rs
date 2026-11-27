//! Allowlisted reply types shared by the text and structured protocols.
use super::{Address, Command, ConnectedPeer, DiscoveredPeer, Status};
use std::fmt::Write;

#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum Reply {
    Status(Status),
    Scan(Vec<DiscoveredPeer>),
    Saved(Vec<DiscoveredPeer>),
    Forgotten(()),
    Devices(Vec<ConnectedPeer>),
    Bonded(Vec<Address>),
}

fn peer_text(
    address: Option<Address>,
    connected: bool,
    encrypted: bool,
    profile_ready: bool,
    key_readable: bool,
    last_error: u32,
) -> String {
    let peer = address
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "none".into());
    format!(
        "peer={peer} connected={connected} encrypted={encrypted} profile-ready={profile_ready} key-readable={key_readable} last-error={last_error:#x}"
    )
}

pub(crate) fn status_text(s: &Status) -> String {
    let mut text = format!(
        "adapter={} ready={} busy={} scanning={} found={} generation={}\n{}",
        s.adapter,
        s.ready,
        s.busy,
        s.scanning,
        s.found,
        s.generation,
        peer_text(
            s.peer,
            s.connected,
            s.encrypted,
            s.profile_ready,
            s.key_readable,
            s.last_error
        )
    );
    if let Some(mode) = s.hid_reconnect_mode {
        write!(text, " hid-reconnect-mode={mode}").unwrap();
    }
    text
}

fn name_text(name: Option<&str>) -> String {
    // Debug quoting keeps remote newlines and terminal controls out of CLI output.
    name.map(|n| format!("{n:?}"))
        .unwrap_or_else(|| "(unknown name)".into())
}

impl Reply {
    pub fn text(&self, cmd: Command) -> String {
        let mut text = String::new();
        match self {
            Self::Status(s) => {
                if let Command::Pair(mac) = cmd {
                    writeln!(text, "bond saved for {}", Address(mac)).unwrap();
                }
                text.push_str(&status_text(s));
            }
            Self::Scan(rows) | Self::Saved(rows) => {
                for row in rows {
                    writeln!(text, "{}  {}", row.address, name_text(row.name.as_deref())).unwrap();
                }
                let verb = if matches!(self, Self::Saved(_)) {
                    "saved"
                } else {
                    "found"
                };
                write!(text, "{} device(s) {verb}", rows.len()).unwrap();
            }
            Self::Forgotten(()) => {
                if let Command::Forget(peer) = cmd {
                    write!(text, "forgot {}", Address(peer)).unwrap();
                }
            }
            Self::Devices(rows) => {
                for row in rows {
                    write!(
                        text,
                        "{} name={}",
                        peer_text(
                            Some(row.address),
                            true,
                            row.encrypted,
                            row.profile_ready,
                            row.key_readable,
                            row.last_error
                        ),
                        name_text(row.name.as_deref())
                    )
                    .unwrap();
                    if let Some(mode) = row.hid_reconnect_mode {
                        write!(text, " hid-reconnect-mode={mode}").unwrap();
                    }
                    text.push('\n');
                }
                write!(text, "{} device(s) connected", rows.len()).unwrap();
            }
            Self::Bonded(rows) => {
                for row in rows {
                    writeln!(text, "{row}").unwrap();
                }
                write!(text, "{} device(s) bonded", rows.len()).unwrap();
            }
        }
        text
    }
}
