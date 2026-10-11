//! Shared stemd system-power protocol and command-line parsing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    PowerOff,
    Reboot,
}

impl PowerAction {
    pub const fn command(self) -> u8 {
        match self {
            Self::PowerOff => 0x04,
            Self::Reboot => 0x09,
        }
    }
    pub fn from_command(command: u8) -> Option<Self> {
        match command {
            0x04 => Some(Self::PowerOff),
            0x09 => Some(Self::Reboot),
            _ => None,
        }
    }
    pub fn from_text(command: &str) -> Option<Self> {
        match command.trim() {
            "shutdown" => Some(Self::PowerOff),
            "reboot" => Some(Self::Reboot),
            _ => None,
        }
    }
    pub const fn response(self) -> &'static str {
        match self {
            Self::PowerOff => "OK: Shutting down\n",
            Self::Reboot => "OK: Rebooting\n",
        }
    }
}

/// None requests help. Reject unknown/contradictory options before connecting.
pub fn parse_args(
    default: PowerAction,
    args: &[&str],
) -> Result<Option<PowerAction>, &'static str> {
    if args == ["--help"] {
        return Ok(None);
    }
    let mut action = default;
    let mut selected = false;
    let mut now = false;
    for arg in args {
        match *arg {
            "-r" | "--reboot" if !selected && !now => {
                action = PowerAction::Reboot;
                selected = true;
            }
            "-h" | "-P" | "--poweroff" if !selected && !now => {
                action = PowerAction::PowerOff;
                selected = true;
            }
            "now" if !now => now = true,
            _ => return Err("expected [-r|--reboot|-P|--poweroff] [now] or --help"),
        }
    }
    Ok(Some(action))
}

#[cfg(target_os = "scarlet")]
pub fn request(action: PowerAction) -> Result<(), std::string::String> {
    use scarlet_os::socket::Socket;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let mut socket = Socket::new().map_err(|e| format!("create stemd socket: {e:?}"))?;
    socket
        .connect("/tmp/stemd.sock")
        .map_err(|e| format!("connect to stemd: {e:?}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("configure stemd socket: {e:?}"))?;
    socket
        .write_all(&[action.command()])
        .map_err(|e| format!("send power request: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut response = [0u8; 256];
    let mut filled = 0;
    while filled < response.len() {
        match socket.read(&mut response[filled..]) {
            Ok(0) => return Err("stemd closed without acknowledging the power request".into()),
            Ok(count) => {
                filled += count;
                if response[..filled].contains(&b'\n') {
                    let reply = std::str::from_utf8(&response[..filled])
                        .map_err(|_| "invalid stemd reply")?;
                    return if reply == action.response() {
                        Ok(())
                    } else {
                        Err(reply.trim().into())
                    };
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("read stemd reply: {error}")),
        }
        if Instant::now() >= deadline {
            return Err("stemd power request timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("stemd power reply is too long".into())
}

#[cfg(target_os = "scarlet")]
pub fn run_cli(default: PowerAction) -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match parse_args(default, &refs) {
        Ok(None) => {
            println!("Usage: shutdown [-r|--reboot|-P|--poweroff] [now]\n       reboot [now]");
            std::process::ExitCode::SUCCESS
        }
        Ok(Some(action)) => match request(action) {
            Ok(()) => {
                print!("{}", action.response());
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("power request failed: {error}");
                std::process::ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_keeps_shutdown_compatible_and_distinguishes_reboot() {
        assert_eq!(PowerAction::from_command(4), Some(PowerAction::PowerOff));
        assert_eq!(PowerAction::from_command(9), Some(PowerAction::Reboot));
        for action in [PowerAction::PowerOff, PowerAction::Reboot] {
            assert_eq!(PowerAction::from_command(action.command()), Some(action));
        }
        for command in [0, 1, 5, 6, 7, 8, 255] {
            assert_eq!(PowerAction::from_command(command), None);
        }
    }
    #[test]
    fn cli_selects_explicit_action_and_rejects_ambiguous_requests() {
        use PowerAction::*;
        assert_eq!(parse_args(PowerOff, &[]), Ok(Some(PowerOff)));
        assert_eq!(parse_args(Reboot, &[]), Ok(Some(Reboot)));
        assert_eq!(parse_args(PowerOff, &["-r", "now"]), Ok(Some(Reboot)));
        assert_eq!(parse_args(Reboot, &["-P", "now"]), Ok(Some(PowerOff)));
        assert_eq!(parse_args(PowerOff, &["--help"]), Ok(None));
        for args in [
            vec!["--bad"],
            vec!["-r", "-P"],
            vec!["tomorrow"],
            vec!["now", "now"],
            vec!["--help", "now"],
        ] {
            assert!(parse_args(PowerOff, &args).is_err());
        }
    }
    #[test]
    fn text_commands_do_not_treat_unrecognized_input_as_poweroff() {
        assert_eq!(
            PowerAction::from_text("reboot\n"),
            Some(PowerAction::Reboot)
        );
        assert_eq!(
            PowerAction::from_text("shutdown\n"),
            Some(PowerAction::PowerOff)
        );
        assert_eq!(PowerAction::from_text("shutdown tomorrow"), None);
    }
}
