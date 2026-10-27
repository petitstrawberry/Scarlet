//! Completion checks use the interface selected by the active radio.
pub fn connection_ready(text: &str) -> bool {
    if !text
        .lines()
        .any(|line| line == "STATE Connected" || line.starts_with("STATE Connected; "))
    {
        return false;
    }
    let Some(interface) = text
        .lines()
        .find_map(|line| line.strip_prefix("INTERFACE "))
    else {
        return false;
    };
    text.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("LINK")
            && words.next() == Some(interface)
            && words.next().is_some_and(|address| {
                address
                    .parse::<core::net::Ipv4Addr>()
                    .is_ok_and(|ip| !ip.is_unspecified())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renamed_mock_interface_completes_after_ip_configuration() {
        assert!(connection_ready(
            "STATE Connected\nINTERFACE veth0\nLINK veth0 10.0.2.15 10.0.2.2\n"
        ));
        assert!(connection_ready(
            "STATE Connected; could not save network\nINTERFACE wifi2\nLINK wifi2 10.0.2.15 10.0.2.2\n"
        ));
    }
    #[test]
    fn another_interface_cannot_complete_a_connection_without_its_address() {
        let text = "STATE Connected\nINTERFACE wifi2\nLINK wlan0 192.168.0.2 192.168.0.1\nLINK wifi2 0.0.0.0 0.0.0.0\n";
        assert!(!connection_ready(text));
        assert!(!connection_ready(
            "STATE Connected\nLINK wlan0 192.168.0.2 192.168.0.1\n"
        ));
        assert!(!connection_ready(
            "STATE Authenticating\nINTERFACE wifi2\nLINK wifi2 10.0.2.15 10.0.2.2\n"
        ));
    }
}
