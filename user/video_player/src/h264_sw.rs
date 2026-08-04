use super::*;

#[cfg(feature = "h264-sw")]
use core::simd::Simd;
#[cfg(feature = "h264-sw")]
use rust_h264::decoder::{Frame, OrderedDecoder};
#[cfg(feature = "h264-sw")]
use rust_h264::nal::parse_annex_b;

#[cfg(feature = "h264-sw")]
pub struct DecodedFrame {
    frame: Frame,
}

#[cfg(not(feature = "h264-sw"))]
pub enum DecodedFrame {}

#[cfg(feature = "h264-sw")]
impl DecodedFrame {
    fn new(frame: Frame) -> Self {
        Self { frame }
    }
}

#[cfg(feature = "h264-sw")]
pub fn decode_loop(
    path: &str,
    mp4_data: Option<&[u8]>,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    println!("[{}] loading video source {}", APP_NAME, path);
    let source = load_video_source(path, mp4_data)?;
    if source
        .access_units
        .iter()
        .any(|unit| unit.codec != VideoCodec::H264)
    {
        return Err(String::from(
            "software decoder supports only H.264; use --hwdc for this video",
        ));
    }
    let total_frames = video_source_total_frames(&source);
    let mut access_unit_scratch = Vec::new();
    println!(
        "[{}] software decode: {} {} access units",
        APP_NAME,
        source.description(),
        source.access_units.len()
    );
    let loop_duration_us = video_source_duration_us(&source);
    controls.set_media_duration_us(loop_duration_us);
    controls.set_buffered_position_us(loop_duration_us);
    let mut loop_index = 0u64;
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = 0u64;

    loop {
        if queue.is_cancelled() {
            return Ok(());
        }
        let mut decoder = OrderedDecoder::<u64>::new();
        let seek_plan = video_seek_plan(&source, seek_target_us);
        let mut display_index = seek_plan.publish_start_rank;
        let loop_time_offset_us = video_loop_time_offset_us(clock, loop_duration_us, loop_index);
        let mut restart_for_seek = false;

        for access_unit in &source.access_units[seek_plan.decode_start_index..] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                loop_index = 0;
                restart_for_seek = true;
                break;
            }
            wait_while_paused(controls, queue);
            let access_unit_bytes = access_unit.bytes(mp4_data, &mut access_unit_scratch)?;
            let nals = parse_annex_b(access_unit_bytes);
            let unit_presentation_time_us = access_unit
                .presentation_time_us
                .saturating_add(loop_time_offset_us);
            for nal in &nals {
                match decoder.decode_nal_with_meta(nal, unit_presentation_time_us) {
                    Ok(frames) => {
                        for (frame, presentation_time_us) in frames {
                            if presentation_time_us
                                < seek_plan
                                    .publish_target_us
                                    .saturating_add(loop_time_offset_us)
                            {
                                continue;
                            }
                            match queue.push_frame(
                                DisplayItem::frame(
                                    DecodedVideoFrame::Software(DecodedFrame::new(frame)),
                                    presentation_time_us,
                                    display_index,
                                    total_frames,
                                    seek_epoch,
                                ),
                                controls,
                            ) {
                                QueuePush::Pushed => {
                                    display_index += 1;
                                }
                                QueuePush::StaleEpoch => {
                                    queue.clear();
                                    seek_target_us = controls.current_seek_target_us();
                                    seek_epoch = controls.current_seek_epoch();
                                    loop_index = 0;
                                    restart_for_seek = true;
                                    break;
                                }
                                QueuePush::Closed => return Ok(()),
                            }
                        }
                        if restart_for_seek {
                            break;
                        }
                    }
                    Err(err) => return Err(format!("decode failed: {err}")),
                }
            }
            if restart_for_seek {
                break;
            }
        }

        if restart_for_seek {
            continue;
        }
        for (frame, presentation_time_us) in decoder.flush_with_meta() {
            if presentation_time_us
                < seek_plan
                    .publish_target_us
                    .saturating_add(loop_time_offset_us)
            {
                continue;
            }
            match queue.push_frame(
                DisplayItem::frame(
                    DecodedVideoFrame::Software(DecodedFrame::new(frame)),
                    presentation_time_us,
                    display_index,
                    total_frames,
                    seek_epoch,
                ),
                controls,
            ) {
                QueuePush::Pushed => {
                    display_index += 1;
                }
                QueuePush::StaleEpoch => {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
                QueuePush::Closed => return Ok(()),
            }
        }

        if restart_for_seek {
            continue;
        }

        if !controls.is_loop_enabled() {
            match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
                QueuePush::Pushed => {}
                QueuePush::StaleEpoch => {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    continue;
                }
                QueuePush::Closed => return Ok(()),
            }
            println!("[{}] finished: {} frames", APP_NAME, display_index);
            let replay_or_seek = wait_for_replay_or_seek_request(controls, queue);
            if queue.is_cancelled() {
                return Ok(());
            }
            seek_target_us = replay_or_seek.unwrap_or(0);
            if let Some(clock) = clock {
                clock.reset_for_replay();
            }
            queue.clear();
            seek_epoch = controls.current_seek_epoch();
            loop_index = 0;
            continue;
        }
        println!(
            "[{}] loop {} complete: {} frames",
            APP_NAME,
            loop_index + 1,
            display_index
        );
        loop_index = loop_index.saturating_add(1);
    }
}

#[cfg(not(feature = "h264-sw"))]
pub fn decode_loop(
    path: &str,
    mp4_data: Option<&[u8]>,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    let _ = (path, mp4_data, controls, clock, queue);
    Err(String::from(
        "software H.264 decode is disabled; rebuild video_player with the h264-sw feature or use --hwdc with a stateful H.264 backend",
    ))
}

#[cfg(feature = "h264-sw")]
pub fn estimated_bytes(frame: &DecodedFrame) -> usize {
    frame.frame.width as usize * frame.frame.height as usize * 3 / 2
}

#[cfg(not(feature = "h264-sw"))]
pub fn estimated_bytes(frame: &DecodedFrame) -> usize {
    let _ = frame;
    0
}

#[cfg(feature = "h264-sw")]
pub fn update_frame_store(
    frame_store: &VideoFrameStore,
    frame: &DecodedFrame,
    current_frame: u32,
    total_frames: u32,
) -> Result<(), String> {
    let width = frame.frame.width;
    let height = frame.frame.height;
    let mut data = frame_store.data.lock();
    let required_len = width as usize * height as usize * 4;
    if data.pixels.len() != required_len {
        data.pixels.resize(required_len, 0);
    }
    yuv420_to_bgra(&frame.frame, &mut data.pixels);
    data.width = width;
    data.height = height;
    data.current_frame = current_frame;
    data.total_frames = total_frames;
    Ok(())
}

#[cfg(not(feature = "h264-sw"))]
pub fn update_frame_store(
    frame_store: &VideoFrameStore,
    frame: &DecodedFrame,
    current_frame: u32,
    total_frames: u32,
) -> Result<(), String> {
    let _ = (frame_store, frame, current_frame, total_frames);
    Err(String::from(
        "software H.264 frame produced without h264-sw",
    ))
}

#[cfg(feature = "h264-sw")]
fn yuv420_to_bgra(frame: &Frame, pixels: &mut [u8]) {
    yuv420_to_bgra_simd(frame, pixels);
}

#[cfg(feature = "h264-sw")]
fn yuv420_to_bgra_simd(frame: &Frame, pixels: &mut [u8]) {
    const LANES: usize = 8;

    let width = frame.width as usize;
    let height = frame.height as usize;
    let chroma_width = width / 2;

    for y in 0..height {
        let y_row = y * width;
        let uv_row = (y / 2) * chroma_width;
        let mut x = 0usize;

        while x + LANES <= width {
            let y_values =
                Simd::<u8, LANES>::from_slice(&frame.y[y_row + x..y_row + x + LANES]).cast::<i32>();
            let u_base = uv_row + x / 2;
            let v_base = uv_row + x / 2;
            let u_values = Simd::<i32, LANES>::from_array([
                frame.u[u_base] as i32,
                frame.u[u_base] as i32,
                frame.u[u_base + 1] as i32,
                frame.u[u_base + 1] as i32,
                frame.u[u_base + 2] as i32,
                frame.u[u_base + 2] as i32,
                frame.u[u_base + 3] as i32,
                frame.u[u_base + 3] as i32,
            ]);
            let v_values = Simd::<i32, LANES>::from_array([
                frame.v[v_base] as i32,
                frame.v[v_base] as i32,
                frame.v[v_base + 1] as i32,
                frame.v[v_base + 1] as i32,
                frame.v[v_base + 2] as i32,
                frame.v[v_base + 2] as i32,
                frame.v[v_base + 3] as i32,
                frame.v[v_base + 3] as i32,
            ]);

            let (r, g, b) = yuv_to_rgb_simd(y_values, u_values, v_values);
            store_bgra8(pixels, (y_row + x) * 4, r, g, b);

            x += LANES;
        }

        while x < width {
            let y_value = frame.y[y_row + x] as i32;
            let u_value = frame.u[uv_row + x / 2] as i32;
            let v_value = frame.v[uv_row + x / 2] as i32;
            let (r, g, b) = yuv_to_rgb(y_value, u_value, v_value);
            let offset = (y_row + x) * 4;
            pixels[offset] = b;
            pixels[offset + 1] = g;
            pixels[offset + 2] = r;
            pixels[offset + 3] = 255;
            x += 1;
        }
    }
}
