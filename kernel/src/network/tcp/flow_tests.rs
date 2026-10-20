use super::*;

#[test_case]
fn deferred_receive_notifications_coalesce_until_batch_end() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::Established);
    let peer = Ipv4Address::new(192, 0, 2, 1);
    socket.recv_ack.store(100, Ordering::SeqCst);
    let recv = Arc::new(crate::sync::Waker::new_interruptible("tcp-deferred-recv"));
    *socket.recv_waker.lock() = Some(recv.clone());

    begin_rx_batch();
    socket.notify_received_data(peer, 80, 100);
    socket.notify_received_data(peer, 80, 200);
    assert_eq!(recv.pending_wake_count_for_test(), 0);
    assert!(socket.rx_notify_pending.load(Ordering::SeqCst));

    flush_rx_batch();
    assert_eq!(recv.pending_wake_count_for_test(), 1);
    assert!(!socket.rx_notify_pending.load(Ordering::SeqCst));
    end_rx_batch();
}

#[test_case]
fn advancing_acks_coalesce_inside_driver_rx_batch() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::Established);
    socket.send_unacked.store(1000, Ordering::SeqCst);
    socket.send_seq.store(3000, Ordering::SeqCst);
    socket
        .send_buffer
        .lock()
        .extend(core::iter::repeat(0x5a).take(2000));
    let mut first = TcpHeader::new(80, 30000);
    first.set_flags(tcp_flags::ACK);
    first.ack_number = 1500;
    first.seq_number = 1;
    let mut second = first;
    second.ack_number = 2500;
    second.seq_number = 2;

    begin_rx_batch();
    socket.process_ack(&first, false);
    socket.process_ack(&second, false);
    assert_eq!(socket.send_unacked.load(Ordering::SeqCst), 1000);
    assert_eq!(socket.send_buffer.lock().len(), 2000);
    assert!(socket.ack_input_pending.load(Ordering::SeqCst));

    flush_ack_input();
    assert_eq!(socket.send_unacked.load(Ordering::SeqCst), 2500);
    assert_eq!(socket.send_buffer.lock().len(), 500);
    assert!(!socket.ack_input_pending.load(Ordering::SeqCst));
    end_rx_batch();
}

#[test_case]
fn unchanged_receive_acks_do_not_broadcast_write_or_poll_wakes() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::Established);
    socket.send_unacked.store(1000, Ordering::SeqCst);
    socket.send_seq.store(2000, Ordering::SeqCst);
    let send = Arc::new(crate::sync::Waker::new_interruptible("tcp-test-send"));
    let poll = Arc::new(crate::sync::Waker::new_interruptible("tcp-test-poll"));
    *socket.send_waker.lock() = Some(send.clone());
    *socket.recv_waker.lock() = Some(poll.clone());
    let mut header = TcpHeader::new(80, 30000);
    header.set_flags(tcp_flags::ACK);
    header.ack_number = 1000;
    for sequence in 1..=64 {
        header.seq_number = sequence;
        socket.process_ack(&header, false);
    }
    assert_eq!(send.pending_wake_count_for_test(), 0);
    assert_eq!(poll.pending_wake_count_for_test(), 0);
    // New outbound acknowledgement must still notify both kinds of waiter.
    header.ack_number = 1500;
    socket.process_ack(&header, false);
    assert_eq!(socket.send_unacked.load(Ordering::SeqCst), 1500);
    assert_eq!(send.pending_wake_count_for_test(), 1);
    assert_eq!(poll.pending_wake_count_for_test(), 1);
}

#[test_case]
fn window_only_and_close_ack_changes_still_wake_write_and_poll_waiters() {
    for close in [false, true] {
        let socket = TcpSocket::new(Weak::new());
        socket.set_state(if close {
            TcpState::LastAck
        } else {
            TcpState::Established
        });
        socket.send_unacked.store(1000, Ordering::SeqCst);
        socket.send_seq.store(1000, Ordering::SeqCst);
        let send = Arc::new(crate::sync::Waker::new_interruptible("tcp-test-send"));
        let poll = Arc::new(crate::sync::Waker::new_interruptible("tcp-test-poll"));
        *socket.send_waker.lock() = Some(send.clone());
        *socket.recv_waker.lock() = Some(poll.clone());
        if !close {
            socket.peer_window.lock().update(0, 1000, 0);
        }
        let mut header = TcpHeader::new(80, 30000);
        header.set_flags(tcp_flags::ACK);
        header.ack_number = 1000;
        header.seq_number = 1;
        socket.process_ack(&header, false);
        assert_eq!(send.pending_wake_count_for_test(), 1);
        assert_eq!(poll.pending_wake_count_for_test(), 1);
        if close {
            assert_eq!(socket.get_state(), TcpState::Closed);
        } else {
            assert!(socket.peer_send_credit() > 0);
        }
    }
}

#[test_case]
fn window_scale_options_handle_zero_limits_unknowns_and_truncation() {
    for shift in [0, 4, 14, 15, 255] {
        let bytes = [TCP_OPTION_NOP, 3, 3, shift, 0];
        assert_eq!(parse_tcp_window_scale_option(&bytes), Some(shift.min(14)));
    }
    for bytes in [&[3][..], &[3, 1, 9], &[3, 4, 9], &[3, 2], &[0, 3, 3, 4]] {
        assert_eq!(parse_tcp_window_scale_option(bytes), None);
    }
    assert_eq!(parse_tcp_window_scale_option(&[99, 2, 3, 3, 7]), Some(7));
    let (options, length) = tcp_syn_options(Some(TCP_LOCAL_WINDOW_SHIFT));
    assert_eq!(length, 8);
    assert_eq!(
        parse_tcp_mss_option(&options[..length]),
        Some(TCP_LOCAL_MSS as u16)
    );
    assert_eq!(
        parse_tcp_window_scale_option(&options[..length]),
        Some(TCP_LOCAL_WINDOW_SHIFT)
    );
    let (options, length) = tcp_syn_options(None);
    assert_eq!(length, 4);
    assert_eq!(parse_tcp_window_scale_option(&options[..length]), None);
}

#[test_case]
fn scaled_receive_advertisements_preserve_capacity_and_maximum_offered_edge() {
    let socket = TcpSocket::new(Weak::new());
    socket.negotiate_window_scaling(Some(0));
    assert!(socket.window_scaling.load(Ordering::SeqCst));
    assert_eq!(socket.wire_receive_window(true), 65535);
    assert!(socket.wire_receive_window(false) as u32 * 16 > 65535);
    let mut ack = u32::MAX - 1000;
    let mut free = MAX_RECV_BUFFER_SIZE as u32;
    let mut previous = None;
    for count in [0, 1460, 1460, 1460, 1, 15, 17, 100] {
        ack = ack.wrapping_add(count);
        free -= count;
        socket.recv_ack.store(ack, Ordering::SeqCst);
        socket.recv_window.store(free, Ordering::SeqCst);
        let wire = socket.wire_receive_window(false);
        let bytes = (wire as u32) << TCP_LOCAL_WINDOW_SHIFT;
        assert!(bytes <= free);
        let edge = ack.wrapping_add(bytes);
        if let Some(old) = previous {
            if seq_before(edge, old) {
                assert!(old.wrapping_sub(edge) < 1 << TCP_LOCAL_WINDOW_SHIFT);
            }
        }
        let mut header = TcpHeader::new(1, 2);
        header.set_flags(tcp_flags::ACK);
        header.ack_number = ack;
        header.window_size = wire;
        socket.record_advertised_recv_window(&header);
        previous = Some(previous.map_or(edge, |old| if seq_after(edge, old) { edge } else { old }));
        assert_eq!(*socket.advertised_recv_edge.lock(), previous);
    }
    socket.recv_window.store(0, Ordering::SeqCst);
    socket.recv_ack.store(previous.unwrap(), Ordering::SeqCst);
    assert_eq!(socket.wire_receive_window(false), 0);
    assert!(socket.update_recv_window_after_drain(MAX_RECV_BUFFER_SIZE - 16384));
    socket.negotiate_window_scaling(None);
    socket
        .recv_window
        .store(MAX_RECV_BUFFER_SIZE as u32, Ordering::SeqCst);
    assert_eq!(socket.wire_receive_window(false), 65535);
}

#[test_case]
fn peer_window_updates_reject_old_packets_and_handle_sequence_wrap() {
    let mut window = PeerWindow::new();
    assert!(window.update(u32::MAX - 2, 100, 8192));
    assert!(!window.update(u32::MAX - 3, 200, 0));
    assert_eq!(window.bytes, 8192);
    assert!(window.update(2, 200, 0));
    assert!(!window.update(2, 199, 1024));
    assert_eq!(window.bytes, 0);
    assert!(window.update(2, 200, 1024));
}

#[test_case]
fn sending_credit_and_readiness_follow_scaled_peer_window_and_valid_acks() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::Established);
    socket.negotiate_window_scaling(Some(2));
    socket.send_unacked.store(u32::MAX - 100, Ordering::SeqCst);
    socket.send_seq.store(50, Ordering::SeqCst);
    let mut header = TcpHeader::new(1, 2);
    header.set_flags(tcp_flags::ACK);
    header.seq_number = 1000;
    header.ack_number = u32::MAX - 100;
    header.window_size = 100;
    socket.process_ack(&header, true);
    assert_eq!(socket.peer_send_credit(), 400 - 151);
    header.window_size = 0;
    socket.process_ack(&header, true);
    assert_eq!(socket.writable_capacity(), 0);
    // No remote destination is required to observe readiness and timer arming.
    socket.schedule_persist_timer();
    assert!(socket.persist_timer.lock().is_some());
    // An ACK of bytes never transmitted cannot open the window.
    header.ack_number = 51;
    header.window_size = 65535;
    socket.process_ack(&header, true);
    assert_eq!(socket.peer_window.lock().bytes, 0);
    assert_eq!(socket.send_unacked.load(Ordering::SeqCst), u32::MAX - 100);
    header.ack_number = 50;
    header.window_size = 100;
    socket.process_ack(&header, true);
    assert_eq!(socket.peer_send_credit(), 400);
    assert!(socket.persist_timer.lock().is_none());
}

#[test_case]
fn syn_windows_are_unscaled_and_established_connections_do_not_renegotiate() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::SynSent);
    socket.send_seq.store(1001, Ordering::SeqCst);
    socket.send_unacked.store(1000, Ordering::SeqCst);
    let mut syn = TcpHeader::new(80, 30000);
    syn.set_flags(tcp_flags::SYN | tcp_flags::ACK);
    syn.seq_number = 300;
    syn.ack_number = 1001;
    syn.window_size = 200;
    let localhost = Ipv4Address::new(127, 0, 0, 1);
    socket.process_segment(localhost, localhost, syn, &[], Some(1460), Some(8));
    assert_eq!(socket.get_state(), TcpState::Established);
    assert_eq!(socket.peer_window.lock().bytes, 200);
    assert_eq!(socket.peer_window_shift.load(Ordering::SeqCst), 8);
    socket.process_segment(localhost, localhost, syn, &[], Some(1460), Some(1));
    assert_eq!(socket.peer_window_shift.load(Ordering::SeqCst), 8);
    let child = TcpSocket::new(Weak::new());
    syn.set_flags(tcp_flags::SYN);
    syn.ack_number = 0;
    child.handle_syn_received(localhost, syn, Some(1460), None);
    assert!(!child.window_scaling.load(Ordering::SeqCst));
    assert_eq!(child.wire_receive_window(false), 65535);
}

#[test_case]
fn payload_writes_stop_at_the_peer_window_and_resume_after_acknowledgment() {
    use crate::object::capability::Selectable;
    use crate::object::capability::selectable::ReadyInterest;
    let socket = TcpSocket::new(Weak::new());
    // No loopback shortcut/peer: exercise real TCP packet construction and
    // retransmission bookkeeping without emitting external test traffic.
    *socket.local_ip.lock() = Some(Ipv4Address::new(127, 0, 0, 1));
    *socket.remote_ip.lock() = Some(Ipv4Address::new(127, 0, 0, 2));
    socket.local_port.store(30000, Ordering::SeqCst);
    socket.remote_port.store(80, Ordering::SeqCst);
    socket.send_seq.store(1000, Ordering::SeqCst);
    socket.send_unacked.store(1000, Ordering::SeqCst);
    socket.peer_mss.store(16, Ordering::SeqCst);
    socket.peer_window.lock().bytes = 25;
    socket.set_state(TcpState::Established);
    let interest = ReadyInterest {
        read: false,
        write: true,
        except: false,
    };
    assert_eq!(socket.send_data(&[42; 100]), Ok(25));
    assert_eq!(socket.send_seq.load(Ordering::SeqCst), 1025);
    assert_eq!(socket.unacked_segments.lock().len(), 2);
    assert_eq!(socket.send_buffer.lock().len(), 25);
    assert!(!socket.current_ready(interest).write);
    assert_eq!(socket.send_data(&[42; 100]), Err(SocketError::WouldBlock));
    let mut header = TcpHeader::new(80, 30000);
    header.set_flags(tcp_flags::ACK);
    header.seq_number = 400;
    header.ack_number = 1025;
    header.window_size = 10;
    socket.process_ack(&header, true);
    assert!(socket.current_ready(interest).write);
    assert!(socket.unacked_segments.lock().is_empty());
    assert_eq!(socket.send_data(&[43; 100]), Ok(10));
    assert_eq!(socket.send_seq.load(Ordering::SeqCst), 1035);
}

#[test_case]
fn zero_window_probes_preserve_stream_sequence_and_do_not_exhaust_data_retries() {
    let socket = TcpSocket::new(Weak::new());
    *socket.local_ip.lock() = Some(Ipv4Address::new(127, 0, 0, 1));
    *socket.remote_ip.lock() = Some(Ipv4Address::new(127, 0, 0, 2));
    socket.send_seq.store(1000, Ordering::SeqCst);
    socket.send_unacked.store(1000, Ordering::SeqCst);
    socket.peer_window.lock().bytes = 0;
    socket.set_state(TcpState::Established);
    socket.persist_delay_ms.store(200, Ordering::SeqCst);
    socket.probe_zero_window();
    assert_eq!(socket.send_seq.load(Ordering::SeqCst), 1000);
    assert_eq!(socket.send_unacked.load(Ordering::SeqCst), 1000);
    assert!(socket.unacked_segments.lock().is_empty());
    assert_eq!(socket.persist_delay_ms.load(Ordering::SeqCst), 400);
    assert_eq!(socket.retrans_count.load(Ordering::SeqCst), 0);
    socket.cancel_persist_timer();
}

#[test_case]
fn out_of_order_fin_does_not_end_the_stream_before_missing_payload_arrives() {
    let socket = TcpSocket::new(Weak::new());
    socket.set_state(TcpState::Established);
    socket.recv_seq.store(100, Ordering::SeqCst);
    socket.recv_ack.store(100, Ordering::SeqCst);
    let mut header = TcpHeader::new(80, 30000);
    header.seq_number = 105;
    header.set_flags(tcp_flags::FIN);
    let localhost = Ipv4Address::new(127, 0, 0, 1);
    socket.handle_control_segment(localhost, header);
    assert_eq!(socket.get_state(), TcpState::Established);
    assert_eq!(socket.recv_seq.load(Ordering::SeqCst), 100);
    header.seq_number = 100;
    header.set_flags(0);
    socket.handle_data_segment(localhost, header, b"hello");
    assert_eq!(socket.get_state(), TcpState::CloseWait);
    assert_eq!(socket.recv_seq.load(Ordering::SeqCst), 106);
    let mut bytes = [0; 5];
    assert_eq!(socket.recv_data(&mut bytes), Ok(5));
    assert_eq!(&bytes, b"hello");
    assert_eq!(socket.recv_data(&mut bytes), Ok(0));
}

#[test_case]
fn loopback_large_writes_make_partial_progress_and_drains_restore_readiness() {
    use crate::object::capability::Selectable;
    use crate::object::capability::selectable::ReadyInterest;
    let sender = TcpSocket::new(Weak::new());
    let receiver = TcpSocket::new(Weak::new());
    sender.set_state(TcpState::Established);
    receiver.set_state(TcpState::Established);
    *sender.loopback_peer.lock() = Arc::downgrade(&receiver);
    *receiver.loopback_peer.lock() = Arc::downgrade(&sender);
    receiver
        .recv_buffer
        .lock()
        .resize(MAX_RECV_BUFFER_SIZE - 2, 0);
    assert_eq!(sender.send_data(b"test"), Ok(2));
    let interest = ReadyInterest {
        read: false,
        write: true,
        except: false,
    };
    assert!(!sender.current_ready(interest).write);
    let mut bytes = [0; 4];
    assert_eq!(receiver.recv_data(&mut bytes), Ok(4));
    assert!(sender.current_ready(interest).write);
    assert_eq!(sender.send_data(b"test"), Ok(4));
}
