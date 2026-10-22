//! Session-owned clipboard data; authorization is checked for every operation.
use sws_protocol::clipboard::{DENIED, EMPTY, OK, Request, Response};

/// Reserve before allocating an owned queued payload.
pub struct PendingRequests {
    clients: std::collections::BTreeSet<usize>,
}
impl PendingRequests {
    pub const fn new() -> Self {
        Self {
            clients: std::collections::BTreeSet::new(),
        }
    }
    pub fn reserve(&mut self, client: usize) -> bool {
        self.clients.len() < 16 && self.clients.insert(client)
    }
    pub fn release(&mut self, client: usize) {
        self.clients.remove(&client);
    }
}

pub struct Clipboard {
    text: Option<String>,
}
impl Clipboard {
    pub const fn new() -> Self {
        Self { text: None }
    }
    pub fn apply(
        &mut self,
        request: Request<'_>,
        focused: Option<u32>,
        owner: Option<usize>,
        client: usize,
        presented: bool,
    ) -> Vec<u8> {
        if !presented || focused != Some(request.window_id()) || owner != Some(client) {
            return Response::encode(DENIED, "").expect("valid denial");
        }
        match request {
            Request::Get { .. } => match &self.text {
                Some(text) => Response::encode(OK, text).expect("bounded stored text"),
                None => Response::encode(EMPTY, "").expect("valid empty response"),
            },
            Request::Set { text, .. } => {
                // Also validate the bound here so internal callers cannot grow
                // the session store beyond the transport contract.
                if text.len() > sws_protocol::clipboard::MAX_TEXT_BYTES {
                    return Response::encode(DENIED, "").expect("valid denial");
                }
                self.text = Some(text.into());
                Response::encode(OK, "").expect("valid acknowledgement")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_payloads_are_bounded_globally_and_per_client() {
        let mut pending = PendingRequests::new();
        assert!(pending.reserve(0));
        assert!(!pending.reserve(0));
        for client in 1..16 {
            assert!(pending.reserve(client));
        }
        assert!(!pending.reserve(16));
        pending.release(7);
        assert!(pending.reserve(16));
        assert!(!pending.reserve(17));
    }
    #[test]
    fn session_cross_client_round_trip_and_ownership() {
        let mut clipboard = Clipboard::new();
        let get = Request::Get { window_id: 1 };
        assert_eq!(
            Response::parse(&clipboard.apply(get, Some(1), Some(10), 10, true))
                .unwrap()
                .status,
            EMPTY
        );
        let text = "日本 e\u{301} 👩‍💻\n\0";
        clipboard.apply(
            Request::Set { window_id: 1, text },
            Some(1),
            Some(10),
            10,
            true,
        );
        // Stale focus alone is insufficient after minimize, hide, workspace
        // changes or presentation revocation.
        assert_eq!(
            Response::parse(&clipboard.apply(get, Some(1), Some(10), 10, false))
                .unwrap()
                .status,
            DENIED
        );
        clipboard.apply(
            Request::Set {
                window_id: 1,
                text: "hidden overwrite",
            },
            Some(1),
            Some(10),
            10,
            false,
        );
        for (focus, owner, client) in [
            (Some(2), Some(10), 10),
            (Some(1), Some(10), 20),
            (Some(1), None, 10),
            (None, Some(10), 10),
        ] {
            assert_eq!(
                Response::parse(&clipboard.apply(get, focus, owner, client, true))
                    .unwrap()
                    .status,
                DENIED
            );
            clipboard.apply(
                Request::Set {
                    window_id: 1,
                    text: "overwrite",
                },
                focus,
                owner,
                client,
                true,
            );
        }
        // A different focused application sees the shared session text; the
        // original writer need not remain connected or retain its own copy.
        let read = clipboard.apply(Request::Get { window_id: 2 }, Some(2), Some(20), 20, true);
        assert_eq!(core::str::from_utf8(&read[8..]).unwrap(), text);
        clipboard.apply(
            Request::Set {
                window_id: 2,
                text: "",
            },
            Some(2),
            Some(20),
            20,
            true,
        );
        assert_eq!(
            Response::parse(&clipboard.apply(
                Request::Get { window_id: 2 },
                Some(2),
                Some(20),
                20,
                true
            ))
            .unwrap()
            .status,
            OK
        );
    }
}
