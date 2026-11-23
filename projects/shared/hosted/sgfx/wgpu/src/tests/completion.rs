//! Real-device acceptance, lifetime and failure-receipt regression tests.

use core::time::Duration;
use std::rc::Rc;
use std::sync::Arc;

use sgfx_core::backend::{CommandSubmitter, Completion, CompletionStatus, SubmitError};
use sgfx_core::ir::{
    Color, CommandEncoder, Extent2D, LoadOp, PixelRect, RenderPassDesc, ResourceTable, StoreOp,
    TextureDesc, TextureFormat, TextureUsage, TextureWrite,
};

use super::HEADLESS_WGPU_TEST_LOCK;
use super::execution::{headless_device, readback_pixels};
use crate::{Error, UnsupportedFeature};

#[test]
fn receipts_outlive_sessions_and_observe_upload_and_render_completion() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    let area = PixelRect::new(0, 0, 4, 4).expect("area");
    let mut results = Vec::new();
    for is_upload in [true, false] {
        let target = table
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(4, 4).expect("extent"),
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::PRESENT
                        | TextureUsage::COPY_DST,
                )
                .expect("descriptor"),
            )
            .expect("target");
        let image = context
            .create_image(4, 4, TextureFormat::Bgra8Unorm)
            .expect("image");
        cache
            .map_image(target.id(), Arc::clone(&image))
            .expect("map image");
        let mut pixels = [23, 41, 197, 255].repeat(16);
        let mut encoder = CommandEncoder::new(&table);
        if is_upload {
            encoder
                .write_texture(target, TextureWrite::new(area, 16, &pixels).expect("write"))
                .expect("upload");
        } else {
            let pass = RenderPassDesc::new(
                &table,
                target,
                area,
                LoadOp::Clear(Color::rgba(0.0, 1.0, 0.0, 1.0).expect("green")),
                StoreOp::Store,
            )
            .expect("pass");
            encoder
                .begin_render_pass(pass)
                .expect("begin")
                .end()
                .expect("end");
        }
        let commands = encoder.finish().expect("finish");
        let receipt = queue
            .executor(&mut cache)
            .submit(&commands)
            .expect("tracked submit");
        drop(commands);
        pixels.fill(0);
        drop(pixels);
        results.push((
            image,
            receipt,
            if is_upload {
                [23, 41, 197, 255]
            } else {
                [0, 255, 0, 255]
            },
        ));
    }
    drop(cache);
    drop(queue);
    drop(context);
    drop(table);
    for (image, receipt, color) in results {
        // A fast GPU may already have completed: never assert timing-dependent Pending.
        assert!(matches!(
            receipt.wait(Some(Duration::ZERO)),
            Ok(CompletionStatus::Pending | CompletionStatus::Complete)
        ));
        assert_eq!(
            receipt.wait(Some(Duration::from_secs(10))),
            Ok(CompletionStatus::Complete)
        );
        assert_eq!(receipt.wait(None), Ok(CompletionStatus::Complete));
        assert_eq!(receipt.poll(), Ok(CompletionStatus::Complete));
        assert_eq!(readback_pixels(&device, &image), vec![color; 16]);
    }
}

#[test]
fn tracked_rejection_preserves_a_receipt_for_possible_staged_work() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    let desc = TextureDesc::new(
        TextureFormat::Bgra8Unorm,
        Extent2D::new(1, 1).expect("extent"),
        TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
    )
    .expect("descriptor");
    let source = table.define_texture(desc).expect("source");
    let target = table.define_texture(desc).expect("target");
    let unaligned = table
        .define_buffer(
            sgfx_core::ir::BufferDesc::new(8, sgfx_core::ir::BufferUsage::COPY_DST)
                .expect("buffer descriptor"),
        )
        .expect("buffer for rejected upload");
    let area = PixelRect::new(0, 0, 1, 1).expect("area");
    let mut encoder = CommandEncoder::new(&table);
    encoder
        .copy_texture_to_texture(source, area, target, area)
        .expect("copy");
    encoder
        .write_buffer(unaligned, 1, &[1, 2, 3, 4])
        .expect("byte-granular buffer upload is valid IR");
    let failed = queue
        .executor(&mut cache)
        .submit(&encoder.finish().expect("finish"))
        .expect_err("unsupported unaligned buffer upload");
    let SubmitError::Failed { error, completion } = failed else {
        panic!("failure lost its conservative queue checkpoint");
    };
    assert_eq!(
        error,
        Error::Unsupported(UnsupportedFeature::BufferWriteAlignment)
    );
    assert_eq!(
        completion.wait(Some(Duration::from_secs(10))),
        Ok(CompletionStatus::Complete)
    );
    let empty = CommandEncoder::new(&table).finish().expect("empty");
    let next = queue
        .executor(&mut cache)
        .submit(&empty)
        .expect("valid checkpoint after failure");
    assert_eq!(next.wait(None), Ok(CompletionStatus::Complete));
}

#[test]
fn tracked_submit_rejects_foreign_resource_ownership_without_a_receipt() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let other = ResourceTable::new();
    let mut cache = context.create_resources(table);
    let commands = CommandEncoder::new(&other)
        .finish()
        .expect("foreign commands");
    assert!(matches!(
        queue.executor(&mut cache).submit(&commands),
        Err(SubmitError::Rejected(Error::ResourceTableMismatch))
    ));
}

#[test]
fn tracking_backpressure_is_explicit_and_dropped_receipts_are_safe() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    let commands = CommandEncoder::new(&table).finish().expect("empty");
    // Hold backend tracking leases directly: testing Busy must not depend on
    // whether a real GPU happens to finish between two submissions.
    let mut held = Vec::new();
    while let Some(slot) = device.tracker.reserve() {
        held.push(slot);
    }
    assert!(matches!(
        queue.executor(&mut cache).submit(&commands),
        Err(SubmitError::Busy)
    ));
    drop(held.pop());
    let receipt = queue
        .executor(&mut cache)
        .submit(&commands)
        .expect("available slot");
    drop(receipt);
    let _ = device.raw_device().poll(crate::raw::Maintain::Wait);
    let next = queue
        .executor(&mut cache)
        .submit(&commands)
        .expect("dropped receipt retired safely");
    assert_eq!(next.wait(None), Ok(CompletionStatus::Complete));
}

#[test]
fn device_loss_is_not_reported_as_successful_completion() {
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let queue = context.create_queue();
    let table = Rc::new(ResourceTable::new());
    let mut cache = context.create_resources(Rc::clone(&table));
    let commands = CommandEncoder::new(&table).finish().expect("empty");
    let receipt = queue
        .executor(&mut cache)
        .submit(&commands)
        .expect("tracked checkpoint");
    device.raw_device().destroy();
    let _ = device.raw_device().poll(crate::raw::Maintain::Wait);
    assert_eq!(receipt.poll(), Err(Error::DeviceLost));
    assert_eq!(receipt.wait(None), Err(Error::DeviceLost));
    assert!(matches!(
        queue.executor(&mut cache).submit(&commands),
        Err(SubmitError::Rejected(Error::DeviceLost))
    ));
}

#[test]
fn destroyed_device_rejects_untracked_buffer_and_texture_uploads() {
    use sgfx_core::ir::{BufferDesc, BufferUsage};
    let _guard = HEADLESS_WGPU_TEST_LOCK.lock().expect("lock WGPU tests");
    let Some(device) = headless_device() else {
        return;
    };
    let context = device.create_context();
    let table = Rc::new(ResourceTable::new());
    let buffer = table
        .define_buffer(BufferDesc::new(4, BufferUsage::COPY_DST).unwrap())
        .unwrap();
    let texture = table
        .define_texture(
            TextureDesc::new(
                TextureFormat::Rgba8Unorm,
                Extent2D::new(1, 1).unwrap(),
                TextureUsage::COPY_DST,
            )
            .unwrap(),
        )
        .unwrap();
    let mut cache = context.create_resources(Rc::clone(&table));
    device.raw_device().destroy();
    for buffer_upload in [true, false] {
        let mut encoder = CommandEncoder::new(&table);
        if buffer_upload {
            encoder.write_buffer(buffer, 0, &[1, 2, 3, 4]).unwrap();
        } else {
            encoder
                .write_texture(
                    texture,
                    TextureWrite::new(PixelRect::new(0, 0, 1, 1).unwrap(), 4, &[1, 2, 3, 4])
                        .unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            context
                .create_queue()
                .submit(&mut cache, &encoder.finish().unwrap()),
            Err(Error::DeviceLost)
        );
    }
}
