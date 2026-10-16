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

## Incremental delivery validation (2026-10-16)

Normal AArch64 Release bundle: Myrica `ef6cbad3224c374b0e0915ddc991c3b9861eef3c`,
WebKit `9183b1eda1d25edac4cddf24561ccac34316861c`. HTTP identity responses now
cross the embedding as headers, bounded data chunks, then completion. Encoded
responses still use the existing whole-entity decoder. Transport unit tests:
21 passed, including prefix-before-EOF, truncation and mid-stream cancellation.

Native run: HVF, 8 vCPUs, 16 GiB, `virtio-gpu-gl-pci`, Cocoa GL/Retina,
`SCARLET_QEMU_AUDIO=1` and the normal `/dev/video0` VideoToolbox backend.
The previous validation VM omitted AUDIO=1; the runner defaults to no sound
device. SAS audio consumption supplies the Myrica A/V clock, so that previous
launch was unsuitable for validating audio/video playback.

Single-run browser fetch observations to a host-local HTTP server at 10.0.2.2:

| Response | Headers available | Entire body available |
| --- | ---: | ---: |
| 8 KiB | 15 ms | 15 ms |
| 256 KiB | 20 ms | 20 ms |
| 4 MiB | 6 ms | 184 ms |

24 parallel 8 KiB requests completed in 122 ms, with no failed requests.
These observations include WebCore owner-thread scheduling and QEMU's local
user network; they do not measure WAN latency or prove that TCP scaling is fixed.
The cross-origin Google 204 fetch failed CORS and is excluded from throughput
results. The earlier audit's 4 MiB result was 1496 ms under different runtime
conditions; this is not a controlled speedup comparison.

Playback through the ordinary browser:

- The original 24 s, 128x96, 8 fps H.264/AAC fixture reached its end with
  192 decoded and 192 presented frames and no media error.
- An original 12 s, 1280x720, 30 fps H.264 Main/AAC clip reached its end with
  360 decoded, 330 presented and 10 decoder-output drops. The presented and
  dropped counters cover different pipeline stages and do not sum to decoded.
- The actual YouTube watch page `cxvXOVAVwjE` selected native 1280x720 playback.
  At media time 31.68 s, counters were 953 decoded, 696 presented and 58 drops.
  Startup presentation was slower than steady playback; frame drops remain.
  This observation uses native counters, without screenshot/pixel verification.

The incremental C++ embedding and ordinary Myrica compiled successfully.
The standalone C++ smoke target compiled but its CMake executable link lacks
Scarlet CRT/linker setup on this Darwin host; it was not executed. The native
observations above use the actual bundled Myrica process instead.
