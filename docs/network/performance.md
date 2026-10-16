# Network performance follow-up

## Transport implementation (2026-10-17)

The normal Release image now includes the previously deferred transport work:

- TCP offers Window Scale 4 on active SYNs and negotiates it on passive opens
  only when the peer offers the option. SYN/SYN-ACK window fields remain
  unscaled; peers without the option retain the 65,535-byte wire limit.
  Receive capacity is 1 MiB, allocated as bytes arrive. Scaled byte accounting
  covers advertised edges, quantization, sequence wrap and window reopening.
- Payload writes and writable readiness respect the peer's remaining send
  credit. Partial writes resume on acknowledgements/window updates. A separate
  bounded-backoff persist timer probes a zero window without consuming stream
  sequence space or exhausting data retransmission retries. Combined read/write
  waiters are notified by window-only updates.
- A bare FIN received ahead of missing data waits for ordered reassembly before
  reporting EOF.
- VirtIO-net chooses power-of-two RX/TX rings within the device maximum,
  preferring 256/128 entries. Every RX descriptor is posted. IRQ processing
  consumes a bounded snapshot; the software RX queue holds up to 1,024 packets.
- The resolver caches up to 256 validated positive answers for the minimum
  relevant CNAME/A TTL (maximum one hour). Configuration changes clear the
  cache. Eight workers and a 64-request queue replace a thread per request;
  truncated UDP responses fall back to TCP. Configuration is still read for
  each lookup to notice changes immediately.

Kernel AArch64 QEMU tests: **1,495 passed**. Five host-side resolver tests cover
TTL expiry, CNAME minimum TTL, response identity, malformed packets and
configuration invalidation. DNS TCP fallback compiled into the native daemon;
its fallback path has not yet been exercised against a deliberately truncated
native response. TCP still has wider gaps such as congestion control, SACK and
receive checksum validation; this work does not establish full TCP conformance.

Reference: [RFC 7323 section 2](https://www.rfc-editor.org/rfc/rfc7323.html#section-2).

## Native transport validation (2026-10-17)

Normal AArch64 Release image, HVF/8 vCPUs/16 GiB, VirGL PCI, Cocoa GL/Retina,
audio enabled and `/dev/video0`. Myrica `4ae10a8c72d4add1ff874be22a2edb98e4e09acb`,
WebKit `7605d3c5779fcbcae4c503c37953dfaa7f145aa6`, SGFX initially
`07263cb0061a0233b1f86387d80758374dcdf9da`.

A QEMU network filter captured the actual guest packets. Three consecutive
`resolve www.rfc-editor.org` calls returned the same addresses with **one**
external DNS query. Guest SYNs offered Window Scale 4. This QEMU user-network
peer did **not** offer scaling in its SYN-ACK, so the native run exercised the
unscaled interoperability path. Negotiated scaled windows are covered by the
kernel tests above, not established by this peer capture.

A browser POST sent 4 MiB of `0x5a` while the host HTTP server deliberately
stopped reading for ten seconds with a small receive buffer. The capture
contained zero-window advertisements and repeated probes. Transfer resumed,
returned HTTP 200 and the server received exactly 4,194,304 bytes with SHA-256
`4656153f1921ea9f09001428d189084d3db94509dd71990a8a971cfa02998087`,
matching the original bytes. Server elapsed time was 10.176 s; browser wall
time was 11.144 s.

Single-run HTTP observations on the same VM:

| Response | Headers available | Entire body available |
| --- | ---: | ---: |
| 8 KiB | 9 ms | 9 ms |
| 256 KiB | 8 ms | 23 ms |
| 4 MiB | 15 ms | 113 ms |

24 parallel 8 KiB requests completed in 74 ms with no failed requests. These
are host-local browser observations, including embedding scheduling and QEMU
user networking. They are not WAN throughput or controlled before/after
measurements. The cross-origin Google 204 request is excluded because it
fails CORS.

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
