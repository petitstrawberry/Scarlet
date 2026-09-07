//! Native SWS event-wait regression checks using a private protocol peer.
//!
//! Run `/bin/sws-event-wait-smoke` inside Scarlet. No desktop connection or
//! visible window is created; the peer only emits synthetic SWS notifications.

#[cfg(target_os = "scarlet")]
mod native {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use scarlet_os::socket::Socket;
    use sws_client::event::Event;
    use sws_client::{Connection, Error};
    use sws_protocol::{
        MessageHeader, payload_frame_done, payload_sgfx_buffer_released, server_msg,
    };

    const WAIT: Duration = Duration::from_millis(500);
    const EARLY: Duration = Duration::from_millis(250);

    fn send(peer: &Socket, message: u32, payload: &[u8]) {
        let header = MessageHeader::new(message, payload.len() as u32).to_le_bytes();
        let stream = peer.as_stream().expect("peer stream");
        for bytes in [&header[..], payload] {
            let mut offset = 0;
            while offset < bytes.len() {
                let written = stream.write(&bytes[offset..]).expect("peer write");
                assert!(written > 0, "peer closed while writing");
                offset += written;
            }
        }
    }

    /// Run the native event-wait regression scenarios.
    ///
    /// # Returns
    ///
    /// Returns after every assertion passes; a failed scenario aborts the probe.
    pub(super) fn run() {
        println!("[sws-event-wait-smoke] starting");
        let listener = Socket::new().expect("private listener");
        let path = format!("/tmp/sws-event-wait-{}.sock", std::process::id());
        listener.bind(&path).expect("private bind");
        listener.listen(1).expect("private listen");
        let connection = Connection::connect(&path).expect("private connection");
        let peer = Arc::new(listener.accept().expect("private accept"));
        let first = connection.subscribe_window_events(101);
        let second = connection.subscribe_window_events(102);
        let lifecycle = connection.subscribe_sgfx_events(102);

        let started = Instant::now();
        assert!(!connection.wait_for_window_events(Duration::ZERO).unwrap());
        assert!(started.elapsed() < EARLY, "zero timeout blocked");
        let idle = Duration::from_millis(25);
        let started = Instant::now();
        assert!(!connection.wait_for_window_events(idle).unwrap());
        assert!(
            started.elapsed() >= idle,
            "idle wait returned before timeout"
        );
        println!("[sws-event-wait-smoke] PASS zero/idle timeout");

        send(
            &peer,
            server_msg::FRAME_DONE,
            &payload_frame_done(102, 1, 1),
        );
        connection.dispatch().unwrap();
        assert!(!first.has_events());
        assert!(second.has_events());
        let started = Instant::now();
        assert!(connection.wait_for_window_events(WAIT).unwrap());
        assert!(
            started.elapsed() < EARLY,
            "queued second-window event slept"
        );
        assert!(matches!(
            second.poll_event(),
            Some(Event::FrameDone { callback_id: 1, .. })
        ));
        assert!(!connection.wait_for_window_events(Duration::ZERO).unwrap());
        println!("[sws-event-wait-smoke] PASS queued other-window event and rearm");

        let delayed_peer = Arc::clone(&peer);
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            send(
                &delayed_peer,
                server_msg::FRAME_DONE,
                &payload_frame_done(102, 2, 2),
            );
        });
        let started = Instant::now();
        assert!(connection.wait_for_window_events(WAIT).unwrap());
        assert!(
            started.elapsed() < EARLY,
            "socket input did not interrupt wait"
        );
        sender.join().expect("sender");
        connection.dispatch().unwrap();
        assert!(matches!(
            second.poll_event(),
            Some(Event::FrameDone { callback_id: 2, .. })
        ));
        println!("[sws-event-wait-smoke] PASS socket-driven wake");

        // A concurrent reader may empty the socket before the event loop's poll.
        // The producer notification must remain readable until the queued event
        // has been consumed, and the transport mutex must remain available.
        let stop = Arc::new(AtomicBool::new(false));
        let receiver_stop = Arc::clone(&stop);
        let receiver_connection = connection.clone();
        let receiver = thread::spawn(move || {
            while !receiver_stop.load(Ordering::Acquire) {
                receiver_connection.dispatch().expect("concurrent dispatch");
                thread::yield_now();
            }
        });
        for callback in 3..35 {
            assert!(!connection.wait_for_window_events(Duration::ZERO).unwrap());
            let delayed_peer = Arc::clone(&peer);
            let sender = thread::spawn(move || {
                thread::sleep(Duration::from_millis(2));
                send(
                    &delayed_peer,
                    server_msg::FRAME_DONE,
                    &payload_frame_done(102, callback, callback),
                );
            });
            let started = Instant::now();
            assert!(connection.wait_for_window_events(WAIT).unwrap());
            assert!(started.elapsed() < EARLY, "concurrent reader lost a wake");
            sender.join().expect("concurrent sender");
            connection.dispatch().unwrap();
            assert!(
                matches!(second.poll_event(), Some(Event::FrameDone { callback_id, .. }) if callback_id == callback)
            );
        }
        stop.store(true, Ordering::Release);
        receiver.join().expect("concurrent receiver");
        println!("[sws-event-wait-smoke] PASS 32 concurrent-reader wakes");

        send(
            &peer,
            server_msg::SGFX_BUFFER_RELEASED,
            &payload_sgfx_buffer_released(102, 1, 1, 1, 1),
        );
        connection.dispatch().unwrap();
        assert!(lifecycle.has_events());
        let started = Instant::now();
        assert!(!connection.wait_for_window_events(idle).unwrap());
        assert!(
            started.elapsed() >= idle,
            "retained lifecycle event caused a spin"
        );
        assert!(
            lifecycle.poll_event().is_some(),
            "wait consumed sink-owned event"
        );
        println!("[sws-event-wait-smoke] PASS SGFX lifecycle isolation");

        drop(peer);
        assert!(connection.wait_for_window_events(WAIT).unwrap());
        assert_eq!(connection.dispatch(), Err(Error::Disconnected));
        assert_eq!(
            connection.wait_for_window_events(WAIT),
            Err(Error::Disconnected)
        );
        println!("[sws-event-wait-smoke] PASS disconnect");
        println!("[sws-event-wait-smoke] ALL PASS");
    }
}

fn main() {
    #[cfg(target_os = "scarlet")]
    native::run();
    #[cfg(not(target_os = "scarlet"))]
    {
        eprintln!("sws-event-wait-smoke must run inside Scarlet");
        std::process::exit(1);
    }
}
