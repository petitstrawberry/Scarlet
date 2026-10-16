# Network performance follow-up

## Outstanding transport work (2026-10-16)

TCP window scaling remains unimplemented. `kernel/src/network/tcp.rs` stores
`recv_window` as `AtomicU16`, caps the advertised window at 65,535 bytes and
has a 128 KiB receive-buffer limit. Increasing that buffer alone does not
increase the window on the wire. This work was explicitly deferred by the
user on 2026-10-16; the HTTP streaming changes do not resolve it.

The next transport change must implement RFC 7323 negotiation on SYN/SYN-ACK,
leave SYN window fields unscaled, preserve unscaled interoperability when the
peer omits the option, and account for scaled bytes in advertised-window edge
tracking and zero-window reopening. Test active/passive handshakes, shift 0,
maximum shift, malformed options, retransmitted SYNs and sequence wrap.

The sender also does not currently use the peer window to throttle payload
transmission. Treat sender flow control and zero-window/persist behavior as
separate correctness work; enlarging receive windows does not fix this.

Other audit findings awaiting implementation/measurement:

- VirtIO-net has a 32-entry RX ring but only 16 posted receive buffers;
  increasing capacity must honor the device maximum and bound IRQ work.
- Resolver queries reread configuration and perform uncached UDP lookups;
  caching must honor DNS TTLs, aliases and configuration changes.
- Separate transport throughput from callbacks running on WebCore's owner
  thread. Whole-response buffering in Myrica delayed delivery until HTTP EOF;
  incremental HTTP delivery is being handled in the Myrica/WebKit repositories.

Reference: [RFC 7323 section 2](https://www.rfc-editor.org/rfc/rfc7323.html#section-2).
