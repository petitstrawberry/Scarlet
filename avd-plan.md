# Apple AVD Hardware Decode Bring-up Plan for Scarlet

## Goal

Apple Silicon 上の Scarlet で Apple Video Decoder、以下 AVD、を利用し、H.264 のハードウェアデコードを実現する。

初期ゴールは **H.264 Annex B access unit を AVD で decode し、NV12 frame として既存の Scarlet video decode device API に返すこと**。SWS は現時点では BGRA 前提なので、初期段階では SWS を変更せず、既存 `video_player` 側の NV12→BGRA 変換経路を使う。

Scarlet にはすでに `/dev/vvideo0` という video decode device contract があり、このAPIは現在 VirtIO video backend で使われているが、設計上 VirtIO 専用ではなく、将来の非 VirtIO backend も同じ contract を露出できるとされている。

## Non-goals

初期実装では以下をやらない。

* HEVC / VP9 / AV1 対応
* Linux ABI の V4L2 Request API 実装
* SWS の NV12 native surface 対応
* GPU shader による NV12→RGB 変換
* 4K / HDR / 10-bit / interlaced / exotic H.264 stream の完全対応
* production quality の error recovery
* Apple 純正 AVD firmware protocol の再実装

## Target architecture

```text
user/bin/video_player
  ↓ existing Scarlet video decode API: /dev/vvideo0
kernel Apple AVD video backend
  ↓
Apple AVD low-level driver
  ↓ MMIO + DART + mailbox IRQ
AVD Cortex-M3 firmware
  ↓
AVD fixed-function H.264 decoder
  ↓
NV12 output frame
```

初期段階では `/dev/vvideo0` の既存 ABI に寄せる。必要なら debug 用に `/dev/avd0` を追加してもよいが、userland の正式入口は既存 video decode device contract に合わせる。

## Existing Scarlet integration points

Scarlet 側にはすでに video decode device API がある。`write/read` の simple path と、`mmap + control` の preferred path があり、`video_player` は mapped control path を使う想定になっている。

現在の decoded frame format は `SVF1` header + payload であり、`video_player` は NV12 video-range payload を期待している。pixel format は `0x3432_3076`、つまり `v024` と定義されている。

`video_player` 側も `/dev/vvideo0`、`VVIDEO_*` command、H.264/AV1 format value、NV12 pixel format をすでに持っている。 また、hardware decoder が返した NV12 payload を BGRA buffer に変換する `update_from_nv12()` 経路も存在する。

そのため、初期実装では SWS は触らず、AVD backend が既存 ABI と同じ NV12 frame を返すことを目標にする。

## External references to use

Asahi 側には AVD 内蔵 Cortex-M3 用の最小 firmware があり、`-nostdlib -mcpu=cortex-m3 -mthumb` で build される。 firmware source は `src/avd.c`, `src/irq.c`, `src/util.c` を中心に構成されている。

Asahi firmware は AVD version/tier ごとに binary を分けている。v2/v3/v4/v5 と tier ごとの出力が Meson に定義されている。

AVD bring-up の実験コードは m1n1 側にもあり、`/arm-io/avd` と `/arm-io/dart-avd` から base address と DART を取得し、power enable と DART init を行っている。 DART mapping は stream と IOVA を指定して物理メモリを貼る形になっている。

## Work packages

## WP1: Rust CM3 firmware

### Objective

Asahi `avd-fw` 相当の薄い CM3 firmware を Rust `no_std` で実装する。

この firmware は codec parser でも decoder driver でもない。役割は以下に限定する。

```text
- vector table / reset handler
- variantごとのtunables適用
- NVIC enable
- IRQ handler
- decode DONE / ERROR status clear
- mailbox notification to AP / Scarlet kernel
- panic / hardfault / unknown IRQ reporting
```

### Recommended location

```text
firmware/apple-avd-fw-rs/
  Cargo.toml
  memory.x or avd-cm3.ld
  build.rs
  src/
    main.rs
    vector.rs
    mmio.rs
    reg.rs
    irq.rs
    tunables.rs
    tunables_v2.rs
    tunables_v3.rs
    tunables_v4.rs
    tunables_v5.rs
```

Target:

```text
thumbv7m-none-eabi
```

Build output:

```text
avd-fw-v2-t0.bin
avd-fw-v3-t0.bin
avd-fw-v3-t1.bin
avd-fw-v4-t0.bin
avd-fw-v5-t0.bin
avd-fw-v5-t1.bin
```

### Implementation details

The reset path should mirror the Asahi firmware behavior:

```text
reset:
  apply_tunables()
  enable_all_nvic_irqs()
  enable_interrupts()
  send FW_READY
  loop wfi
```

Asahi’s `_start()` applies tunables, enables NVIC, enables interrupts, writes `CM3_BOOT`, then waits in `wfi`. Scarlet’s Rust firmware should keep this behavior but add an explicit `FW_READY` message for easier kernel bring-up.

Define a Scarlet AVD firmware ABI:

```rust
const MSG_READY:       u32 = 0x0000_0001;
const MSG_PANIC:       u32 = 0x0000_0002;
const MSG_VP_DONE:     u32 = 0x0000_0100;
const MSG_VP_ERROR:    u32 = 0x0000_0200;
const MSG_PP_DONE:     u32 = 0x0000_1000;
const MSG_UNKNOWN_IRQ: u32 = 0x0001_0000;
```

For IRQ handling, port the Asahi model: clear decode status and send a compact mailbox message. In Asahi firmware, `vpdone()` clears `DECODE_STATUS_DONE` and sends `0x100 | pipe`, `err()` clears `DECODE_STATUS_ERR`, and `ppdone()` sends `0x1000`.

### Acceptance criteria

* Firmware builds to raw `.bin` for each supported AVD version/tier.
* Binary starts with a valid CM3 vector table.
* Kernel can load firmware into AVD SRAM and receive `MSG_READY`.
* Unknown IRQ and hardfault paths send debug/panic messages instead of silently hanging.
* DONE/ERROR IRQ paths clear status and notify kernel.

## WP2: Apple AVD low-level kernel driver

### Objective

Implement the low-level AVD device bring-up in Scarlet kernel.

This layer is responsible for:

```text
- ADT discovery
- PMGR power enable
- AVD MMIO mapping
- DART discovery and initialization
- firmware SRAM load
- CM3 boot
- mailbox receive path
- DART-backed input/output buffer allocation
- cache maintenance around DMA
```

### Recommended location

```text
kernel/src/drivers/apple/avd.rs
kernel/src/drivers/apple/avd/
  regs.rs
  firmware.rs
  dart.rs or use existing DART abstraction
  session.rs
  h264.rs
```

Or, if Apple Silicon drivers are loadable/in-tree driver crates in the current branch, place it under the existing Apple driver module structure.

### Bring-up sequence

Implement a kernel sequence corresponding to the m1n1 AVD experiment:

```text
1. Locate ADT node /arm-io/avd
2. Locate ADT node /arm-io/dart-avd
3. Enable PMGR power for both nodes
4. Initialize DART
5. Map AVD MMIO
6. Clear AVD code/SRAM areas
7. Load CM3 firmware blob
8. Start CM3
9. Wait for MSG_READY
```

m1n1’s experiment powers `/arm-io/avd` and `/arm-io/dart-avd`, initializes DART, and stores the AVD MMIO base from ADT. Its boot code clears AVD code/SRAM-like regions and contains explicit CM3 start logic.

### DART buffer management

Implement an AVD-local DMA allocator:

```rust
struct AvdDmaBuffer {
    phys: PhysAddr,
    iova: u64,
    len: usize,
    cpu_ptr: NonNull<u8>,
    stream: u32,
}
```

Requirements:

* Use AVD DART stream 0 initially.
* Use 16 KiB alignment/page size unless current DART abstraction proves otherwise.
* Support fixed IOVA placement for early bring-up if necessary.
* Add explicit cache clean before AVD reads input buffers.
* Add explicit cache invalidate before CPU reads AVD output buffers.
* Track ownership and unmap on session teardown.

m1n1 uses `PAGE_SIZE = 0x4000` for AVD allocations and maps physical memory into DART by stream and IOVA.

### Acceptance criteria

* Driver can print AVD base, DART base, firmware variant, and selected stream.
* Driver can load Rust CM3 firmware and receive `MSG_READY`.
* Driver can allocate, map, write, read, and unmap an AVD DART buffer.
* Driver has timeout/error paths for firmware boot and mailbox wait.

## WP3: AVD hardware initialization and MMIO command path

### Objective

Port the known AVD MMIO init and H.264 submit path into Scarlet.

This is separate from the firmware. The CM3 firmware handles tunables and IRQ forwarding, but the AP/kernel side still needs to configure top-level AVD registers, DMA paths, instruction FIFO, codec enable bits, and submit commands.

### Tasks

1. Port top-level AVD wrapper/DMA initialization from m1n1 experiment.
2. Define register constants for:

   * code/SRAM regions
   * mailbox registers
   * decode control registers
   * DMA registers
   * instruction FIFO registers
   * status register
   * H.264 instruction register
   * submit register
3. Implement `avd_init_engine()`.
4. Implement `avd_setup_instruction_fifo(session)`.
5. Implement `avd_submit_h264(session, request)`.
6. Initially support only one session and one pending decode.

m1n1’s `setup_dma()` writes instruction FIFO address and size registers, then enables codec-specific bits. For H.264 it uses the H.264-specific path and modifies decode control bits.

For H.264, m1n1 writes instruction words to the H.264 register, submits through `0x1104014`, and watches status transitions through `0x1104060`.

### Acceptance criteria

* Kernel can run engine init without faulting.
* Kernel can submit a synthetic or known-good H.264 instruction stream.
* Completion can be detected via mailbox DONE or, temporarily, polling.
* Error IRQs are surfaced as structured kernel errors.

## WP4: H.264 AVD frontend

### Objective

Generate AVD-compatible H.264 decode requests from Scarlet input.

This is the hardest part. AVD does not consume MP4 files directly, and it should not be treated as “write compressed bytes, get frame” hardware. The driver/frontend must parse H.264 access units, manage reference frames, and generate the AVD instruction stream.

### Initial scope

Support only:

```text
- H.264 Annex B input
- known-good baseline/main/high streams
- progressive frames
- 8-bit NV12 output
- one access unit per submit
- simple DPB sufficient for the test stream
```

### Recommended approach

Use existing code only as a guide for the first pass.

`video_player` currently imports `rust_h264` and `parse_annex_b`, so Scarlet already has H.264 parsing pieces in the tree/userland path. However, AVD instruction stream generation is separate from normal software decode. The implementation should introduce a dedicated AVD H.264 frontend rather than trying to reuse the full CPU decoder as-is.

Suggested split:

```text
lib/apple-avd-h264 or kernel/src/drivers/apple/avd/h264.rs:
  - parse access unit
  - collect SPS/PPS/slice metadata
  - allocate input payload buffer
  - allocate output Y/UV buffers
  - track reference frame slots
  - build AVD instruction stream
```

Data model:

```rust
struct AvdH264Request {
    input_iova: u64,
    input_len: usize,
    output_y_iova: u64,
    output_uv_iova: u64,
    width: u32,
    height: u32,
    stride: u32,
    sps: H264Sps,
    pps: H264Pps,
    slice: H264SliceParams,
    refs: AvdReferenceList,
    timestamp: u64,
}
```

### Initial simplification

For the first milestone, allow a hardcoded known-good stream path:

```text
- fixed test H.264 file
- fixed SPS/PPS
- fixed resolution
- minimal reference handling
- compare output checksum
```

After that works, generalize.

### Acceptance criteria

* Decode one known H.264 frame into NV12.
* Decode a short known H.264 Annex B stream.
* Output frame dimensions and payload length match expected NV12 layout.
* No per-frame heap allocation in the hot path except where explicitly marked temporary.

## WP5: Expose AVD through existing `/dev/vvideo0` contract

### Objective

Make the AVD backend usable by existing Scarlet userspace.

The existing video decode API supports:

```text
1. open /dev/vvideo0
2. create session
3. mmap session buffer
4. copy coded access unit into input region
5. submit session
6. dequeue session
7. read decoded NV12 payload from output region
```

This sequence is documented in the current video decode API.

### Design

Introduce a backend abstraction behind the existing character device API.

```rust
trait VideoDecodeBackend {
    fn create_session(&self, owner: TaskId) -> Result<SessionId>;
    fn destroy_session(&self, id: SessionId) -> Result<()>;
    fn mmap_info(&self, id: SessionId) -> Result<ScarletVideoBufferInfo>;
    fn submit(&self, id: SessionId, input_len: usize, coded_format: u32, timestamp: u64) -> Result<()>;
    fn dequeue(&self, id: SessionId) -> Result<Option<ScarletVideoDequeuedFrame>>;
}
```

Then implement:

```text
VirtioVideoBackend
AppleAvdBackend
```

The current API structures are duplicated in kernel and `video_player`; the documentation already notes this should move to a shared header/crate before being stable. Do that refactor only if it is small. Otherwise keep the ABI duplicated for the initial AVD bring-up and avoid expanding scope.

### Output layout

Return the same logical frame format that `video_player` already expects:

```text
SVF1 header
width
height
pixel_format = 0x3432_3076
payload_len = width * height * 3 / 2
payload = NV12 single-buffer layout: Y plane followed by interleaved UV
```

The existing API currently assumes a single-buffer NV12 frame. If AVD internally produces separate Y/UV planes, the backend may initially copy them into the existing single-buffer output layout.

### Acceptance criteria

* Existing `video_player` can open `/dev/vvideo0` on Apple Silicon.
* `VVIDEO_CREATE_SESSION`, `mmap`, `VVIDEO_SUBMIT_SESSION`, and `VVIDEO_DEQUEUE_SESSION` work.
* `video_player` receives NV12 and converts it to BGRA using the existing path.
* No SWS changes are required for the first working demo.

## WP6: Test and debug tooling

### Required debug commands or test hooks

Add small tools or kernel debug commands:

```text
avd-info
  Print ADT info, MMIO base, DART stream, AVD version/tier, firmware name.

avd-fw-ping
  Load firmware, start CM3, wait for MSG_READY.

avd-dart-test
  Allocate DART buffer, write pattern, optionally let AVD/DART path read/write if safe.

avd-decode-one
  Decode one built-in H.264 access unit and print output checksum.

avd-trace
  Enable verbose MMIO/mailbox/status logs for one decode.
```

### Test assets

Add a small H.264 Annex B test file to an appropriate media/test bundle, or document how to provide it externally.

The existing `video_player` default path is `/root/media/bad_apple.h264`. That can be used as a smoke target once the simple one-frame test passes.

### Acceptance criteria

* `avd-fw-ping` succeeds repeatedly after cold boot.
* `avd-decode-one` produces stable output checksum.
* `video_player` can play a short H.264 stream via AVD backend.
* Timeout paths do not wedge the kernel permanently.
* Errors include enough state: last mailbox message, status register, stream id, pending request id.

## Milestones

### M0: Spec and skeleton

Deliverables:

* Add `docs/device/apple-avd.md`.
* Add register map notes and links to source references.
* Define Scarlet AVD firmware ABI messages.
* Add empty Rust firmware crate.
* Add empty kernel driver module gated behind Apple Silicon config.

Done when:

* Project builds with AVD disabled by default.
* Documentation states exact non-goals and first target stream.

### M1: Rust CM3 firmware READY

Deliverables:

* Rust `thumbv7m-none-eabi` firmware builds to `.bin`.
* Vector table and reset handler implemented.
* `MSG_READY`, `MSG_PANIC`, `MSG_UNKNOWN_IRQ` implemented.
* Kernel can load firmware and observe `MSG_READY`.

Done when:

* `avd-fw-ping` works on M1 hardware.

### M2: AVD + DART bring-up

Deliverables:

* ADT discovery for AVD and DART.
* PMGR enable.
* DART init and stream 0 mapping.
* AVD SRAM load and CM3 boot integrated.
* Basic mailbox receive path.

Done when:

* Kernel can map AVD buffers and pass simple DART mapping tests.
* Repeated firmware boot does not crash or fault.

### M3: AVD engine init and command path

Deliverables:

* Port m1n1-derived top-level AVD init sequence.
* Implement instruction FIFO setup.
* Implement H.264 register write path.
* Implement submit and status wait path.
* Use polling first if mailbox IRQ is not stable; switch to IRQ once stable.

Done when:

* Driver can submit a known-good synthetic/fixed instruction stream and observe expected status or controlled error.

### M4: One-frame H.264 decode

Deliverables:

* Minimal H.264 AVD frontend for one known test frame.
* Input payload DART buffer.
* Output Y/UV DART buffers.
* NV12 output extraction.
* Output checksum test.

Done when:

* `avd-decode-one` returns a valid NV12 frame with expected width/height/checksum.

### M5: Existing `/dev/vvideo0` integration

Deliverables:

* Implement AVD backend for current Scarlet video decode contract.
* Support one session and one pending decode initially.
* Return `SVF1` + single-buffer NV12.
* Wire `video_player` to use the same API without SWS changes.

Done when:

* `video_player` can display a short H.264 file using AVD backend.

### M6: Continuous playback

Deliverables:

* Basic DPB/reference management.
* Frame reorder handling sufficient for common H.264 streams.
* Timestamp propagation through `VVIDEO_SUBMIT_SESSION` and dequeue.
* Frame drop/recovery behavior for late frames.
* Remove obvious per-frame allocations in the hot path.

Done when:

* A short H.264 video plays continuously with stable frame order.
* CPU software decode path is not used for video decode.
* NV12→BGRA conversion remains the main user-visible CPU-side cost.

### M7: Hardening

Deliverables:

* Interrupt-driven completion path.
* Better error codes.
* Session teardown cleanup.
* DART unmap correctness.
* Cache maintenance audit.
* Power/reset recovery path.
* Basic performance counters.

Done when:

* Failed decode requests do not wedge subsequent sessions.
* Repeated open/play/close cycles work.
* Kernel logs are useful but not spammy in normal playback.

## Risks and unknowns

### H.264 instruction stream generation

This is the largest risk. The AVD MMIO path is visible enough to start, but generating correct AVD instruction streams from arbitrary H.264 access units is the hard part. Keep the first target to a known-good stream and expand only after one-frame decode works.

### Tunables and SoC variants

Asahi firmware carries AVD version/tier-specific tunables. Scarlet should initially mirror that variant split rather than trying to infer the meaning of every bit. The exact AVD version/tier detection must be documented and logged.

### Cache maintenance

DMA coherency bugs will look like random decode failures. Every input buffer write and output buffer read must have explicit cache maintenance unless the memory type/coherency model proves it unnecessary.

### Output layout mismatch

AVD may naturally expose separate Y and UV planes. The current Scarlet API assumes a single-buffer NV12 payload. Initially copying Y+UV into the existing output mapping is acceptable.

### Interrupts

Polling can be used during bring-up, but final playback should use mailbox IRQ completion. If AIC routing is unstable, keep a debug polling fallback.

### Scope creep

Do not add SWS NV12 surface support, V4L2 Request API, HEVC, VP9, AV1, or GPU color conversion until H.264 via `/dev/vvideo0` works.

## Recommended first PR stack

1. Documentation and firmware ABI constants.
2. Rust CM3 firmware crate that builds binaries but is not loaded yet.
3. Kernel AVD skeleton: ADT/MMIO/DART discovery and logs.
4. Firmware loader and `MSG_READY` test.
5. DART buffer allocator for AVD.
6. AVD engine init/register definitions.
7. One-frame H.264 decode debug command.
8. `/dev/vvideo0` AVD backend integration.
9. `video_player` smoke path and documentation update.

## Final success definition

The project is considered successful when:

```text
On aarch64-apple-limine-full running on M1/M2 hardware:
  - Scarlet boots normally.
  - AVD firmware is loaded by Scarlet, not Apple firmware.
  - /dev/vvideo0 is backed by Apple AVD.
  - video_player submits H.264 access units.
  - AVD returns NV12 frames.
  - video_player converts NV12 to BGRA and displays through existing SWS.
  - CPU-side rust_h264 full software decode is not used for the video frames.
```
