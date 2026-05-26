# Scarlet Audio System Design

## Goal

Scarlet audio is a PCM transport. The kernel does not decode media, mix
applications, resample, route policy, or own per-application volume. Those jobs
belong in user space, eventually in an `audiod` service.

The kernel owns only:

- audio device discovery and registration
- PCM format and buffer negotiation
- a shared playback ring buffer
- backpressure and readiness for writers
- period submission to the hardware backend

The first backend is `virtio-snd`, because it is available in QEMU and maps well
to the existing VirtIO transport code.

## Architecture

Scarlet has two intended playback paths. A single program can open the native
device directly, while multiple programs should go through SAS.

```text
Direct native playback
======================

  user process
  +-------------------------+
  | playwav                 |
  |                         |
  |  RIFF/WAVE parser       |
  |  PCM writer             |
  +------------+------------+
               |
               | open/ioctl/mmap/commit/start
               v
  kernel
  +-------------------------+       +-------------------------+
  | /dev/audio0             |       | virtio-snd backend      |
  |                         |       |                         |
  |  AudioCharDevice        | ----> |  controlq/eventq/txq   |
  |  AudioPcmRing           |       |  period submission      |
  |  single-client lock     |       |  completion reclaim     |
  +------------+------------+       +------------+------------+
               |                                 |
               | virtio PCI                      | host audio backend
               v                                 v
          QEMU virtio-snd                  CoreAudio/ALSA/etc.
```

```text
SAS playback
============

  client A                 client B
  +------------------+     +------------------+
  | playwav --sas    |     | future client    |
  |                  |     |                  |
  | mmap client ring |     | mmap client ring |
  | write PCM frames |     | write PCM frames |
  +--------+---------+     +--------+---------+
           |                        |
           | control IPC only       | control IPC only
           | /tmp/sas.sock          | /tmp/sas.sock
           v                        v
  user space
  +----------------------------------------------------------+
  | sas: Scarlet Audio Server                               |
  |                                                          |
  |  CONFIGURE -> create SharedMemory ring -> send handle    |
  |  DRAIN/CLOSE control messages                           |
  |  read per-client rings                                  |
  |  mix into one output period                             |
  +-----------------------------+----------------------------+
                                |
                                | the only /dev/audio0 client
                                v
  kernel
  +-------------------------+       +-------------------------+
  | /dev/audio0             |       | virtio-snd backend      |
  | AudioCharDevice         | ----> | period submission       |
  | AudioPcmRing            |       | completion reclaim      |
  +-------------------------+       +-------------------------+
```

The socket protocol is deliberately control-only. PCM data moves through shared
memory rings so the server can mix without copying audio payloads through IPC.

## Native Device Model

The native device is exposed as `/dev/audio0`. It is a character device with a
ring-buffer API:

1. `AUDIO_SET_PARAMS` configures format, rate, channel count, period size, and
   ring size.
2. `AUDIO_GET_BUFFER` returns the mmap-able PCM ring layout.
3. The application maps `/dev/audio0` and writes interleaved PCM frames into the
   shared ring.
4. `AUDIO_COMMIT_FRAMES` advances the application pointer after frames have been
   written.
5. `AUDIO_START` starts playback after optional pre-buffering.
6. `AUDIO_GET_STATUS` reports `hw_ptr`, `app_ptr`, queued delay, writable
   frames, and stream state.
7. `poll()` reports writable when the ring has space.

`write()` is a compatibility path. It copies bytes into the same ring and then
commits the copied frames. It is not a separate synchronous audio path.

The native device is single-client. While a stream is prepared or running,
another client must not reconfigure the device or write its own stream into the
same ring. A second open returns busy. Mixing multiple applications is
intentionally left to a future user-space audio server.

## ABI Structures

All native structs are `#[repr(C)]` and use integer fields only.

```rust
pub struct AudioPcmParams {
    pub format: u32,
    pub rate: u32,
    pub channels: u16,
    pub period_frames: u32,
    pub buffer_frames: u32,
}

pub struct AudioPcmBufferInfo {
    pub mmap_offset: u64,
    pub buffer_bytes: u64,
    pub frame_bytes: u32,
    pub period_bytes: u32,
    pub buffer_frames: u32,
    pub period_frames: u32,
}

pub struct AudioPcmStatus {
    pub state: u32,
    pub hw_ptr_frames: u64,
    pub app_ptr_frames: u64,
    pub submitted_ptr_frames: u64,
    pub writable_frames: u32,
    pub delay_frames: u32,
    pub xruns: u32,
}
```

Capabilities are also native ABI data. Formats are represented as a bitmask over
Scarlet PCM format IDs. Sample rates are returned as an explicit list so the
common ABI does not inherit a hardware-specific rate enum.

```rust
pub struct AudioPcmCapabilities {
    pub formats: u32,
    pub rate_count: u32,
    pub rates: [u32; AUDIO_PCM_MAX_RATES],
    pub min_channels: u16,
    pub max_channels: u16,
    pub min_period_frames: u32,
    pub max_period_frames: u32,
    pub min_buffer_frames: u32,
    pub max_buffer_frames: u32,
}
```

Initial PCM format IDs are:

- `AUDIO_PCM_FORMAT_S16LE`
- `AUDIO_PCM_FORMAT_S24LE3`
- `AUDIO_PCM_FORMAT_S32LE`
- `AUDIO_PCM_FORMAT_F32LE`
- `AUDIO_PCM_FORMAT_S8`

The MVP defaults to `S16LE`, 48 kHz, stereo, but drivers should expose the
actual format/rate/channel set they can configure. The kernel does not resample
or convert sample formats; user space must select a supported tuple or perform
conversion before writing to the PCM ring.

## Kernel Layers

`kernel/src/device/audio/`

- Defines native PCM structs and control numbers.
- Implements `AudioPcmRing`, backed by physically contiguous pages so it can be
  mmaped.
- Implements `AudioCharDevice`, the `/dev/audioN` wrapper.
- Defines `AudioPlaybackDevice`, the small backend trait used by hardware
  drivers.

`kernel/src/drivers/virtio_snd.rs`

- Initializes the four VirtIO sound queues: `controlq`, `eventq`, `txq`, `rxq`.
- Queries stream information and selects the first output stream that can play
  the configured PCM format.
- Sends `SET_PARAMS`, `PREPARE`, `START`, `STOP`, and period-sized tx messages.
- Reclaims completed tx messages and advances the ring hardware pointer.

## Pointer Semantics

Pointers are monotonically increasing frame counters:

- `app_ptr_frames`: frames committed by user space
- `submitted_ptr_frames`: frames submitted to the backend
- `hw_ptr_frames`: frames completed by the backend

The mmap write offset is:

```text
(app_ptr_frames % buffer_frames) * frame_bytes
```

The kernel prevents overwrite by limiting committed frames to available writable
space:

```text
buffer_frames - (app_ptr_frames - hw_ptr_frames)
```

The backend submits only whole periods except for a future drain/end-of-stream
extension.

## Stop and Release Semantics

`AUDIO_STOP`, `AUDIO_RELEASE`, and device close must leave the backend in a
clean state for the next stream. The common audio layer performs that policy:

- stop the backend and drain/cancel already-submitted periods
- submit one configured ring buffer worth of silence, chunked by backend queue
  capacity
- start the backend for the silence duration
- sleep the current task through the scheduler while silence periods complete
- stop the backend again

The sleep is deliberately handled in the common audio layer rather than inside
the hardware driver, because the driver should not busy-wait for wall-clock
audio time. The backend remains responsible for device commands, queue
submission, and reclaiming completed descriptors.

## Scarlet Audio Server

`sas` is the Scarlet Audio Server. Its role is intentionally above the kernel
PCM transport:

- own `/dev/audio0` as the single kernel audio client
- accept application control messages over `/tmp/sas.sock`
- register the service name `org.scarlet-os.sas` with sbus when sbus is running
- create one shared-memory PCM ring per client and mix those rings into one
  output stream
- keep decode, resampling, format conversion, routing, and volume policy in user
  space

The initial SAS control protocol is a small framed local-socket protocol. Each
message has an 8-byte little-endian header:

```text
u32 message_type
u32 payload_size
```

The MVP messages are:

- `CONFIGURE`: stream format, rate, channel count, preferred period, and
  preferred buffer size; SAS replies `OK` and transfers a shared-memory handle
- `DRAIN`: wait until this client's queued PCM has been mixed
- `CLOSE`: close the stream
- `OK` / `ERROR`: server responses

PCM sample data is not sent as socket payload. After `CONFIGURE`, the client
maps the returned shared-memory object and writes interleaved PCM frames into
the ring. The ring header stores monotonic `write_frames` and `read_frames`
counters; the client is the single writer, and SAS is the single reader.

```text
SAS client setup
================

  client                                sas
  ------                                ---
  connect /tmp/sas.sock  ------------> accept

  CONFIGURE(format/rate/ch/buffer) ---> validate MVP format
                                         SharedMemory::create(ring_size)
                                         mmap ring in SAS
                                         initialize RingHeader

  OK + shared-memory handle <--------- send_handle(shm)

  mmap shared-memory ring
  write PCM frames into ring --------> mix thread reads ring
  update write_frames                  update read_frames

  DRAIN ------------------------------> wait until read_frames >= write_frames
  OK <---------------------------------

  CLOSE ------------------------------> mark ring closed
```

```text
Shared-memory ring layout
=========================

  +--------------------------------------------------------------+
  | RingHeader                                                   |
  |                                                              |
  | magic/version                                                |
  | format/rate/channels/frame_bytes                             |
  | period_frames/buffer_frames                                  |
  | write_frames  <- client advances after writing PCM           |
  | read_frames   <- SAS advances after mixing PCM               |
  | flags/xrun_count                                             |
  +--------------------------------------------------------------+
  | PCM frame 0                                                  |
  | PCM frame 1                                                  |
  | ...                                                          |
  | PCM frame N-1                                                |
  +--------------------------------------------------------------+

  client write offset = (write_frames % buffer_frames) * frame_bytes
  server read offset  = (read_frames  % buffer_frames) * frame_bytes
```

The first implementation accepts only `S16LE`, 48 kHz, stereo client streams and
mixes by saturating summed samples into a fixed `S16LE`, 48 kHz, stereo output
stream. That constraint is a server policy, not a kernel ABI limit. Future SAS
work should query `/dev/audio0` capabilities, choose an output format, and add
client-side conversion or server-side conversion before mixing.

## User Space Clients

`playwav` can use the kernel device directly:

- parses RIFF/WAVE PCM
- configures `/dev/audio0`
- mmap-writes into the shared ring
- commits whole periods, padding the final partial period with silence
- starts playback after initial buffering
- polls/status-loops until the stream drains

`playwav --sas FILE.wav` sends the WAV data to SAS instead. In that mode,
`playwav` is a client and does not open `/dev/audio0`; SAS owns the device and
performs mixing.

## QEMU Usage

QEMU audio is opt-in. The run scripts add `virtio-sound-pci` only when
`SCARLET_QEMU_AUDIO=1` or `SCARLET_QEMU_AUDIO=true` is set. Selecting an audio
backend with `SCARLET_QEMU_AUDIO_DRIVER` does not enable the device by itself.

For AArch64 on macOS, a stable CoreAudio configuration is:

```sh
export SCARLET_QEMU_AUDIO=1
export SCARLET_QEMU_AUDIO_DRIVER='coreaudio,out.fixed-settings=on,out.frequency=48000,out.channels=2,out.format=s16'
export SCARLET_QEMU_ACCEL=hvf
export SCARLET_QEMU_SMP=8
export SCARLET_QEMU_GPU=virtio-gpu-pci
export SCARLET_QEMU_DISPLAY=cocoa
export SCARLET_QEMU_MEMORY=16G

cargo make run-aarch64
```

When audio is enabled successfully, PCI discovery should show a VirtIO sound
device with vendor/device `[1af4:1059]`, and `/dev/audio0` should appear in the
guest. Without `SCARLET_QEMU_AUDIO=1`, no audio PCI device is exposed to the
guest.

The CoreAudio fixed settings are host-side QEMU settings. The guest virtio-snd
driver can negotiate the PCM stream format with QEMU, but it cannot select the
macOS output device or force the host device's nominal sample rate. If playback
is stable through headphones but distorted through built-in speakers, check the
macOS output device sample rate or use the fixed CoreAudio settings above.

## Playback Test

`playwav` plays PCM RIFF/WAVE files through `/dev/audio0`:

```sh
playwav /root/foo.wav
```

The MVP supports PCM S16LE WAV input. `playwav` queries `/dev/audio0`
capabilities before configuring the stream, so test WAV files must match a
format, sample rate, and channel count reported by the driver unless user space
adds resampling or format conversion before writing to `/dev/audio0`.

To test the audio server path, start SAS first and then run:

```sh
playwav --sas /root/foo.wav
```

The current SAS MVP accepts only S16LE, 48 kHz, stereo WAV input.

## Compatibility

Linux compatibility should be a shim, not the native design. OSS-style
`/dev/dsp` can map cleanly to `write()` and a small ioctl subset. ALSA PCM
compatibility can translate selected `/dev/snd/pcm*` ioctls into the native ring
API later.
