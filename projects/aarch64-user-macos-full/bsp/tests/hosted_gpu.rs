//! Issue the guest's GPU controls against real task memory and Metal, without a disk image.
use scarlet::{
    device::{gpu::*, manager::DeviceManager},
    hosted,
    interrupt::InterruptManager,
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::capability::ControlOps,
    sched::scheduler,
    task,
    timer::get_kernel_timer,
};
use scarlet_aarch64_user_macos_full_bsp::gpu;
use scarlet_hosted_sgfx_protocol as wire;
use sgfx_core::ir::*;
use std::{
    process::{self, Command},
    thread,
    time::Duration,
};

const ARG: usize = 0x40000;
const DATA: usize = ARG + 0x1000;

#[test]
#[ignore = "requires a Metal adapter"]
fn guest_controls_render_and_read_back_from_metal() {
    const CHILD: &str = "SCARLET_METAL_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "guest_controls_render_and_read_back_from_metal",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(30));
        process::exit(2);
    });
    // Exercise routing independently of the macOS display's default IRQ 22.
    gpu::run_offscreen_configured(gpu::Config { irq: 23, trace: None }, |endpoint| {
        let mut platform = hosted::prepare(2, 128 << 20)?;
        let init = task::new_user_task("init".into(), 0);
        init.init();
        assert_eq!(scheduler::register_task(init), 1);
        let interrupts = InterruptManager::global();
        interrupts.init_controllers();
        interrupts.init_controllers_for_cpu(0);
        endpoint.ok_or("Metal adapter required")?.register()?;
        scarlet::interrupt::enable_cpu_interrupts();
        get_kernel_timer().init(0);
        let test = task::new_kernel_task("guest-metal-controls".into(), 0, exercise);
        test.set_pinned_cpu(Some(0));
        test.init();
        test.allocate_data_pages(ARG, 4).unwrap();
        let id = scheduler::register_task(test);
        scheduler::register_online_cpu(0);
        scheduler::spawn_idle_task(0);
        scheduler::enqueue_task(id, 0);
        let first = scheduler::start_scheduler().unwrap();
        platform.start_secondary_cpus()?;
        scheduler::first_switch_to_kernel_task(first);
    })
    .unwrap();
}

// All callers use initialized fixed-width ABI records with no padding.
fn control<T: Copy>(ops: &dyn ControlOps, opcode: u32, mut value: T) -> T {
    let task = task::mytask().unwrap();
    let bytes = unsafe {
        std::slice::from_raw_parts_mut((&mut value as *mut T).cast::<u8>(), size_of::<T>())
    };
    copy_to_user(&task, ARG, bytes).unwrap();
    ops.control(opcode, ARG).unwrap();
    copy_from_user(&task, ARG, bytes).unwrap();
    assert_eq!(
        u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
        GPU_RESULT_SUCCESS,
        "GPU control {opcode:#x}"
    );
    value
}

fn exercise() {
    let task = task::mytask().unwrap();
    let dev = DeviceManager::get_manager()
        .get_device_by_name("gpu0")
        .unwrap()
        .open()
        .unwrap();
    let dev = dev.as_char_device().unwrap();
    let info = control(dev, GPU_QUERY_INFO, GpuQueryInfo::new());
    assert_eq!(
        &info.backend_id[..info.backend_id_len as usize],
        wire::BACKEND
    );
    let d = control(dev, GPU_QUERY_DIALECT, GpuQueryDialect::new(0));
    let ctx = control(
        dev,
        GPU_CREATE_CONTEXT,
        GpuCreateContext::new(0, d.dialect_token),
    );
    let ctx = task.handle_table.get_arc_clone(ctx.context_handle).unwrap();
    let ctx = ctx.as_gpu().unwrap().as_control_ops().unwrap();
    let queue = control(ctx, GPU_CREATE_QUEUE, GpuCreateQueue::new());
    let queue = task.handle_table.get_arc_clone(queue.queue_handle).unwrap();
    let queue = queue.as_gpu().unwrap().as_control_ops().unwrap();
    let (w, h) = (13, 7);
    let image = control(
        dev,
        GPU_CREATE_IMAGE,
        GpuCreateImage::new_with_usage(
            w,
            h,
            GPU_IMAGE_USAGE_PRESENTABLE
                | GPU_IMAGE_USAGE_RENDER_TARGET
                | GPU_IMAGE_USAGE_TRANSFER_SRC,
        ),
    );
    let attach = control(
        ctx,
        GPU_CONTEXT_ATTACH_IMAGE,
        GpuContextAttachImage::new(image.image_handle),
    );
    let staging = control(
        dev,
        GPU_CREATE_IMAGE,
        GpuCreateImage::new_with_usage(
            w,
            h,
            GPU_IMAGE_USAGE_TRANSFER_DST | GPU_IMAGE_USAGE_TRANSFER_SRC,
        ),
    );
    let staging_token = control(
        ctx,
        GPU_CONTEXT_ATTACH_IMAGE,
        GpuContextAttachImage::new(staging.image_handle),
    )
    .command_resource_token;
    // Upload two short rows with guest padding into a fresh image. Reading
    // the whole texture checks both the row packing and untouched zero texels.
    let source = [1, 2, 3, 4, 0xee, 0xee, 0xee, 0xee, 5, 6, 7, 8];
    copy_to_user(&task, DATA, &source).unwrap();
    control(
        ctx,
        GPU_CONTEXT_UPLOAD_IMAGE_BGRA,
        GpuContextUploadImageBgra::new(staging.image_handle, DATA as u64, 12, 8, 2, 1, 1, 2),
    );
    let mut fresh = vec![0xff; (w * h * 4) as usize];
    control(
        ctx,
        GPU_CONTEXT_READBACK_IMAGE_BGRA,
        GpuContextReadbackImageBgra::new(
            staging.image_handle,
            DATA as u64,
            fresh.len() as u64,
            w * 4,
            0,
            0,
            w,
            h,
        ),
    );
    copy_from_user(&task, DATA, &mut fresh).unwrap();
    let mut expected = vec![0; fresh.len()];
    for (y, pixel) in [(1, &source[..4]), (2, &source[8..])] {
        let start = ((y * w + 2) * 4) as usize;
        expected[start..start + 4].copy_from_slice(pixel);
    }
    assert_eq!(fresh, expected);
    let table = ResourceTable::new();
    let buffer = table
        .define_buffer(BufferDesc::new(32, BufferUsage::COPY_DST | BufferUsage::COPY_SRC).unwrap())
        .unwrap();
    let target = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                Extent2D::new(w, h).unwrap(),
                TextureUsage::PRESENT | TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
            )
            .unwrap(),
        )
        .unwrap();
    let send = |words: &[u64]| {
        let bytes = wire::as_bytes(words);
        copy_to_user(&task, DATA, bytes).unwrap();
        control(
            queue,
            GPU_QUEUE_SUBMIT,
            GpuQueueSubmit::new(DATA as u64, bytes.len() as u32),
        );
    };
    let mut snapshot = vec![wire::SNAPSHOT];
    snapshot.extend_from_slice(&table.abi_snapshot().unwrap());
    send(&snapshot);
    send(&[
        wire::MAP,
        target.slot() as u64,
        attach.command_resource_token,
    ]);
    table.enable_abi_commands();
    let pattern: [u8; 32] = std::array::from_fn(|i| 3 * i as u8 + 17);
    let mut upload = CommandEncoder::new(&table);
    upload.write_buffer(buffer, 0, &pattern).unwrap();
    let commands = upload.finish().unwrap();
    let batch = commands.abi_batch().unwrap();
    let mut words = vec![
        wire::EXECUTE,
        batch.table,
        batch.count as u64,
        batch.words.len as u64,
    ];
    words.extend_from_slice(unsafe { batch.words.as_slice() });
    wire::uploads(&mut words[4..], batch.count, |offset, size| {
        assert_eq!(size, pattern.len());
        *offset = 0;
        Ok(())
    })
    .unwrap();
    for chunk in pattern.chunks_exact(8) {
        words.push(u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    // Fragment across the recording/payload boundary to exercise in-place
    // relocation in the host's reassembled packet. Later clears use direct submits.
    send(&[wire::START, words.len() as u64]);
    for chunk in words.chunks(3) {
        let mut packet = vec![wire::APPEND];
        packet.extend_from_slice(chunk);
        send(&packet);
    }
    send(&[wire::FINISH]);
    let bytes_image = control(
        dev,
        GPU_CREATE_IMAGE,
        GpuCreateImage::new_with_usage(
            3,
            1,
            GPU_IMAGE_USAGE_TRANSFER_SRC | GPU_IMAGE_USAGE_TRANSFER_DST,
        ),
    );
    let bytes_token = control(
        ctx,
        GPU_CONTEXT_ATTACH_IMAGE,
        GpuContextAttachImage::new(bytes_image.image_handle),
    )
    .command_resource_token;
    send(&[wire::READ_BUFFER, buffer.slot() as u64, 5, 11, bytes_token]);
    control(
        ctx,
        GPU_CONTEXT_READBACK_IMAGE_BGRA,
        GpuContextReadbackImageBgra::new(bytes_image.image_handle, DATA as u64, 12, 12, 0, 0, 3, 1),
    );
    let mut read = [0; 12];
    copy_from_user(&task, DATA, &mut read).unwrap();
    assert_eq!(&read[..11], &pattern[5..16]);
    assert_eq!(read[11], 0);
    let area = PixelRect::new(0, 0, w, h).unwrap();
    for red in [true, false] {
        let mut encoder = CommandEncoder::new(&table);
        let color = if red {
            Color::rgba(1., 0., 0., 1.)
        } else {
            Color::rgba(0., 1., 0., 1.)
        }
        .unwrap();
        encoder
            .begin_render_pass(
                RenderPassDesc::new(&table, target, area, LoadOp::Clear(color), StoreOp::Store)
                    .unwrap(),
            )
            .unwrap()
            .end()
            .unwrap();
        let recording = encoder.finish().unwrap();
        let batch = recording.abi_batch().unwrap();
        let mut words = vec![
            wire::EXECUTE,
            batch.table,
            batch.count as u64,
            batch.words.len as u64,
        ];
        words.extend_from_slice(unsafe { batch.words.as_slice() });
        let bytes = wire::as_bytes(&words);
        copy_to_user(&task, DATA, bytes).unwrap();
        let submit = control(
            queue,
            GPU_QUEUE_SUBMIT_ASYNC,
            GpuQueueSubmitAsync {
                abi_version: GPU_ABI_VERSION,
                result: 0,
                flags: 0,
                reserved: 0,
                command_ptr: DATA as u64,
                command_size: bytes.len() as u32,
                completion_handle: 0,
                accepted: 0,
                reserved2: 0,
            },
        );
        assert_eq!(submit.accepted, 1);
        // Readback must wait for the accepted GPU write even if its observer is idle.
        let mut pixels = vec![0xa5; (w * h * 4) as usize];
        control(
            ctx,
            GPU_CONTEXT_READBACK_IMAGE_BGRA,
            GpuContextReadbackImageBgra::new(
                image.image_handle,
                DATA as u64,
                pixels.len() as u64,
                w * 4,
                0,
                0,
                w,
                h,
            ),
        );
        copy_from_user(&task, DATA, &mut pixels).unwrap();
        let expected = if red {
            [0, 0, 255, 255]
        } else {
            [0, 255, 0, 255]
        };
        assert!(pixels.chunks_exact(4).all(|p| p == expected));
        // The Vulkan driver reads logical resources through its own attached
        // destination, then imports the tightly packed bytes into guest memory.
        send(&[
            wire::READ_TEXTURE,
            target.slot() as u64,
            0,
            pixels.len() as u64,
            staging_token,
        ]);
        control(
            ctx,
            GPU_CONTEXT_READBACK_IMAGE_BGRA,
            GpuContextReadbackImageBgra::new(
                staging.image_handle,
                DATA as u64,
                pixels.len() as u64,
                w * 4,
                0,
                0,
                w,
                h,
            ),
        );
        pixels.fill(0xa5);
        copy_from_user(&task, DATA, &mut pixels).unwrap();
        assert!(pixels.chunks_exact(4).all(|p| p == expected));
        let receipt = task
            .handle_table
            .get_arc_clone(submit.completion_handle)
            .unwrap();
        let info = control(
            receipt.as_gpu().unwrap().as_control_ops().unwrap(),
            GPU_COMPLETION_QUERY,
            GpuCompletionInfo::new(),
        );
        assert_eq!(info.state, GPU_COMPLETION_COMPLETE);
        task.handle_table.remove(submit.completion_handle);
    }
    process::exit(0)
}
