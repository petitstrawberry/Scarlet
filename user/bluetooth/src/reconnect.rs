//! BlueZ input-profile policy: only a previously live host/any HID peer can
//! start a 30-second retry timer. Disk registration alone never arms a page.
use super::{
    Address, ConnectedPeer, Handle, HidReconnectMode, Result, bonds, bt, control, flags,
    generation, hid_mode, last_error, record_peer, status,
};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(30);
const ATTEMPTS: u32 = 6;
pub const POLL: Duration = Duration::from_secs(5);
// Match the controller's bounded peer history. Reserve slots for live peers first.
const MAX_LOSSES: usize = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
struct Retry {
    generation: u32,
    due: Instant,
    expires: Instant,
    attempts: u32,
}

impl Retry {
    fn attempted(&mut self, now: Instant) {
        self.attempts += 1;
        self.due = now + INTERVAL;
    }
}

#[derive(Default)]
struct Tracker {
    live: BTreeMap<Address, u32>,
    lost: BTreeMap<Address, Retry>,
}

impl Tracker {
    fn observe(
        &mut self,
        eligible: BTreeMap<Address, u32>,
        connected: &[ConnectedPeer],
        now: Instant,
    ) {
        for peer in connected {
            self.cancel(peer.address);
        }
        for (peer, generation) in std::mem::replace(&mut self.live, eligible) {
            if self.lost.len() < MAX_LOSSES {
                self.lost.insert(
                    peer,
                    Retry {
                        generation,
                        due: now + INTERVAL,
                        // Allow the final timer tick to be serviced on the next worker poll.
                        expires: now + INTERVAL * ATTEMPTS + POLL,
                        attempts: 0,
                    },
                );
            }
        }
    }

    fn cancel(&mut self, peer: Address) {
        self.live.remove(&peer);
        self.lost.remove(&peer);
    }

    fn due(&mut self, now: Instant) -> Option<(Address, Retry)> {
        self.lost
            .retain(|_, p| p.attempts < ATTEMPTS && now < p.expires);
        self.lost
            .iter()
            .filter(|(_, p)| p.due <= now)
            .min_by_key(|(_, p)| p.due)
            .map(|(peer, plan)| (*peer, *plan))
    }
}

// Only the daemon observes link snapshots. Its IPC thread also cancels timers.
// No key material or file/adapter handles enter this table.
static TRACKERS: Mutex<BTreeMap<bt::Address, Tracker>> = Mutex::new(BTreeMap::new());

pub fn reset(adapter: bt::Address) {
    TRACKERS.lock().unwrap().remove(&adapter);
}

pub fn cancel(adapter: bt::Address, peer: Address) {
    if let Some(t) = TRACKERS.lock().unwrap().get_mut(&adapter) {
        t.cancel(peer);
    }
}

fn host_mode(mode: Option<HidReconnectMode>) -> bool {
    matches!(mode, Some(HidReconnectMode::Host | HidReconnectMode::Any))
}

pub fn observe(handle: &Handle, adapter: bt::Address, rows: &[ConnectedPeer]) {
    let mut trackers = TRACKERS.lock().unwrap();
    let mut eligible = BTreeMap::new();
    for row in rows.iter().filter(|p| host_mode(p.hid_reconnect_mode)) {
        // LINK_RESULT's generation describes the entire listing. PEER_STATUS's
        // generation belongs to this connection and changes for manual operations.
        if let Ok(state) = status(handle, Some(row.address.0))
            && flags(&state) & (bt::CONNECTED | bt::ENCRYPTED | bt::BUSY)
                == bt::CONNECTED | bt::ENCRYPTED
            && host_mode(hid_mode(&state))
        {
            eligible.insert(row.address, generation(&state));
        }
    }
    trackers
        .entry(adapter)
        .or_default()
        .observe(eligible, rows, Instant::now());
}

fn lost_connection(row: &bt::Record, expected: u32) -> bool {
    generation(row) == expected
        && flags(row) & (bt::CONNECTED | bt::BUSY) == 0
        && last_error(row) != 0
}

pub fn poll(handle: &Handle, adapter: bt::Address) -> Result<()> {
    let now = Instant::now();
    loop {
        let due = TRACKERS
            .lock()
            .unwrap()
            .get_mut(&adapter)
            .and_then(|t| t.due(now));
        let Some((peer, plan)) = due else {
            return Ok(());
        };
        let Some(_store) = bonds::Store::try_lock(adapter)? else {
            return Ok(());
        };
        // A local disconnect or forget starts a new generation. Also require a
        // terminal error: a confirmed user disconnect has LAST_ERROR == 0.
        let state = status(handle, Some(peer.0));
        if !state
            .is_ok_and(|row| row[bt::ADAPTER] == adapter && lost_connection(&row, plan.generation))
        {
            cancel(adapter, peer);
            continue;
        }
        // The controller requires its registered bond for a keyless CONNECT.
        let mut request = [0; bt::RECORD_SIZE];
        request[bt::PEER].copy_from_slice(&peer.0);

        // Recheck cancellation after storage I/O. Serialize only submission
        // with explicit intent. There is no wait for paging/authentication here.
        let mut trackers = TRACKERS.lock().unwrap();
        let Some(p) = trackers
            .get_mut(&adapter)
            .and_then(|t| t.lost.get_mut(&peer))
        else {
            continue;
        };
        if *p != plan {
            continue;
        }
        p.attempted(now);
        control(handle, bt::CONNECT, &mut request)?;
        if record_peer(&request) != Some(peer.0) {
            return Err("controller did not confirm HID reconnect target".into());
        }
        p.generation = generation(&request);
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_host_and_any_advertise_outgoing_reconnect() {
        for mode in [
            None,
            Some(HidReconnectMode::Device),
            Some(HidReconnectMode::None),
        ] {
            assert!(!host_mode(mode));
        }
        assert!(host_mode(Some(HidReconnectMode::Host)));
        assert!(host_mode(Some(HidReconnectMode::Any)));
    }

    #[test]
    fn remote_loss_timer_is_bounded_and_cancelled_by_explicit_intent() {
        let now = Instant::now();
        let peer = Address([7; 6]);
        let mut tracker = Tracker::default();
        tracker.observe(BTreeMap::new(), &[], now);
        assert!(tracker.due(now + INTERVAL).is_none());
        tracker.live.insert(peer, 9);
        tracker.observe(BTreeMap::new(), &[], now);
        assert!(
            tracker
                .due(now + INTERVAL - Duration::from_millis(1))
                .is_none()
        );
        for tick in 1..=ATTEMPTS {
            let at = now + INTERVAL * tick;
            let (candidate, plan) = tracker.due(at).unwrap();
            assert_eq!((candidate, plan.generation), (peer, 9));
            tracker.lost.get_mut(&peer).unwrap().attempted(at);
            assert!(tracker.due(at).is_none());
        }
        assert!(tracker.lost.is_empty());
        tracker.live.insert(peer, 10);
        tracker.observe(BTreeMap::new(), &[], now);
        tracker.cancel(peer);
        assert!(tracker.due(now + INTERVAL).is_none());

        tracker.live.insert(peer, 11);
        tracker.observe(BTreeMap::new(), &[], now);
        assert!(tracker.due(now + INTERVAL * ATTEMPTS + POLL).is_none());
    }

    #[test]
    fn only_terminal_loss_of_the_observed_generation_can_page() {
        let mut state = [0; bt::RECORD_SIZE];
        state[bt::GENERATION].copy_from_slice(&7u32.to_le_bytes());
        assert!(!lost_connection(&state, 7));
        state[bt::LAST_ERROR].copy_from_slice(&8u32.to_le_bytes());
        assert!(lost_connection(&state, 7));
        assert!(!lost_connection(&state, 6));
        for flag in [bt::BUSY, bt::CONNECTED] {
            state[bt::FLAGS].copy_from_slice(&flag.to_le_bytes());
            assert!(!lost_connection(&state, 7));
        }
    }
}
