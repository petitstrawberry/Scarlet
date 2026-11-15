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
        MessageHeader, payload_frame_done, payload_sgfx_buffer_released, payload_window_destroyed,
        server_msg,
    };

    const WAIT: Duration = Duration::from_millis(500);
    const EARLY: Duration = Duration::from_millis(250);

    fn socket_and_notification_wait() {
        use scarlet_os::poll::{POLLHUP, POLLIN, PollHandle, poll};

        let listener = Socket::new().expect("multipoll listener");
        let path = format!("/tmp/sws-multipoll-{}.sock", std::process::id());
        listener.bind(&path).expect("multipoll bind");
        listener.listen(1).expect("multipoll listen");
        let socket = Socket::new().expect("multipoll socket");
        socket.connect(&path).expect("multipoll connect");
        let peer = Arc::new(listener.accept().expect("multipoll accept"));
        let (notification, sender) = scarlet_os::ipc::pipe().expect("multipoll pipe");
        let sender = Arc::new(sender);
        let mut handles = [
            PollHandle::new(socket.as_raw() as u32, POLLIN),
            PollHandle::new(notification.as_raw() as u32, POLLIN),
        ];
        assert_eq!(poll(&mut handles, 0).unwrap(), 0);
        // Alternate both sources, already-published events and delayed events.
        // The source not being written must remain quiet through every rearm.
        for iteration in 0..64 {
            let index = iteration % 2;
            let peer = Arc::clone(&peer);
            let sender = Arc::clone(&sender);
            let producer = thread::spawn(move || {
                if iteration % 4 >= 2 {
                    thread::sleep(Duration::from_millis(2));
                }
                let byte = [iteration as u8];
                let written = if index == 0 {
                    peer.as_stream().unwrap().write(&byte).unwrap()
                } else {
                    sender.as_stream().unwrap().write(&byte).unwrap()
                };
                assert_eq!(written, 1);
            });
            assert_eq!(poll(&mut handles, WAIT.as_nanos() as i64).unwrap(), 1);
            assert_ne!(handles[index].revents & POLLIN, 0);
            assert_eq!(handles[1 - index].revents, 0);
            producer.join().expect("multipoll producer");
            let mut byte = [0];
            let count = if index == 0 {
                socket.as_stream().unwrap().read(&mut byte).unwrap()
            } else {
                notification.as_stream().unwrap().read(&mut byte).unwrap()
            };
            assert_eq!(count, 1);
            assert_eq!(byte[0], iteration as u8);
            assert_eq!(poll(&mut handles, 0).unwrap(), 0);
        }
        let idle = Duration::from_millis(25);
        let started = Instant::now();
        assert_eq!(poll(&mut handles, idle.as_nanos() as i64).unwrap(), 0);
        assert!(started.elapsed() >= idle, "multipoll restarted/shortened deadline");
        let closer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(2));
            drop(sender);
        });
        assert_eq!(poll(&mut handles, WAIT.as_nanos() as i64).unwrap(), 1);
        assert_ne!(handles[1].revents & POLLHUP, 0);
        closer.join().expect("multipoll closer");
        println!("[sws-event-wait-smoke] PASS socket+pipe 64 wakes/rearms, idle deadline, pipe close");
    }

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
        socket_and_notification_wait();
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
        assert!(
            !connection
                .wait_for_subscribed_window_events(Duration::ZERO)
                .unwrap()
        );
        assert!(started.elapsed() < EARLY, "zero timeout blocked");
        let idle = Duration::from_millis(25);
        let started = Instant::now();
        assert!(!connection.wait_for_subscribed_window_events(idle).unwrap());
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
        assert!(connection.wait_for_subscribed_window_events(WAIT).unwrap());
        assert!(
            started.elapsed() < EARLY,
            "queued second-window event slept"
        );
        assert!(matches!(
            second.poll_event(),
            Some(Event::FrameDone { callback_id: 1, .. })
        ));
        assert!(
            !connection
                .wait_for_subscribed_window_events(Duration::ZERO)
                .unwrap()
        );
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
        assert!(connection.wait_for_subscribed_window_events(WAIT).unwrap());
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
            assert!(
                !connection
                    .wait_for_subscribed_window_events(Duration::ZERO)
                    .unwrap()
            );
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
            assert!(connection.wait_for_subscribed_window_events(WAIT).unwrap());
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
        assert!(!connection.wait_for_subscribed_window_events(idle).unwrap());
        assert!(
            started.elapsed() >= idle,
            "retained lifecycle event caused a spin"
        );
        assert!(
            lifecycle.poll_event().is_some(),
            "wait consumed sink-owned event"
        );
        println!("[sws-event-wait-smoke] PASS SGFX lifecycle isolation");

        // The app drops its window receiver before the asynchronous destroy
        // reply arrives. That reply belongs to the unclaimed queue; ScarletUI
        // drains only the remaining window subscriptions.
        drop(second);
        drop(lifecycle);
        send(
            &peer,
            server_msg::WINDOW_DESTROYED,
            &payload_window_destroyed(102),
        );
        connection.dispatch().unwrap();
        assert!(connection.has_events());
        assert!(connection.wait_for_window_events(Duration::ZERO).unwrap());
        let started = Instant::now();
        assert!(!connection.wait_for_subscribed_window_events(idle).unwrap());
        assert!(
            started.elapsed() >= idle,
            "closed-window reply caused a spin"
        );
        // An unclaimed event must neither hide a live window's wake nor be
        // consumed by the subscription-only wait.
        send(
            &peer,
            server_msg::FRAME_DONE,
            &payload_frame_done(101, 35, 35),
        );
        connection.dispatch().unwrap();
        assert!(connection.wait_for_subscribed_window_events(WAIT).unwrap());
        assert!(matches!(
            first.poll_event(),
            Some(Event::FrameDone {
                callback_id: 35,
                ..
            })
        ));
        let started = Instant::now();
        assert!(!connection.wait_for_subscribed_window_events(idle).unwrap());
        assert!(started.elapsed() >= idle, "unclaimed queue prevented rearm");
        assert!(matches!(
            connection.poll_event(),
            Some(Event::SurfaceDestroyed { surface_id: 102 })
        ));
        println!("[sws-event-wait-smoke] PASS closed-window isolation and live-window wake");

        drop(peer);
        assert!(connection.wait_for_subscribed_window_events(WAIT).unwrap());
        assert_eq!(connection.dispatch(), Err(Error::Disconnected));
        assert_eq!(
            connection.wait_for_subscribed_window_events(WAIT),
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
