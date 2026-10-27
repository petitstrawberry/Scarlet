extern crate alloc;
// The control/authentication model does not instantiate a hardware transport.
// Keep the packet callback type available when compiling auth.rs standalone.
pub mod device {
    pub mod network {
        pub struct DevicePacket;
    }
}
#[path = "../../../../kernel/src/device/wireless/auth.rs"]
pub mod auth;
#[path = "../../../../kernel/src/device/wireless/ccmp.rs"]
pub mod ccmp;
#[path = "../../../../kernel/src/device/wireless/core.rs"]
pub mod core;
#[path = "../../../../kernel/src/device/wireless/gate.rs"]
pub mod gate;
#[path = "../../../../kernel/src/device/wireless/mock.rs"]
pub mod mock;

#[cfg(test)]
mod tests {
    use super::{core::*, mock::MockBackend};
    use alloc::{boxed::Box, vec};
    use scarlet_abi::wireless::*;
    fn radio(mode: u32) -> (Radio, u64) {
        let mut radio = Radio::new(42, [0; 32], Box::new(MockBackend::new(mode, false)));
        let session = radio.open().unwrap();
        assert_eq!(
            call(&mut radio, session, CLAIM_CONTROLLER, &[], 0, 0)
                .header
                .result,
            OK
        );
        (radio, session)
    }
    fn call(
        r: &mut Radio,
        s: u64,
        command: u32,
        payload: &[u8],
        capacity: usize,
        id: u64,
    ) -> Reply {
        let len = payload.len().max(capacity);
        let h = Header {
            request_id: id,
            link_id: r.link_id,
            device_epoch: r.device_epoch,
            connection_id: r.connection_id,
            expected_state_seq: r.state_seq,
            ..Header::new(HEADER_SIZE + len)
        };
        let mut input = vec![0; len];
        input[..payload.len()].copy_from_slice(payload);
        r.command(s, command, h, &input, 0)
    }
    fn connect() -> Connect {
        let mut c = Connect {
            ssid_len: 3,
            akm: AKM_OPEN,
            timeout_ms: 100,
            ..Connect::default()
        };
        c.ssid[..3].copy_from_slice(&[0xff, 0, 42]);
        c
    }
    #[test]
    fn worker_deadlines_follow_pending_work_and_disappear_after_completion() {
        let mut r = Radio::new(42, [0; 32], Box::new(MockBackend::new(MLME_FIRMWARE, true)));
        let s = r.open().unwrap();
        assert_eq!(r.next_deadline_ns(), None);
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        let request = call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
        assert_eq!(request.header.result, ACCEPTED);
        let due = r.next_deadline_ns().unwrap();
        assert!(due > 0 && due < 100_000_000);
        r.pump(due - 1);
        assert_eq!(r.next_deadline_ns(), Some(due));
        r.pump(due);
        assert_eq!(r.next_deadline_ns(), None);
        call(
            &mut r,
            s,
            DISCONNECT,
            Scan {
                timeout_ms: 1,
                reserved: 0,
            }
            .bytes(),
            0,
            2,
        );
        assert!(r.next_deadline_ns().is_some());
        r.pump(1_000_000_000);
        assert_eq!(r.next_deadline_ns(), None);
    }
    fn token(r: &Radio, s: u64, request: u64) -> Token {
        Token {
            session: s,
            request,
            epoch: r.device_epoch,
            connection: r.connection_id,
        }
    }
    fn complete(r: &mut Radio, token: Token, kind: u32) {
        let bss = Bss {
            ssid_len: 0,
            frequency_mhz: 2412,
            ..Bss::default()
        };
        r.apply(
            BackendEvent {
                token,
                kind,
                result: OK,
                bss: vec![bss],
            },
            1,
        );
    }
    fn result(r: &mut Radio, s: u64, id: u64) -> RequestInfo {
        let reply = call(r, s, GET_REQUEST, Id { id }.bytes(), 32, 0);
        assert_eq!(reply.header.result, OK);
        RequestInfo::decode(&reply.body).unwrap()
    }
    #[test]
    fn controlled_port_rejects_unbound_closed_and_retired_tokens() {
        let mut gate = super::gate::Gate::new();
        assert!(!gate.authorize());
        gate.link_id = 42;
        assert!(!gate.accepts(None));
        assert!(gate.authorize());
        let old = gate.token();
        assert!(gate.accepts(old));
        assert!(!gate.accepts(Some((43, gate.epoch))));
        assert!(gate.retire());
        assert!(!gate.accepts(old));
        assert!(gate.authorize());
        assert!(!gate.accepts(old));
        assert!(gate.accepts(gate.token()));
        gate.epoch = u64::MAX;
        assert!(!gate.retire());
        assert!(!gate.authorize());
        assert_eq!(gate.token(), None);
    }
    #[test]
    fn data_connect_completes_only_after_facade_authorization() {
        for succeeds in [true, false] {
            let mut r = Radio::new(
                42,
                [0; 32],
                Box::new(MockBackend::new(MLME_FIRMWARE, false).with_data()),
            );
            let s = r.open().unwrap();
            assert_eq!(
                call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0).header.result,
                OK
            );
            assert_eq!(
                call(&mut r, s, CONNECT, connect().bytes(), 0, 1)
                    .header
                    .result,
                ACCEPTED
            );
            let t = token(&r, s, 1);
            complete(&mut r, t, BACKEND_ASSOCIATED);
            assert_eq!(r.status.phase, ASSOCIATED);
            assert_eq!(result(&mut r, s, 1).result, ACCEPTED);
            complete(&mut r, t, BACKEND_COMPLETE);
            assert_eq!(result(&mut r, s, 1).result, ACCEPTED);
            // A duplicate association cannot roll back readiness pending commit.
            complete(&mut r, t, BACKEND_ASSOCIATED);
            assert_eq!(r.status.phase, AUTHORIZED);
            r.complete_data_authorization(succeeds, 2);
            assert_eq!(
                result(&mut r, s, 1).result,
                if succeeds { OK } else { FAILED }
            );
            assert_eq!(r.status.phase, if succeeds { AUTHORIZED } else { IDLE });
        }
    }
    #[test]
    fn data_mock_controls_are_capability_and_owner_gated() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        let request = DataTest::default();
        assert_eq!(
            call(&mut r, s, MOCK_DATA_TEST, request.bytes(), 72, 0)
                .header
                .result,
            NOT_SUPPORTED
        );
        let mut r = Radio::new(
            42,
            [0; 32],
            Box::new(MockBackend::new(MLME_FIRMWARE, false).with_data()),
        );
        let s = r.open().unwrap();
        assert_eq!(
            call(&mut r, s, MOCK_DATA_TEST, request.bytes(), 72, 0)
                .header
                .result,
            NOT_OWNER
        );
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        assert_eq!(
            call(&mut r, s, MOCK_DATA_TEST, request.bytes(), 72, 0)
                .header
                .result,
            OK
        );
        let invalid = DataTest {
            tx_queued: 1,
            ..request
        };
        assert_eq!(
            call(&mut r, s, MOCK_DATA_TEST, invalid.bytes(), 72, 0)
                .header
                .result,
            INVALID
        );
        assert_eq!(
            call(
                &mut r,
                s,
                MOCK_DATA_TEST,
                DataTest {
                    operation: 99,
                    ..request
                }
                .bytes(),
                72,
                0
            )
            .header
            .result,
            INVALID
        );
    }
    #[test]
    fn both_mlme_modes_share_scan_and_connection_contract() {
        for mode in [MLME_FIRMWARE, MLME_HOST] {
            let (mut r, s) = radio(mode);
            assert_eq!(
                call(&mut r, s, SUBSCRIBE_EVENTS, &[], 0, 0).header.result,
                OK
            );
            assert_eq!(
                call(&mut r, s, SCAN, Scan::default().bytes(), 0, 1)
                    .header
                    .result,
                ACCEPTED
            );
            let t = token(&r, s, 1);
            complete(&mut r, t, BACKEND_COMPLETE);
            let scan_id = result(&mut r, s, 1).object_id;
            let page = call(
                &mut r,
                s,
                GET_SCAN_RESULTS,
                ScanPage {
                    scan_id,
                    cursor: 0,
                    reserved: 0,
                }
                .bytes(),
                80,
                0,
            );
            assert_eq!(page.body.len(), 80);
            assert_eq!(Bss::decode(&page.body[16..]).unwrap().frequency_mhz, 2412);
            assert_eq!(
                call(&mut r, s, CONNECT, connect().bytes(), 0, 2)
                    .header
                    .result,
                ACCEPTED
            );
            assert_eq!(
                r.status.phase,
                if mode == MLME_HOST {
                    AUTHENTICATING
                } else {
                    ASSOCIATING
                }
            );
            let t = token(&r, s, 2);
            complete(&mut r, t, BACKEND_ASSOCIATED);
            assert_eq!(r.status.phase, ASSOCIATED);
            assert_eq!(result(&mut r, s, 2).result, ACCEPTED);
            complete(&mut r, t, BACKEND_COMPLETE);
            assert_eq!(r.status.phase, AUTHORIZED);
            assert_eq!(result(&mut r, s, 2).result, OK);
            assert_eq!(
                call(&mut r, s, DISCONNECT, Scan::default().bytes(), 0, 3)
                    .header
                    .result,
                ACCEPTED
            );
            let t = token(&r, s, 3);
            complete(&mut r, t, BACKEND_COMPLETE);
            assert_eq!(r.status.phase, IDLE);
            assert_eq!(r.connection_id, 0);
        }
    }
    #[test]
    fn writer_lease_close_and_late_callback_fence() {
        let (mut r, a) = radio(MLME_HOST);
        let b = r.open().unwrap();
        assert_eq!(
            call(&mut r, b, CLAIM_CONTROLLER, &[], 0, 0).header.result,
            BUSY
        );
        assert_eq!(
            call(&mut r, b, SCAN, Scan::default().bytes(), 0, 1)
                .header
                .result,
            NOT_OWNER
        );
        call(&mut r, a, CONNECT, connect().bytes(), 0, 1);
        let old = token(&r, a, 1);
        r.close(a, 1);
        assert_eq!(
            call(&mut r, b, CLAIM_CONTROLLER, &[], 0, 0).header.result,
            OK
        );
        call(&mut r, b, CONNECT, connect().bytes(), 0, 1);
        let current = r.connection_id;
        complete(&mut r, old, BACKEND_COMPLETE);
        assert_eq!(r.connection_id, current);
        assert_eq!(r.status.phase, AUTHENTICATING);
        assert_eq!(result(&mut r, b, 1).result, ACCEPTED);
    }
    #[test]
    fn cancel_reconnect_rejects_obsolete_connection_and_duplicate_ids() {
        let (mut r, s) = radio(MLME_HOST);
        call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
        let old = token(&r, s, 1);
        call(&mut r, s, CANCEL_REQUEST, Id { id: 1 }.bytes(), 0, 0);
        assert_eq!(result(&mut r, s, 1).result, CANCELED);
        call(&mut r, s, ACK_REQUEST, Id { id: 1 }.bytes(), 0, 0);
        assert_eq!(
            call(&mut r, s, CONNECT, connect().bytes(), 0, 1)
                .header
                .result,
            INVALID
        );
        assert_eq!(
            call(&mut r, s, CONNECT, connect().bytes(), 0, 2)
                .header
                .result,
            ACCEPTED
        );
        let current = r.connection_id;
        complete(&mut r, old, BACKEND_COMPLETE);
        assert_eq!(r.connection_id, current);
        assert_eq!(r.status.phase, AUTHENTICATING);
    }
    #[test]
    fn copyout_failure_and_short_output_do_not_consume_events() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        call(&mut r, s, SUBSCRIBE_EVENTS, &[], 0, 0);
        call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
        let short = call(&mut r, s, DRAIN_EVENTS, &[], 0, 0);
        assert_eq!(short.header.result, BUFFER_TOO_SMALL);
        assert!(short.consume.is_none());
        assert_eq!(short.header.buffer_len as usize, HEADER_SIZE + 96);
        let first = call(&mut r, s, DRAIN_EVENTS, &[], 96, 0);
        let retry = call(&mut r, s, DRAIN_EVENTS, &[], 96, 0);
        assert_eq!(first.body, retry.body); // first copyout failed: no commit
        r.commit(s, &retry);
        assert_eq!(
            call(&mut r, s, DRAIN_EVENTS, &[], 96, 0).header.result,
            WOULD_BLOCK
        );
    }
    #[test]
    fn overflow_is_sticky_until_successful_atomic_resync() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        call(&mut r, s, SUBSCRIBE_EVENTS, &[], 0, 0);
        for id in 1..=70 {
            call(&mut r, s, SCAN, Scan::default().bytes(), 0, id);
            call(&mut r, s, CANCEL_REQUEST, Id { id }.bytes(), 0, 0);
            call(&mut r, s, ACK_REQUEST, Id { id }.bytes(), 0, 0);
        }
        let overflow = call(&mut r, s, DRAIN_EVENTS, &[], 96, 0);
        assert_eq!(Event::decode(&overflow.body).unwrap().kind, EVENT_OVERFLOW);
        r.commit(s, &overflow);
        assert!(r.ready(s));
        let short = call(&mut r, s, RESYNC, &[], 0, 0);
        r.commit(s, &short);
        assert!(r.ready(s));
        let good = call(&mut r, s, RESYNC, &[], ::core::mem::size_of::<Status>(), 0);
        let cursor = Status::decode(&good.body).unwrap().event_sequence;
        assert_eq!(cursor, Event::decode(&overflow.body).unwrap().sequence);
        // Failed copyout must preserve the pending resynchronization and queue.
        assert!(r.ready(s));
        let retry = call(&mut r, s, RESYNC, &[], ::core::mem::size_of::<Status>(), 0);
        assert_eq!(retry.body, good.body);
        r.commit(s, &retry);
        assert!(!r.ready(s));
        assert!(!r.readiness(s).load(::core::sync::atomic::Ordering::Acquire));
        call(&mut r, s, SCAN, Scan::default().bytes(), 0, 71);
        call(&mut r, s, CANCEL_REQUEST, Id { id: 71 }.bytes(), 0, 0);
        let next = call(&mut r, s, DRAIN_EVENTS, &[], 96, 0);
        assert_eq!(Event::decode(&next.body).unwrap().sequence, cursor + 1);
    }

    #[test]
    fn resync_preserves_other_sessions_and_returns_each_sessions_cursor() {
        let (mut r, owner) = radio(MLME_HOST);
        call(&mut r, owner, SUBSCRIBE_EVENTS, &[], 0, 0);
        call(&mut r, owner, SCAN, Scan::default().bytes(), 0, 1);
        call(&mut r, owner, CANCEL_REQUEST, Id { id: 1 }.bytes(), 0, 0);
        let observer = r.open().unwrap();
        call(&mut r, observer, SUBSCRIBE_EVENTS, &[], 0, 0);
        call(&mut r, owner, SCAN, Scan::default().bytes(), 0, 2);
        call(&mut r, owner, CANCEL_REQUEST, Id { id: 2 }.bytes(), 0, 0);
        let owner_snapshot = call(
            &mut r,
            owner,
            RESYNC,
            &[],
            ::core::mem::size_of::<Status>(),
            0,
        );
        assert_eq!(
            Status::decode(&owner_snapshot.body).unwrap().event_sequence,
            2
        );
        r.commit(owner, &owner_snapshot);
        assert!(!r.ready(owner));
        assert!(r.ready(observer));
        let observer_snapshot = call(
            &mut r,
            observer,
            GET_STATUS,
            &[],
            ::core::mem::size_of::<Status>(),
            0,
        );
        assert_eq!(
            Status::decode(&observer_snapshot.body)
                .unwrap()
                .event_sequence,
            1
        );
        let observer_event = call(&mut r, observer, DRAIN_EVENTS, &[], 96, 0);
        assert_eq!(Event::decode(&observer_event.body).unwrap().sequence, 1);
    }
    #[test]
    fn deadlines_win_over_same_time_callback() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        let request = Scan {
            timeout_ms: 1,
            reserved: 0,
        };
        call(&mut r, s, SCAN, request.bytes(), 0, 1);
        let t = token(&r, s, 1);
        r.pump(1_000_000);
        complete(&mut r, t, BACKEND_COMPLETE);
        assert_eq!(result(&mut r, s, 1).result, TIMED_OUT);
    }
    #[test]
    fn terminal_ledger_is_bounded_and_ack_releases_capacity() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        for id in 1..=MAX_REQUESTS as u64 {
            assert_eq!(
                call(&mut r, s, SCAN, Scan::default().bytes(), 0, id)
                    .header
                    .result,
                ACCEPTED
            );
            call(&mut r, s, CANCEL_REQUEST, Id { id }.bytes(), 0, 0);
        }
        assert_eq!(
            call(&mut r, s, SCAN, Scan::default().bytes(), 0, 65)
                .header
                .result,
            RESOURCE_LIMIT
        );
        call(&mut r, s, ACK_REQUEST, Id { id: 1 }.bytes(), 0, 0);
        assert_eq!(
            call(&mut r, s, SCAN, Scan::default().bytes(), 0, 65)
                .header
                .result,
            ACCEPTED
        );
    }
    #[test]
    fn reset_and_removal_are_persistent_and_fence_old_epoch() {
        let (mut r, s) = radio(MLME_HOST);
        call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
        let t = token(&r, s, 1);
        complete(&mut r, t, BACKEND_RESET);
        assert_eq!(r.device_epoch, 2);
        assert_eq!(result(&mut r, s, 1).result, STALE_DEVICE);
        complete(&mut r, t, BACKEND_COMPLETE);
        assert_eq!(r.status.phase, IDLE);
        let t = token(&r, s, 0);
        complete(&mut r, t, BACKEND_REMOVED);
        assert!(r.open().is_err());
        assert!(r.ready(s));
        let response = call(
            &mut r,
            s,
            GET_STATUS,
            &[],
            ::core::mem::size_of::<Status>(),
            0,
        );
        assert_eq!(
            Status::decode(&response.body).unwrap().availability,
            RADIO_REMOVED
        );
        let response = call(&mut r, s, RESYNC, &[], ::core::mem::size_of::<Status>(), 0);
        r.commit(s, &response);
        assert!(r.ready(s));
        assert_eq!(
            call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0).header.result,
            DEVICE_GONE
        );
    }
    #[test]
    fn scan_snapshots_expire_and_short_page_is_not_partial() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        call(&mut r, s, SCAN, Scan::default().bytes(), 0, 1);
        let t = token(&r, s, 1);
        complete(&mut r, t, BACKEND_COMPLETE);
        let id = result(&mut r, s, 1).object_id;
        let page = ScanPage {
            scan_id: id,
            cursor: 0,
            reserved: 0,
        };
        let response = call(&mut r, s, GET_SCAN_RESULTS, page.bytes(), 16, 0);
        assert_eq!(response.header.result, BUFFER_TOO_SMALL);
        assert!(response.body.is_empty());
        r.pump(31_000_000_000);
        assert_eq!(
            call(&mut r, s, GET_SCAN_RESULTS, page.bytes(), 80, 0)
                .header
                .result,
            NOT_FOUND
        );
    }
    #[test]
    fn malformed_input_and_unsupported_security_have_no_side_effects() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        let c = connect();
        let base = Header {
            link_id: 42,
            device_epoch: 1,
            expected_state_seq: 1,
            request_id: 1,
            ..Header::new(112)
        };
        for h in [
            Header {
                reserved: 1,
                ..base
            },
            Header { flags: 1, ..base },
            Header {
                abi_version: 9,
                ..base
            },
            Header {
                buffer_len: 63,
                ..base
            },
        ] {
            assert_eq!(
                r.command(s, CONNECT, h, c.bytes(), 0).header.result,
                INVALID
            );
        }
        let mut c = c;
        c.ssid_len = 33;
        assert_eq!(
            call(&mut r, s, CONNECT, c.bytes(), 0, 1).header.result,
            INVALID
        );
        c.ssid_len = 3;
        c.akm = 2;
        assert_eq!(
            call(&mut r, s, CONNECT, c.bytes(), 0, 1).header.result,
            NOT_SUPPORTED
        );
        assert_eq!(r.connection_id, 0);
        assert_eq!(
            call(&mut r, s, INSTALL_KEY, &[], 0, 0).header.result,
            NOT_SUPPORTED
        );
    }
    #[test]
    fn session_limit_and_stale_state_comparison() {
        let (mut r, s) = radio(MLME_HOST);
        for _ in 1..MAX_SESSIONS {
            r.open().unwrap();
        }
        assert_eq!(r.open(), Err(RESOURCE_LIMIT));
        let h = Header {
            link_id: 42,
            device_epoch: 1,
            request_id: 1,
            expected_state_seq: 0,
            ..Header::new(112)
        };
        assert_eq!(
            r.command(s, CONNECT, h, connect().bytes(), 0).header.result,
            STATE_CHANGED
        );
        assert_eq!(r.connection_id, 0);
    }
    #[test]
    fn automatic_backend_advances_without_query_side_effects() {
        for mode in [MLME_FIRMWARE, MLME_HOST] {
            let mut r = Radio::new(42, [0; 32], Box::new(MockBackend::new(mode, true)));
            let s = r.open().unwrap();
            call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
            call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
            r.pump(30_000_000);
            assert_eq!(
                r.status.phase,
                if mode == MLME_HOST {
                    ASSOCIATED
                } else {
                    ASSOCIATING
                }
            );
            r.pump(50_000_000);
            assert_eq!(r.status.phase, AUTHORIZED);
            assert_eq!(result(&mut r, s, 1).result, OK);
        }
    }
    #[test]
    fn snapshot_limit_rejects_before_admission_and_release_allows_retry() {
        let (mut r, s) = radio(MLME_FIRMWARE);
        let mut first = 0;
        for id in 1..=MAX_SCANS as u64 {
            assert_eq!(
                call(&mut r, s, SCAN, Scan::default().bytes(), 0, id)
                    .header
                    .result,
                ACCEPTED
            );
            let t = token(&r, s, id);
            complete(&mut r, t, BACKEND_COMPLETE);
            if first == 0 {
                first = result(&mut r, s, id).object_id;
            }
        }
        assert_eq!(
            call(&mut r, s, SCAN, Scan::default().bytes(), 0, 5)
                .header
                .result,
            RESOURCE_LIMIT
        );
        assert_eq!(
            call(&mut r, s, GET_REQUEST, Id { id: 5 }.bytes(), 32, 0)
                .header
                .result,
            NOT_FOUND
        );
        call(
            &mut r,
            s,
            RELEASE_SCAN_RESULTS,
            Id { id: first }.bytes(),
            0,
            0,
        );
        assert_eq!(
            call(&mut r, s, SCAN, Scan::default().bytes(), 0, 5)
                .header
                .result,
            ACCEPTED
        );
    }
    #[test]
    fn backend_submission_failure_is_terminal_and_does_not_leave_busy_state() {
        struct Reject;
        impl WirelessBackend for Reject {
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    mlme: MLME_HOST,
                    akm_mask: AKM_OPEN,
                    ..Capabilities::default()
                }
            }
            fn submit(&mut self, _: BackendRequest, _: u64) -> Result<(), i32> {
                Err(FAILED)
            }
            fn cancel(&mut self, _: Token) {}
            fn poll(&mut self, _: u64) -> Option<BackendEvent> {
                None
            }
        }
        let mut r = Radio::new(42, [0; 32], Box::new(Reject));
        let s = r.open().unwrap();
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        assert_eq!(
            call(&mut r, s, CONNECT, connect().bytes(), 0, 1)
                .header
                .result,
            FAILED
        );
        assert_eq!(result(&mut r, s, 1).result, FAILED);
        assert_eq!(r.status.phase, IDLE);
        assert_eq!(r.connection_id, 0);
        assert_eq!(
            call(&mut r, s, CONNECT, connect().bytes(), 0, 2)
                .header
                .result,
            FAILED
        );
    }
    #[test]
    fn disconnect_cancels_pending_connect_and_fences_its_completion() {
        let (mut r, s) = radio(MLME_HOST);
        call(&mut r, s, CONNECT, connect().bytes(), 0, 1);
        let old = token(&r, s, 1);
        assert_eq!(
            call(&mut r, s, DISCONNECT, Scan::default().bytes(), 0, 2)
                .header
                .result,
            ACCEPTED
        );
        assert_eq!(result(&mut r, s, 1).result, CANCELED);
        complete(&mut r, old, BACKEND_COMPLETE);
        assert_eq!(r.status.phase, DISCONNECTING);
        let t = token(&r, s, 2);
        complete(&mut r, t, BACKEND_COMPLETE);
        assert_eq!(r.status.phase, IDLE);
    }
}

#[cfg(test)]
mod authentication_tests {
    use super::{auth::*, core::*, mock::MockBackend};
    use scarlet_abi::wireless::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    const PEER: [u8; 6] = [2, 0, 0, 0, 0, 1];
    #[derive(Default)]
    struct Port {
        inbox: Mutex<EapolInbox>,
        blocks: AtomicUsize,
        tx: AtomicUsize,
        crypto: Mutex<Option<super::ccmp::KeyRing>>,
    }
    impl AuthenticationPort for Port {
        fn block(&self) -> Result<(), i32> {
            self.blocks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn install_ccmp(&self, scope: Association, key: &KeyInstall) -> Result<(), i32> {
            self.crypto
                .lock()
                .unwrap()
                .as_mut()
                .ok_or(NOT_SUPPORTED)?
                .install(scope, key)
                .map_err(|_| FAILED)
        }
        fn replace_group_ccmp(&self, scope: Association, key: &KeyInstall) -> Result<(), i32> {
            self.crypto
                .lock()
                .unwrap()
                .as_mut()
                .ok_or(NOT_SUPPORTED)?
                .replace_group(scope, key)
                .map_err(|_| FAILED)
        }
        fn retire_ccmp(&self) -> Result<(), i32> {
            self.crypto
                .lock()
                .unwrap()
                .as_mut()
                .ok_or(NOT_SUPPORTED)?
                .clear();
            Ok(())
        }
        fn tx_eapol(&self, _: Association, _: &Eapol) -> Result<(), i32> {
            self.tx.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn peek_eapol(&self, _: Association) -> Result<Option<Eapol>, i32> {
            Ok(self.inbox.lock().unwrap().peek())
        }
        fn consume_eapol(&self, _: Association, seq: u64) {
            self.inbox.lock().unwrap().consume(seq);
        }
        fn eapol_state(&self) -> (u64, usize, bool) {
            let q = self.inbox.lock().unwrap();
            (q.sequence, q.len(), q.fault)
        }
    }
    fn call(r: &mut Radio, s: u64, op: u32, payload: &[u8], capacity: usize, id: u64) -> Reply {
        let len = capacity.max(payload.len());
        let mut bytes = vec![0; len];
        bytes[..payload.len()].copy_from_slice(payload);
        let h = Header {
            request_id: id,
            link_id: r.link_id,
            device_epoch: r.device_epoch,
            connection_id: r.connection_id,
            expected_state_seq: r.state_seq,
            ..Header::new(HEADER_SIZE + len)
        };
        r.command(s, op, h, &bytes, 0)
    }
    fn step(r: &mut Radio, s: u64, request: u64, action: u32) {
        let token = Token {
            session: s,
            request,
            epoch: r.device_epoch,
            connection: r.connection_id,
        };
        assert_eq!(
            call(
                r,
                s,
                MOCK_STEP,
                MockStep {
                    token,
                    step: action,
                    reserved: 0
                }
                .bytes(),
                0,
                0
            )
            .header
            .result,
            OK
        );
        r.pump(1);
    }
    fn setup() -> (Radio, u64, Arc<Port>) {
        let mut r = Radio::new(
            42,
            [0; 32],
            Box::new(MockBackend::new(MLME_FIRMWARE, false).with_authentication()),
        );
        let port = Arc::new(Port::default());
        r.attach_authentication_port(port.clone());
        let s = r.open().unwrap();
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        start(&mut r, s, 1);
        (r, s, port)
    }
    fn start(r: &mut Radio, s: u64, id: u64) {
        let mut c = Connect {
            ssid_len: 1,
            akm: AKM_EXTERNAL,
            timeout_ms: 15000,
            ..Connect::default()
        };
        c.ssid[0] = b'x';
        assert_eq!(
            call(r, s, CONNECT, c.bytes(), 0, id).header.result,
            ACCEPTED
        );
        step(r, s, id, STEP_ASSOCIATED);
        assert_eq!(r.status.phase, ASSOCIATED);
    }
    fn key(kind: u32, version: u64, value: u8) -> KeyInstall {
        let mut k = KeyInstall {
            version,
            cipher: CIPHER_CCMP128,
            kind,
            index: if kind == KEY_PAIRWISE { 0 } else { 1 },
            key_len: 16,
            peer: if kind == KEY_PAIRWISE { PEER } else { [255; 6] },
            ..KeyInstall::default()
        };
        k.key[..16].fill(value);
        k
    }
    fn info(r: &mut Radio, s: u64, id: u64) -> RequestInfo {
        RequestInfo::decode(&call(r, s, GET_REQUEST, Id { id }.bytes(), 32, 0).body).unwrap()
    }
    fn backend(r: &mut Radio, s: u64) -> MockSecurityStats {
        MockSecurityStats::decode(&call(r, s, MOCK_SECURITY_STATS, &[], 16, 0).body).unwrap()
    }
    fn install(r: &mut Radio, s: u64, id: u64, k: KeyInstall) {
        assert_eq!(
            call(r, s, INSTALL_KEY, k.bytes(), 0, id).header.result,
            ACCEPTED
        );
        step(r, s, id, STEP_SUCCESS);
        assert_eq!(info(r, s, id).result, OK);
    }
    fn permit(r: &mut Radio, s: u64) -> i32 {
        call(
            r,
            s,
            SET_CONTROLLED_PORT,
            PortControl {
                key_revision: r.security_status().key_revision,
                authorized: 1,
                reserved: 0,
            }
            .bytes(),
            0,
            0,
        )
        .header
        .result
    }
    fn pdu() -> Eapol {
        let mut f = Eapol {
            peer: PEER,
            length: 8,
            ..Eapol::default()
        };
        f.payload[..8].copy_from_slice(&[2, 3, 0, 4, 1, 2, 3, 4]);
        f
    }

    #[test]
    fn authorization_requires_confirmed_pairwise_and_group_keys() {
        let (mut r, s, port) = setup();
        assert_eq!(permit(&mut r, s), BUSY);
        assert_eq!(
            call(
                &mut r,
                s,
                INSTALL_KEY,
                key(KEY_PAIRWISE, 1, 0x11).bytes(),
                0,
                2
            )
            .header
            .result,
            ACCEPTED
        );
        assert_eq!(r.security_status().keys_pending, 1);
        assert_eq!(backend(&mut r, s).installed, 0);
        assert_eq!(permit(&mut r, s), BUSY);
        step(&mut r, s, 2, STEP_SUCCESS);
        assert_eq!(r.security_status().keys_installed, 1);
        assert_eq!(permit(&mut r, s), BUSY);
        install(&mut r, s, 3, key(KEY_GROUP, 1, 0x22));
        assert_eq!(backend(&mut r, s).installed, 2);
        // A backend CONNECT success cannot bypass the controlled-port API.
        step(&mut r, s, 1, STEP_SUCCESS);
        assert_eq!(r.status.phase, ASSOCIATED);
        assert_eq!(info(&mut r, s, 1).result, ACCEPTED);
        assert_eq!(permit(&mut r, s), OK);
        r.complete_data_authorization(true, 2);
        assert_eq!(info(&mut r, s, 1).result, OK);
        assert!(port.blocks.load(Ordering::SeqCst) >= 2);
    }
    #[test]
    fn deleting_an_active_key_closes_data_before_firmware_confirmation() {
        let (mut r, s, port) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        install(&mut r, s, 3, key(KEY_GROUP, 1, 2));
        assert_eq!(permit(&mut r, s), OK);
        r.complete_data_authorization(true, 2);
        let revision = r.security_status().key_revision;
        let blocks = port.blocks.load(Ordering::SeqCst);
        let reference = KeyRef {
            kind: KEY_PAIRWISE,
            index: 0,
            version: 1,
        };
        assert_eq!(
            call(&mut r, s, DELETE_KEY, reference.bytes(), 0, 4)
                .header
                .result,
            ACCEPTED
        );
        assert_eq!(r.status.phase, ASSOCIATED);
        assert!(port.blocks.load(Ordering::SeqCst) > blocks);
        assert_eq!(backend(&mut r, s).installed, 2);
        assert_eq!(permit(&mut r, s), BUSY);
        step(&mut r, s, 4, STEP_SUCCESS);
        assert_eq!(backend(&mut r, s).installed, 1);
        assert_eq!(permit(&mut r, s), BUSY);
        assert_eq!(
            call(
                &mut r,
                s,
                SET_CONTROLLED_PORT,
                PortControl {
                    key_revision: revision,
                    authorized: 1,
                    reserved: 0
                }
                .bytes(),
                0,
                0
            )
            .header
            .result,
            STATE_CHANGED
        );
    }
    #[test]
    fn same_temporal_key_and_obsolete_versions_cannot_reset_replay_counters() {
        let (mut r, s, _) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        assert_eq!(
            call(
                &mut r,
                s,
                INSTALL_KEY,
                key(KEY_PAIRWISE, 2, 1).bytes(),
                0,
                3
            )
            .header
            .result,
            STATE_CHANGED
        );
        assert_eq!(
            call(
                &mut r,
                s,
                INSTALL_KEY,
                key(KEY_PAIRWISE, 1, 3).bytes(),
                0,
                3
            )
            .header
            .result,
            STATE_CHANGED
        );
        let reference = KeyRef {
            kind: KEY_PAIRWISE,
            index: 0,
            version: 1,
        };
        assert_eq!(
            call(&mut r, s, DELETE_KEY, reference.bytes(), 0, 3)
                .header
                .result,
            ACCEPTED
        );
        step(&mut r, s, 3, STEP_SUCCESS);
        assert_eq!(
            call(
                &mut r,
                s,
                INSTALL_KEY,
                key(KEY_PAIRWISE, 2, 1).bytes(),
                0,
                4
            )
            .header
            .result,
            STATE_CHANGED
        );
        install(&mut r, s, 4, key(KEY_PAIRWISE, 2, 3));
        assert_eq!(
            call(&mut r, s, DELETE_KEY, reference.bytes(), 0, 5)
                .header
                .result,
            NOT_FOUND
        );
    }
    #[test]
    fn disconnect_retires_pending_and_installed_keys_and_fences_late_completion() {
        let (mut r, s, _) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        assert_eq!(
            call(&mut r, s, INSTALL_KEY, key(KEY_GROUP, 1, 2).bytes(), 0, 3)
                .header
                .result,
            ACCEPTED
        );
        let old = Token {
            session: s,
            request: 3,
            epoch: r.device_epoch,
            connection: r.connection_id,
        };
        assert_eq!(
            call(&mut r, s, DISCONNECT, Scan::default().bytes(), 0, 4)
                .header
                .result,
            ACCEPTED
        );
        assert_eq!(info(&mut r, s, 3).result, CANCELED);
        assert_eq!(backend(&mut r, s).installed, 0);
        assert_eq!(backend(&mut r, s).pending, 0);
        step(&mut r, s, 4, STEP_SUCCESS);
        start(&mut r, s, 5);
        call(
            &mut r,
            s,
            MOCK_STEP,
            MockStep {
                token: old,
                step: STEP_SUCCESS,
                reserved: 0,
            }
            .bytes(),
            0,
            0,
        );
        r.pump(1);
        assert_eq!(r.security_status().keys_installed, 0);
        assert_eq!(permit(&mut r, s), BUSY);
        let h = Header {
            link_id: r.link_id,
            device_epoch: old.epoch,
            connection_id: old.connection,
            ..Header::new(80)
        };
        assert_eq!(
            r.command(s, SET_CONTROLLED_PORT, h, PortControl::default().bytes(), 1)
                .header
                .result,
            STALE_CONNECTION
        );
        assert_eq!(backend(&mut r, s).installed, 0);
    }
    #[test]
    fn failed_key_completion_and_timeout_both_abort_and_retire_authentication() {
        for timeout in [false, true] {
            let (mut r, s, _) = setup();
            install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
            call(&mut r, s, INSTALL_KEY, key(KEY_GROUP, 1, 2).bytes(), 0, 3);
            if timeout {
                r.pump(6_000_000_000);
            } else {
                step(&mut r, s, 3, STEP_FAILURE);
            }
            assert_eq!(r.status.phase, IDLE);
            assert_eq!(r.connection_id, 0);
            assert_eq!(
                info(&mut r, s, 3).result,
                if timeout { TIMED_OUT } else { FAILED }
            );
            assert_eq!(backend(&mut r, s).installed, 0);
            assert_eq!(backend(&mut r, s).pending, 0);
        }
    }
    #[test]
    fn backend_retirement_failure_blocks_reconnect_until_confirmed_cleanup() {
        let (mut r, s, _) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        step(&mut r, s, 0, STEP_RETIRE_FAILURE);
        call(&mut r, s, DISCONNECT, Scan::default().bytes(), 0, 3);
        step(&mut r, s, 3, STEP_SUCCESS);
        assert_ne!(r.security_status().flags & SECURITY_RETIRE_FAILED, 0);
        assert_eq!(backend(&mut r, s).installed, 1);
        let mut c = Connect {
            ssid_len: 1,
            akm: AKM_EXTERNAL,
            ..Connect::default()
        };
        c.ssid[0] = 1;
        assert_eq!(
            call(&mut r, s, CONNECT, c.bytes(), 0, 4).header.result,
            FAILED
        );
        step(&mut r, s, 0, STEP_RETIRE_RECOVER);
        start(&mut r, s, 4);
        assert_eq!(backend(&mut r, s).installed, 0);
    }
    #[test]
    fn eapol_receive_is_owner_only_and_consumes_after_successful_copyout() {
        let (mut r, s, port) = setup();
        port.inbox.lock().unwrap().push(pdu());
        r.pump(1);
        let observer = r.open().unwrap();
        assert_eq!(
            call(&mut r, observer, RX_EAPOL, &[], 1048, 0).header.result,
            NOT_OWNER
        );
        let short = call(&mut r, s, RX_EAPOL, &[], 0, 0);
        assert_eq!(short.header.result, BUFFER_TOO_SMALL);
        r.commit(s, &short);
        let reply = call(&mut r, s, RX_EAPOL, &[], 1048, 0);
        assert_eq!(reply.header.result, OK);
        let frame = Eapol::decode(&reply.body).unwrap();
        assert_eq!(frame.sequence, 1);
        // Failed copyout: leave reply uncommitted and query the same frame again.
        let retry = call(&mut r, s, RX_EAPOL, &[], 1048, 0);
        assert_eq!(Eapol::decode(&retry.body).unwrap().sequence, 1);
        r.commit(s, &retry);
        assert_eq!(
            call(&mut r, s, RX_EAPOL, &[], 1048, 0).header.result,
            WOULD_BLOCK
        );
        let mut invalid = pdu();
        invalid.peer[5] = 9;
        assert_eq!(
            call(&mut r, s, TX_EAPOL, invalid.bytes(), 0, 0)
                .header
                .result,
            INVALID
        );
        assert_eq!(
            call(&mut r, s, TX_EAPOL, pdu().bytes(), 0, 0).header.result,
            OK
        );
        assert_eq!(port.tx.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn authentication_queue_or_controller_event_loss_aborts_instead_of_resuming() {
        for event_overflow in [false, true] {
            let (mut r, s, port) = setup();
            call(&mut r, s, SUBSCRIBE_EVENTS, &[], 0, 0);
            install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
            if event_overflow {
                for _ in 0..70 {
                    port.inbox.lock().unwrap().push(pdu());
                    r.pump(1);
                    let seq = port.inbox.lock().unwrap().sequence;
                    port.inbox.lock().unwrap().consume(seq);
                }
            } else {
                for _ in 0..=MAX_EAPOL_QUEUE {
                    port.inbox.lock().unwrap().push(pdu());
                }
                r.pump(1);
            }
            assert_eq!(r.status.phase, IDLE);
            assert_eq!(backend(&mut r, s).installed, 0);
        }
    }
    #[test]
    fn malformed_keys_do_not_consume_request_ids_or_close_the_port() {
        let (mut r, s, port) = setup();
        for which in 0..5 {
            let mut k = key(KEY_PAIRWISE, 1, 1);
            match which {
                0 => k.key_len = 32,
                1 => k.reserved[0] = 1,
                2 => k.rx_sequence[7] = 1,
                3 => k.peer = [255; 6],
                _ => k.key[31] = 1,
            };
            assert_eq!(
                call(&mut r, s, INSTALL_KEY, k.bytes(), 0, 2).header.result,
                INVALID
            );
        }
        assert_eq!(port.blocks.load(Ordering::SeqCst), 0);
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
    }
    #[test]
    fn connect_failure_cancel_and_deadline_retire_all_outstanding_key_work() {
        for mode in 0..3 {
            let (mut r, s, _) = setup();
            install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
            // Install close to the CONNECT deadline, with its own later deadline.
            let h = Header {
                request_id: 3,
                link_id: r.link_id,
                device_epoch: r.device_epoch,
                connection_id: r.connection_id,
                ..Header::new(HEADER_SIZE + 72)
            };
            assert_eq!(
                r.command(
                    s,
                    INSTALL_KEY,
                    h,
                    key(KEY_GROUP, 1, 2).bytes(),
                    14_000_000_000
                )
                .header
                .result,
                ACCEPTED
            );
            match mode {
                0 => step(&mut r, s, 1, STEP_FAILURE),
                1 => {
                    call(&mut r, s, CANCEL_REQUEST, Id { id: 1 }.bytes(), 0, 0);
                }
                _ => r.pump(15_000_000_000),
            }
            assert_eq!(r.connection_id, 0);
            assert_ne!(info(&mut r, s, 3).result, ACCEPTED);
            assert_eq!(backend(&mut r, s).installed, 0);
            assert_eq!(backend(&mut r, s).pending, 0);
        }
    }
    #[test]
    fn explicit_port_revocation_invalidates_old_grants_without_deleting_keys() {
        let (mut r, s, _) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        install(&mut r, s, 3, key(KEY_GROUP, 1, 2));
        assert_eq!(permit(&mut r, s), OK);
        r.complete_data_authorization(true, 1);
        let revision = r.security_status().key_revision;
        assert_eq!(
            call(
                &mut r,
                s,
                SET_CONTROLLED_PORT,
                PortControl {
                    key_revision: revision,
                    authorized: 0,
                    reserved: 0
                }
                .bytes(),
                0,
                0
            )
            .header
            .result,
            OK
        );
        assert_eq!(r.status.phase, ASSOCIATED);
        assert_eq!(backend(&mut r, s).installed, 2);
        assert_eq!(
            call(
                &mut r,
                s,
                SET_CONTROLLED_PORT,
                PortControl {
                    key_revision: revision,
                    authorized: 1,
                    reserved: 0
                }
                .bytes(),
                0,
                0
            )
            .header
            .result,
            STATE_CHANGED
        );
        assert_eq!(permit(&mut r, s), OK);
    }
    #[test]
    fn resync_cannot_hide_an_authentication_queue_fault() {
        let (mut r, s, port) = setup();
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        for _ in 0..=MAX_EAPOL_QUEUE {
            port.inbox.lock().unwrap().push(pdu());
        }
        // Deliberately do not pump; the core query itself must honor the latch.
        let snapshot = call(&mut r, s, RESYNC, &[], 56, 0);
        r.commit(s, &snapshot);
        assert_eq!(r.connection_id, 0);
        assert_eq!(backend(&mut r, s).installed, 0);
    }
    #[test]
    fn rsn_association_metadata_is_backend_reported_and_connection_scoped() {
        let mut r = Radio::new(
            42,
            [0; 32],
            Box::new(MockBackend::new(MLME_FIRMWARE, false).with_wpa2()),
        );
        let port = Arc::new(Port::default());
        r.attach_authentication_port(port);
        let s = r.open().unwrap();
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        assert_eq!(
            call(&mut r, s, GET_ASSOCIATION_RSN, &[], 144, 0)
                .header
                .result,
            BUSY
        );
        let mut request = RsnConnect::default();
        request.connect.ssid_len = 1;
        request.connect.ssid[0] = b'x';
        request.connect.akm = AKM_EXTERNAL;
        request.ie_len = 22;
        request.ie[..22].copy_from_slice(&RSN_CCMP_PSK_IE);
        let mut bad = request;
        bad.ie[7] = 2;
        assert_eq!(
            call(&mut r, s, CONNECT_RSN, bad.bytes(), 0, 1)
                .header
                .result,
            INVALID
        );
        assert_eq!(
            call(&mut r, s, CONNECT_RSN, request.bytes(), 0, 1)
                .header
                .result,
            ACCEPTED
        );
        assert_eq!(info(&mut r, s, 1).command, CONNECT_RSN);
        step(&mut r, s, 1, STEP_ASSOCIATED);
        let info = AssociationRsn::decode(&call(&mut r, s, GET_ASSOCIATION_RSN, &[], 144, 0).body)
            .unwrap();
        assert_eq!((info.own_len, info.ap_len, info.peer), (22, 22, PEER));
        assert_eq!(info.own_ie[..22], RSN_CCMP_PSK_IE);
        assert_eq!(info.ap_ie[..22], RSN_CCMP_PSK_IE);
        call(&mut r, s, DISCONNECT, Scan::default().bytes(), 0, 2);
        step(&mut r, s, 2, STEP_SUCCESS);
        assert_eq!(
            call(&mut r, s, GET_ASSOCIATION_RSN, &[], 144, 0)
                .header
                .result,
            BUSY
        );
    }
    #[test]
    fn ordinary_authentication_mock_does_not_claim_rsn_negotiation() {
        let (mut r, s, _) = setup();
        assert_eq!(
            call(&mut r, s, GET_ASSOCIATION_RSN, &[], 144, 0)
                .header
                .result,
            NOT_SUPPORTED
        );
        let mut request = RsnConnect::default();
        request.connect.ssid_len = 1;
        request.connect.ssid[0] = b'x';
        request.connect.akm = AKM_EXTERNAL;
        request.ie_len = 22;
        request.ie[..22].copy_from_slice(&RSN_CCMP_PSK_IE);
        assert_eq!(
            call(&mut r, s, CONNECT_RSN, request.bytes(), 0, 2)
                .header
                .result,
            NOT_SUPPORTED
        );
    }
    fn setup_ccmp() -> (Radio, u64, Arc<Port>) {
        let mut r = Radio::new(
            42,
            [0; 32],
            Box::new(MockBackend::new(MLME_FIRMWARE, false).with_ccmp()),
        );
        let port = Arc::new(Port {
            crypto: Mutex::new(Some(super::ccmp::KeyRing::default())),
            ..Port::default()
        });
        r.attach_authentication_port(port.clone());
        let s = r.open().unwrap();
        call(&mut r, s, CLAIM_CONTROLLER, &[], 0, 0);
        start(&mut r, s, 1);
        install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
        install(&mut r, s, 3, key(KEY_GROUP, 1, 2));
        assert_eq!(permit(&mut r, s), OK);
        r.complete_data_authorization(true, 1);
        (r, s, port)
    }
    #[test]
    fn group_rekey_keeps_authorized_port_while_pending_then_retires_old_slot() {
        let (mut r, s, p) = setup_ccmp();
        let epoch = r.status.link_epoch;
        let blocks = p.blocks.load(Ordering::SeqCst);
        let mut next = key(KEY_GROUP, 1, 3);
        next.index = 2;
        assert_eq!(
            call(&mut r, s, REPLACE_GROUP_KEY, next.bytes(), 0, 4)
                .header
                .result,
            ACCEPTED
        );
        assert_eq!(r.status.phase, AUTHORIZED);
        assert_eq!(r.security_status().keys_pending, 1);
        assert_eq!(r.status.link_epoch, epoch);
        assert_eq!(p.blocks.load(Ordering::SeqCst), blocks);
        assert_eq!(backend(&mut r, s).installed, 2);
        assert_eq!(
            call(&mut r, s, REPLACE_GROUP_KEY, next.bytes(), 0, 5)
                .header
                .result,
            BUSY
        );
        step(&mut r, s, 4, STEP_SUCCESS);
        assert_eq!(info(&mut r, s, 4).result, OK);
        assert_eq!(r.status.phase, AUTHORIZED);
        assert_eq!(
            (
                r.security_status().keys_installed,
                r.security_status().keys_pending
            ),
            (2, 0)
        );
        assert_eq!(backend(&mut r, s).installed, 2);
        assert_eq!(r.status.link_epoch, epoch);
        assert_eq!(
            p.crypto.lock().unwrap().as_ref().unwrap().status().retired,
            1
        );
        assert_eq!(
            call(
                &mut r,
                s,
                REPLACE_GROUP_KEY,
                key(KEY_GROUP, 2, 2).bytes(),
                0,
                5
            )
            .header
            .result,
            STATE_CHANGED
        );
        assert_eq!(p.blocks.load(Ordering::SeqCst), blocks);
    }
    #[test]
    fn failed_canceled_or_timed_out_group_rekey_retires_both_keys() {
        for failure in 0..3 {
            let (mut r, s, p) = setup_ccmp();
            let mut next = key(KEY_GROUP, 1, 3);
            next.index = 2;
            assert_eq!(
                call(&mut r, s, REPLACE_GROUP_KEY, next.bytes(), 0, 4)
                    .header
                    .result,
                ACCEPTED
            );
            match failure {
                0 => step(&mut r, s, 4, STEP_FAILURE),
                1 => {
                    call(&mut r, s, CANCEL_REQUEST, Id { id: 4 }.bytes(), 0, 0);
                }
                _ => r.pump(6_000_000_000),
            }
            assert_eq!(r.connection_id, 0);
            assert_eq!(backend(&mut r, s).installed, 0);
            assert_eq!(backend(&mut r, s).pending, 0);
            assert_eq!(p.crypto.lock().unwrap().as_ref().unwrap().status().keys, 0);
        }
    }
    #[test]
    fn protected_eapol_and_group_rekey_require_explicit_backend_capabilities() {
        let (mut r, s, port) = setup();
        let mut frame = pdu();
        frame.flags = EAPOL_PROTECTED;
        assert_eq!(
            call(&mut r, s, TX_EAPOL, frame.bytes(), 0, 0).header.result,
            NOT_SUPPORTED
        );
        assert_eq!(port.tx.load(Ordering::SeqCst), 0);
        assert_eq!(
            call(
                &mut r,
                s,
                REPLACE_GROUP_KEY,
                key(KEY_GROUP, 1, 9).bytes(),
                0,
                2
            )
            .header
            .result,
            NOT_SUPPORTED
        );
    }
    #[test]
    fn controller_close_and_reset_retire_backend_keys() {
        for reset in [false, true] {
            let (mut r, s, _) = setup();
            install(&mut r, s, 2, key(KEY_PAIRWISE, 1, 1));
            if reset {
                step(&mut r, s, 0, STEP_RESET);
            } else {
                r.close(s, 1);
            }
            assert_eq!(r.connection_id, 0);
            assert_eq!(r.security_status().keys_installed, 0);
            if reset {
                assert_eq!(backend(&mut r, s).installed, 0);
            }
        }
    }
}
