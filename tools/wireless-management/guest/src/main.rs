#![no_std]
#![no_main]
extern crate scarlet_std as std;
use scarlet_os::{
    poll::{self, POLLIN, PollHandle},
    wireless::*,
};
use std::{println, thread, time::Duration};

fn completed(session: &mut WirelessSession, id: u64) -> RequestInfo {
    for _ in 0..100 {
        let result = session.request(id).unwrap();
        if result.result != ACCEPTED {
            return result;
        }
        let mut handles = [PollHandle::new(session.handle().as_raw() as u32, POLLIN)];
        poll::poll(&mut handles, 100_000_000).unwrap();
        while let Some(event) = session.next_event().unwrap() {
            assert_ne!(event.kind, EVENT_OVERFLOW);
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("wireless completion timed out: {}", id);
}
fn test_radio(index: usize, mlme: u32) {
    let path = std::format!("/dev/wifi{index}");
    let mut session = WirelessSession::open(&path).unwrap();
    assert_eq!(session.capabilities().unwrap().mlme, mlme);
    assert_eq!(session.capabilities().unwrap().features, FEATURE_MOCK);
    session.claim_controller().unwrap();
    session.subscribe().unwrap();
    let mut contender = WirelessSession::open(&path).unwrap();
    contender.subscribe().unwrap();
    assert_eq!(
        contender.claim_controller(),
        Err(WirelessError::Operation(BUSY))
    );
    let mut handles = [PollHandle::new(session.handle().as_raw() as u32, POLLIN)];
    assert_eq!(poll::poll(&mut handles, 0).unwrap(), 0);

    let scan = session.scan(5000).unwrap();
    // Completion arrives via a worker, not from a query. Wait before querying.
    assert_eq!(poll::poll(&mut handles, 1_000_000_000).unwrap(), 1);
    assert_ne!(handles[0].revents & POLLIN, 0);
    let short = session.exchange(DRAIN_EVENTS, &[], 0, 0).unwrap();
    assert_eq!(short.header.result, BUFFER_TOO_SMALL);
    let scan_result = completed(&mut session, scan);
    assert_eq!(scan_result.result, OK);
    let (bss, next) = session.scan_result(scan_result.object_id, 0).unwrap();
    let bss = bss.unwrap();
    assert_eq!(&bss.ssid[..bss.ssid_len as usize], b"ScarletMock");
    assert_eq!(
        session.scan_result(scan_result.object_id, next).unwrap().0,
        None
    );
    session.release_scan(scan_result.object_id).unwrap();
    session.acknowledge(scan).unwrap();
    session.resync().unwrap();

    let connect = session.connect_open(&[0xff, 0, 42], 5000).unwrap();
    assert_eq!(completed(&mut session, connect).result, OK);
    assert_eq!(session.status().unwrap().phase, AUTHORIZED);
    let link_id = session.identity().link_id;
    let links = scarlet_os::network::list_links().unwrap();
    let link = links.iter().find(|l| l.id == link_id).unwrap();
    assert_eq!(link.kind, scarlet_abi::network::LINK_KIND_WIFI);
    assert!(!link.is_up(), "control-only mock must not start DHCP");
    let connection = session.identity().connection_id;
    let epoch = session.identity().device_epoch;
    let disconnect = session.disconnect(5000).unwrap();
    assert_eq!(completed(&mut session, disconnect).result, OK);
    session
        .mock_step(connect, connection, epoch, STEP_SUCCESS)
        .unwrap();
    thread::sleep(Duration::from_millis(10));
    assert_eq!(session.status().unwrap().phase, IDLE);

    let canceled = session.scan(5000).unwrap();
    session.cancel(canceled).unwrap();
    assert_eq!(session.request(canceled).unwrap().result, CANCELED);
    // A reset is observable through old handles; sanitized queries recover the epoch.
    session.mock_step(0, 0, epoch, STEP_RESET).unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(session.status().unwrap().phase, IDLE);
    assert!(session.identity().device_epoch > epoch);
    session.resync().unwrap();
    for _ in 0..70 {
        let id = session.scan(5000).unwrap();
        session.cancel(id).unwrap();
        session.acknowledge(id).unwrap();
    }
    let overflow = session.next_event().unwrap().unwrap();
    assert_eq!(overflow.kind, EVENT_OVERFLOW);
    let snapshot = session.resync().unwrap();
    assert_eq!(snapshot.event_sequence, overflow.sequence);
    assert!(session.next_event().unwrap().is_none());
    assert_eq!(
        contender.next_event().unwrap().unwrap().kind,
        EVENT_OVERFLOW
    );
    let next = session.scan(5000).unwrap();
    session.cancel(next).unwrap();
    assert_eq!(
        session.next_event().unwrap().unwrap().sequence,
        snapshot.event_sequence + 1
    );

    // Closing one duplicate does not release the writer; closing the last does.
    let duplicate = session.handle().duplicate().unwrap();
    drop(session);
    contender.status().unwrap(); // refresh this observer after the earlier reset
    assert_eq!(
        contender.claim_controller(),
        Err(WirelessError::Operation(BUSY))
    );
    drop(duplicate);
    contender.status().unwrap();
    contender.claim_controller().unwrap();
    let epoch = contender.identity().device_epoch;
    contender.mock_step(0, 0, epoch, STEP_REMOVE).unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(contender.status().unwrap().availability, RADIO_REMOVED);
    assert!(WirelessSession::open(&path).is_err());
    println!("WIRELESS_QA RADIO {} PASS", index);
}
#[unsafe(no_mangle)]
fn main() -> i32 {
    test_radio(0, MLME_FIRMWARE);
    test_radio(1, MLME_HOST);
    println!("WIRELESS_QA PASS");
    std::task::shutdown(std::task::ShutdownType::PowerOff);
}
