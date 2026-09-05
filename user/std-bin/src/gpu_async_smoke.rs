//! Opt-in VirtIO/VirGL async transport smoke test, run inside Scarlet.
//!
//! This intentionally uses driver-private VirGL packets to exercise GPU clear,
//! completion, readback, detach, dropped observers and closed owner handles.
//! It is a diagnostic, not an application-facing SGFX API or a benchmark.

#[cfg(target_os = "scarlet")]
mod native {
    use std::time::{Duration, Instant};

    use gpu_raw::{
        GPU_COMPLETION_COMPLETE, GPU_COMPLETION_FAILED, GPU_IMAGE_USAGE_RENDER_TARGET,
        GPU_IMAGE_USAGE_TRANSFER_SRC, Gpu, GpuCompletion, GpuImageBgraRect, GpuQueue,
        GpuSubmitError,
    };
    use scarlet_os::poll::{POLLIN, PollHandle, poll};

    fn checked<T, E: core::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("{error:?}"))
    }

    fn submit(queue: &GpuQueue, commands: &[u8]) -> Result<GpuCompletion, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match queue.submit_async(commands) {
                Ok(completion) => return Ok(completion),
                Err(GpuSubmitError::Busy) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(format!("submit: {error:?}")),
            }
        }
    }

    fn wait(completion: &GpuCompletion) -> Result<(), String> {
        let handle = checked(u32::try_from(completion.as_handle().as_raw()))?;
        let mut handles = [PollHandle::new(handle, POLLIN)];
        if checked(poll(&mut handles, 5_000_000_000))? == 0 {
            return Err("completion wait timed out".into());
        }
        let info = checked(completion.query())?;
        if info.state == GPU_COMPLETION_FAILED {
            return Err(format!("completion failed: {}", info.failure));
        }
        if info.state != GPU_COMPLETION_COMPLETE {
            return Err(format!(
                "readiness did not report completion: {}",
                info.state
            ));
        }
        Ok(())
    }

    fn packet(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    fn clear(red: f32, green: f32) -> Vec<u8> {
        // VirGL CLEAR, eight payload dwords: color0 mask, RGBA, depth, stencil.
        packet(&[
            7 | (8 << 16),
            1 << 2,
            red.to_bits(),
            green.to_bits(),
            0,
            1.0f32.to_bits(),
            0,
            0,
            0,
        ])
    }

    pub(super) fn run() -> Result<(), String> {
        let gpu = checked(Gpu::open("/dev/gpu0"))?;
        if checked(gpu.query_info())?.backend_id_bytes() != b"virtio-gpu" {
            return Err("this packet-level smoke test requires VirtIO/VirGL".into());
        }
        let context = checked(gpu.create_context(&checked(gpu.query_dialect(0))?))?;
        let queue = checked(context.create_queue())?;
        let limits = checked(queue.query_async())?;
        if limits.max_pending_submissions < 2 {
            return Err("multiple in-flight async submissions are not supported".into());
        }
        println!(
            "[gpu-async-smoke] capacity={} command_bytes={}",
            limits.max_pending_submissions, limits.max_opaque_command_size
        );
        let image = checked(gpu.create_image_with_usage(
            16,
            16,
            GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_TRANSFER_SRC,
        ))?;
        let resource = checked(u32::try_from(checked(context.attach_image(&image))?))?;
        // Create surface 1 (BGRA8 level/layer zero), then bind it as color0.
        let mut commands = packet(&[
            1 | (8 << 8) | (5 << 16),
            1,
            resource,
            1,
            0,
            0,
            5 | (3 << 16),
            1,
            0,
            1,
        ]);
        commands.extend(clear(1.0, 0.0));
        let drawn = submit(&queue, &commands)?;
        commands.fill(0); // The driver must already own an independent copy.
        let checkpoint = submit(&queue, &[])?;
        // No completion queries or polls during this interval: progress belongs
        // to the kernel worker, not to the observing userspace process.
        std::thread::sleep(Duration::from_millis(200));
        wait(&checkpoint)?;
        wait(&drawn)?;
        let mut pixels = [0; 16 * 16 * 4];
        checked(context.readback_image_bgra(
            &image,
            &mut pixels,
            16 * 4,
            GpuImageBgraRect::new(0, 0, 16, 16),
        ))?;
        if !pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 255, 255])
        {
            return Err(format!(
                "async red clear readback mismatch: {:?}",
                &pixels[..16]
            ));
        }
        println!("[gpu-async-smoke] async clear + checkpoint + readback PASS");

        let detached = submit(&queue, &clear(0.0, 1.0))?;
        // Legacy detach must preserve the live binding until preceding GPU
        // accesses retire, even if the worker has not published the receipt yet.
        checked(context.detach_image(&image))?;
        wait(&detached)?;
        checked(context.attach_image(&image))?;
        for _ in 0..32 {
            drop(submit(&queue, &clear(1.0, 0.0))?);
        }
        let final_receipt = submit(&queue, &[])?;
        drop((drawn, checkpoint, detached, queue, context, image, gpu));
        std::thread::sleep(Duration::from_millis(200));
        wait(&final_receipt)?;
        println!("[gpu-async-smoke] detach + dropped observers + closed owners PASS");
        println!("[gpu-async-smoke] ALL PASS");
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "scarlet")]
    if let Err(error) = native::run() {
        eprintln!("[gpu-async-smoke] FAIL: {error}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        eprintln!("gpu-async-smoke must run on Scarlet with VirtIO/VirGL");
        std::process::exit(1);
    }
}
