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

## Shared GPU shader capacity and remaining playback failure (2026-10-17)

A longer run after the previous short playback check accumulated 27,890
`ERROR_OUT_OF_DEVICE_MEMORY` pipeline/specialization errors. Shader modules
shared an accidental 256-slot limit with pipeline objects. Multiple Skia
contexts can each retain 64 programs with two stages, exhausting that shared
shader table before the pipeline limit. SGFX `07263cb` separates shader capacity
(1,024 modules) from the unchanged 256 compute/256 graphics pipeline limits.
ABI mirrors and generation retirement use the same category-specific limits.
The 512 MiB resource budget and Skia's 64-program cache remain unchanged.

Core/ABI tests passed; Vulkan unit tests passed 111 with four ignored. A real
Metal-device regression retained 160 pipelines/320 live shader modules across
350 iterations, verified draw/readback pixels and failed-create rollback.
This host test does not establish native VirGL behavior.

The first normal-bundle native YouTube run subsequently hit a distinct
`SubmissionTooLarge` rejection followed by Vulkan `ERROR_DEVICE_LOST`.
Another run also failed at a video transition. The exact limit responsible
(packet, upload-arena, logical-stream or shader-source) remains unresolved.
A subsequent retry using the normal bundled driver reached native 720p media time 208.53 s, with 6,253 decoded, 4,672
presented and 260 decoder-output drops, without shader-slot OOM or another
oversize rejection. Those counters cover different pipeline stages and do not
sum. This retry is counter-based evidence without visual verification. The
failures in the other runs remain evidence against playback stability; the
precise rejection limit is still unresolved.

The normal image includes the capacity and transport changes. The oversize
submission failure and visible-frame performance remain follow-up work;
transport success must not be interpreted as a playback stability result.

## Native fullscreen and GPU transfer follow-up (2026-10-17)

Myrica `345ee0641a5d341fd7482c8f654846ae5151dde6` uses WebKit
`1f3a2f646fe4a0d80477acf100e964672ad4ea1d`. The ordinary Release profile enables
the document Fullscreen API explicitly, including when reusing an older CMake
cache. WebCore's element entry/exit callbacks now wait for the native window
acknowledgement. Myrica hides its browser chrome and decoration in fullscreen;
Escape uses WebCore's normal exit path. Stale native acknowledgements cannot
complete a newer request.

On the normal HVF/4-vCPU/8-GiB image with Cocoa GL/Retina, audio and `/dev/video0`,
the actual YouTube watch page `cxvXOVAVwjE` entered fullscreen, returned with
Escape and entered again. Native state acknowledgements and RFB screenshots
confirmed the video occupying the complete 1280x800 output, hidden address bar
and window decoration, and restored browser chrome on exit. The WebCore
viewport changed from 2196x1282 to 1280x800 and back. Playback continued through
these transitions. Myrica host tests passed **67**, including browser-chrome
restoration. The C++ embedding and full normal Release image compiled.

The cache policy previously gave **each** Ganesh context 256 MiB despite sharing
one Vulkan device with a **512 MiB** allocation ceiling. WebKit now divides one
device allowance (half the device-local heap, capped at 256 MiB) among live
contexts, refreshing each limit on its owning thread. Idle contexts adopt a
changed allowance on their next use; this is a cache policy, not a global hard
allocator. The native run with two live contexts reported 128 MiB per context,
instead of 256 MiB. The ICD's advertised heap now matches its existing 512 MiB
allocation ceiling; the ceiling itself was not increased.

The Scarlet layer host also now follows WebKit's `LayerTreeHost` native-image
sharing: one `CoordinatedImageBackingStore` per native image identity, reused
across layers. Owner-side updates eject entries whose only owner is this map,
and teardown clears it before destroying the painting engine. The previous port
created another store on each request instead of sharing an existing upload.

Failure-only diagnostics identified the earlier `SubmissionTooLarge` precisely:
the native logical stream exceeded **64 MiB**, after 32 approximately 2-MiB
chunks. A subsequent comment-scroll reproduction contained **314 operations /
176,979,520 texture source bytes**, no buffer uploads and no render passes
or draws. This reproduced the rejection even after the shared cache correction,
establishing a separate transfer-batching problem.

SGFX `b6bb291cf9d0bf2fca2010e3de457a961d7b4749` validates texture-transfer-only
recordings in full, then submits ordered batches of at most 8 MiB of source
pixels. Large uploads split at row boundaries and preserve rectangle positions,
mip levels, array layers and overwrite order. Texture barriers, image copies
and mip blits remain in order between uploads; the mip-generation regression
covers this actual recording pattern. Every receipt participates in
fence completion/readback waits. Mixed draw/transfer recordings keep their
existing path. Vulkan tests passed **115** with **4 ignored**; SGFX core tests
passed **22**. Native validation of this batching change follows below.

The shared cache change alone did not eliminate allocation pressure. A native
fullscreen/comment-scroll run still reached 536,689,168 allocated bytes against
the 536,870,912-byte ceiling, with 333 buffers and 401 images alive. The video
session then failed around media time 46.4 s, followed by a queue OOM/device
loss. The owner-context cache reported about 89 MiB, so its cache counter alone
cannot account for the complete device. Live images, other contexts and
unbudgeted/shared output allocations need further lifetime/accounting work.
During comment loading, one owner tick reached 621 ms. Fullscreen API success
does not resolve these remaining rendering-latency and playback-stability issues.

The final normal image (Myrica `345ee06`, WebKit `1f3a2f646fe`, SGFX `b6bb291`)
was built with `cargo scarlet image --project projects/aarch64-limine-full --release`.
The populated WebKit source HEAD and CMake cache confirmed the pinned revision,
`Release`, Fullscreen API, JIT, DFG and FTL enabled. Real YouTube playback entered
native fullscreen and returned with Escape; screenshots confirmed the video,
hidden chrome and restoration. A 155-wheel-step scroll into actual comments and
155 steps back produced **zero** logical-stream rejection, device-loss or WebCore
resize errors in this run. It still logged **21 allocation-budget failures**
near 512 MiB, and the media session failed. Thus native-image sharing and transfer
batching are included, but allocation-pressure recovery and comment-loading
latency remain open. An earlier fullscreen-only retry reached media time
176.64 s with 5,298 decoded / 4,924 presented / 166 decoder-output drops; that
separate observation does not establish comment-scroll playback stability.

## Offscreen video upload lifetime correction (2026-10-17)

Allocation-owner diagnostics in SGFX `22c0ce2` identified about **100 pending
720p NV12 frames**: 99 R8 1280x720 images (91,238,400 bytes) and 100 RG8 640x360
images (46,080,000 bytes), alongside 195,690,496 bytes in 257 transfer-source
buffers. CPU mirrors alone occupied 195,972,864 bytes at the first reported
failure. This was not just ordinary thumbnail storage or the budgeted Skia cache.

The compositor video pump created textures for each decoded frame, but relied
on a later page composition to flush/submit them. Scrolling the player offscreen
made scene damage empty, so the compositor skipped that submit. Texture writes
and their staging buffers accumulated while playback continued. A subsequent
page paint could then send the large pending transfer burst. The pump also did
not independently poll GPU completions.

WebKit `4f02391e559fe75a5494a19a1ddd56c2fb430f7b`, consumed by Myrica
`2ecd7f70f0263725a5bcf54155bf8e6641c0143a`, makes the pump flush/submit texture
uploads asynchronously, including images that have never been drawn. Skia's
finished callback supplies an owned completion flag; at most **three** uploads
can be outstanding per pump. The compositor polls completions before acquiring
another decoded frame, even when the page is unchanged. The decoder's bounded
queue and audio clock continue independently when upload slots are occupied.
Completion callbacks retain only their flag, not the media player/pump.

The owner preview path also submits transfers without waiting for a later page
paint and skips new GPU uploads for hidden pages. A genuine Skia-reported OOM
on a valid decoded frame retires outstanding work, frees unused GPU resources
and retries once. Continued temporary OOM preserves the last image and releases
the frame lease for a later retry, instead of declaring decoder failure.
Invalid frames and device loss remain errors. The **512 MiB limit is unchanged**.

Validation used the ordinary rebuilt AArch64 Release image, HVF/4 vCPUs/8 GiB,
Cocoa GL/Retina, audio and `/dev/video0`, with the same YouTube watch URL. After
fullscreen entry/exit, 155 wheel steps moved into actual comments. The video
remained offscreen for **141.5 seconds**: media time reached 191.51 s and 5,743
frames were decoded, with **zero allocation-budget failures, stream rejections
or device-loss errors**. After 155 steps back, visible playback resumed and
fullscreen worked again. At media time 224.47 s, counters reached 6,732 decoded,
1,588 presented and 171 decoder-output drops. Presentation stayed at 985 while
the player was offscreen, then advanced on return; these are not full-run FPS
measurements. Native screenshots confirmed the returned video image.

C++ syntax validation and the full native Release build passed; Vulkan unit
tests passed **115**, with **4 ignored**. No forced-OOM fault injection was used
to exercise the new retry branch; the actual formerly failing scroll case
completed without OOM. These checks resolve that reproduction, not every source
of comment-loading latency or all possible GPU memory pressure.

## Threaded asynchronous scrolling (2026-10-17)

The previous Scarlet port disabled `ENABLE_ASYNC_SCROLLING` and
`ENABLE_SCROLLING_THREAD`; wheel input always requested synchronous scrolling
and blocking DOM dispatch on the Page owner. Independent composition therefore
left wheel input waiting behind script and layout.

WebKit `dbe7f6de1709d4461662d3725025afd28136fadc` connects
`ThreadedScrollingCoordinator` and `ScrollingTreeCoordinated` through Scarlet's
ChromeClient factory. Frame/overflow scrolling, hit testing, fixed/sticky nodes,
event regions and scroll-position reconciliation use WebCore's existing classes.
The native build enables async scrolling and the coordinated port's scrolling
thread, kinetic-wheel and wheel-event-region capabilities.

Myrica routes wheel events from the UI through an independently owned endpoint.
Acceptance checks navigation generation and viewport dimensions/scale, with a
256-event pending limit; rejected events retain the owner queue. WebKit chooses
blocking or passive DOM dispatch. Endpoint invalidation clears owner callbacks
before Page destruction. DOM routing temporarily retains the owner run loop
under the live-state lock, then posts after releasing that lock to avoid a
lock inversion with the UI mailbox's wake callback.

The compositor supplies display ticks while scrolling is active. It consumes
scroll-thread position writes under the scrolling-tree lock, keeping fixed and
sticky positions together and leaving ordinary layout-position commits with
the owner. A queued owner update is signalled to WebKit's standard rendering
synchronization timeout, allowing composition while script is busy. Phase-less
pixel wheels retain immediate pixel deltas and are excluded from kinetic
history, since they have no gesture-end event to drain that history.

Host verification: **77 Myrica tests passed**, including blocked-owner input,
ordinary-input fallback, pending navigation and resize rejection. Syntax checks
also passed for the embedding and standard scrolling implementation files.

Native validation used the ordinary Release image with Myrica
`3be1ca8` / WebKit `dbe7f6de1709d4461662d3725025afd28136fadc`, HVF,
4 vCPUs, 8 GiB, Cocoa GL/Retina, audio and `/dev/video0`. The guest output was
2560×1600. The startup log reports `compositor=threaded scrolling=async`.

A real HTML timer blocked JavaScript for eight seconds; the recorded owner tick
was **8,001,014 microseconds**. During that block, wheel input moved the retained
page rows while the fixed header remained in place. The nested overflow region
scrolled independently. Four non-passive wheel events updated their DOM counter
to four and left the page stationary through `preventDefault()`. Maximize/restore,
24 further wheel steps and ordinary window closure passed.

The same image then opened the real YouTube watch page
`https://www.youtube.com/watch?v=cxvXOVAVwjE`, decoding 1280×720 H.264 through
Scarlet's NV12 device path. Six wheel steps partially exposed the comments while
video presentation continued. Another 40 steps reached actual loaded comments;
while offscreen, media time advanced from 56.88 to 57.90 seconds and decoded
frames increased from 1,708 to 1,739 while presentation intentionally held at
1,319. After 46 steps back, presentation advanced from 1,583 to 1,612 across
media time 70.08 to 71.08 seconds.

Fullscreen entry/Escape exit, theater mode and a twelve-event corner-drag resize
passed. The backing extent changed from 2196×1282 to 2008×1094, and the browser
remained usable. The 4:05 video reached its natural end, then autoplay started
the next video. Both browser processes closed normally. Across this run there
were **zero device-loss, GPU allocation-budget, queue-execution or WebCore resize
failures**. The full native build passed.

YouTube initial loading still included an owner tick of about **1.61 seconds**.
The native tests establish independent scroll composition and the listed
regressions; they do not measure all sources of loading latency or establish
a general frame-rate benchmark. Remote-desktop capture adds readback work.
Validation logs and captures use `/tmp/scarlet-async-scroll-*`; the implementation
and bundle references are preserved in their repositories.
