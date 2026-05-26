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

The MVP supports `S16LE`, 48 kHz, stereo by default. The ABI keeps format/rate
fields explicit so other formats can be added without changing the transport.

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

## User Space

The first user program is `playwav`:

- parses RIFF/WAVE PCM
- configures `/dev/audio0`
- mmap-writes into the shared ring
- commits whole periods, padding the final partial period with silence
- starts playback after initial buffering
- polls/status-loops until the stream drains

Future user-space work should add `audiod`, mixing, format conversion, device
routing, and per-client policy without expanding the kernel beyond the PCM ring
transport.

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
playwav /root/sweetmemory.wav
```

The MVP supports PCM S16LE WAV input. The current virtio-snd backend supports
48 kHz stereo output, so test WAV files should match that format unless user
space adds resampling or format conversion before writing to `/dev/audio0`.

## Compatibility

Linux compatibility should be a shim, not the native design. OSS-style
`/dev/dsp` can map cleanly to `write()` and a small ioctl subset. ALSA PCM
compatibility can translate selected `/dev/snd/pcm*` ioctls into the native ring
API later.
