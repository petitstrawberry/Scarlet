//! Stateless VP9 request preparation, diagnostics, and ABI submission.

#[cfg(feature = "vp9-stateless-hw")]
use alloc::format;
use alloc::string::String;

#[cfg(feature = "vp9-stateless-hw")]
use core::mem;
#[cfg(feature = "vp9-stateless-hw")]
use scarlet_codecs::{ScarletVideoVp9StatelessParams, Vp9RequestContext};
use scarlet_os::handle::Handle;
#[cfg(feature = "vp9-stateless-hw")]
use std::fs::File;
#[cfg(all(feature = "std", feature = "vp9-stateless-hw"))]
use std::io::Write;
#[cfg(feature = "vp9-stateless-hw")]
use std::sync::OnceLock;

use crate::abi::ScarletVideoCapabilities;
#[cfg(feature = "vp9-stateless-hw")]
use crate::abi::{
    SCARLET_VIDEO_CAP_STATELESS_VP9, SCARLET_VIDEO_SUBMIT_VP9_STATELESS, ScarletVideoVp9ParamPtrs,
    ScarletVideoVp9StatelessSubmit,
};
#[cfg(feature = "vp9-stateless-hw")]
use crate::read_device;

#[cfg(feature = "vp9-stateless-hw")]
static VP9_STATELESS_DUMP_DIR: OnceLock<String> = OnceLock::new();

#[cfg(all(feature = "std", feature = "vp9-stateless-hw"))]
fn create_dump_directory(path: &str) {
    let _ = std::fs::create_dir_all(path);
}

#[cfg(all(feature = "legacy-scarlet-std", feature = "vp9-stateless-hw"))]
fn create_dump_directory(path: &str) {
    let _ = std::fs::create_directory(path);
}

#[cfg(feature = "vp9-stateless-hw")]
#[derive(Default)]
pub(crate) struct Context {
    request: Vp9RequestContext,
}

#[cfg(not(feature = "vp9-stateless-hw"))]
#[derive(Default)]
pub(crate) struct Context;

#[cfg(feature = "vp9-stateless-hw")]
pub(crate) fn enable_dump(path: &str) {
    if path.is_empty() {
        return;
    }
    create_dump_directory(path);
    let _ = VP9_STATELESS_DUMP_DIR.set(String::from(path));
    std::println!("[scarlet-video-client] VP9 stateless dump {path}");
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub(crate) fn enable_dump(path: &str) {
    let _ = path;
    std::println!(
        "[scarlet-video-client] VP9 stateless dump ignored because vp9-stateless-hw is disabled"
    );
}

#[cfg(feature = "vp9-stateless-hw")]
pub(crate) fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    caps.map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_VP9))
        .unwrap_or(false)
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub(crate) fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    let _ = caps;
    false
}

pub(crate) fn reset_for_discontinuity(context: &mut Context) {
    *context = Context::default();
}

#[cfg(feature = "vp9-stateless-hw")]
pub(crate) fn submit(
    device: &Handle,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    timestamp: u64,
    debug_submits: &mut u32,
) -> Result<(bool, u64), String> {
    let log_submit = *debug_submits < 4;
    if log_submit {
        std::println!(
            "[scarlet-video-client] VP9 stateless prepare input={} stream={}",
            access_unit.len(),
            stream_id
        );
    }
    let vp9 = context
        .request
        .params_for_frame_with_timestamp(access_unit, timestamp)?;
    let params: &ScarletVideoVp9StatelessParams = &vp9.params;
    let should_display =
        params.frame.flags & scarlet_codecs::SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME != 0;
    if log_submit {
        std::println!(
            "[scarlet-video-client] VP9 stateless parsed ts={} key={} show={} size={}x{} render={}x{} tiles={} uh={} ch={} flags=0x{:x}",
            vp9.timestamp,
            params.frame.flags & scarlet_codecs::SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0,
            should_display,
            u32::from(params.frame.frame_width_minus_1) + 1,
            u32::from(params.frame.frame_height_minus_1) + 1,
            u32::from(params.frame.render_width_minus_1) + 1,
            u32::from(params.frame.render_height_minus_1) + 1,
            params.tiles.tile_count,
            params.frame.uncompressed_header_size,
            params.frame.compressed_header_size,
            params.frame.flags
        );
    }
    let submit = ScarletVideoVp9StatelessSubmit {
        stream_id,
        input_len: access_unit.len() as u32,
        timestamp: vp9.timestamp,
        params: ScarletVideoVp9ParamPtrs {
            frame: &params.frame as *const _ as usize as u64,
            probabilities: &params.probabilities as *const _ as usize as u64,
            tiles: &params.tiles as *const _ as usize as u64,
        },
        flags: 0,
        padding: 0,
    };
    if let Some(dir) = VP9_STATELESS_DUMP_DIR.get()
        && let Err(error) = dump_stateless_request(dir, vp9.timestamp, access_unit, params)
    {
        std::println!("[scarlet-video-client] VP9 stateless dump failed: {error}");
    }
    if log_submit {
        std::println!(
            "[scarlet-video-client] VP9 stateless submit begin ts={} stream={}",
            vp9.timestamp,
            stream_id
        );
    }
    device
        .control(
            SCARLET_VIDEO_SUBMIT_VP9_STATELESS,
            &submit as *const _ as usize,
        )
        .map_err(|_| {
            let status = read_decoder_status(device);
            format!("hardware decoder stateless VP9 submit failed{status}")
        })?;
    if log_submit {
        std::println!(
            "[scarlet-video-client] VP9 stateless submit ok ts={} stream={}",
            vp9.timestamp,
            stream_id
        );
    }
    *debug_submits = debug_submits.saturating_add(1);
    Ok((should_display, vp9.timestamp))
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub(crate) fn submit(
    device: &Handle,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    timestamp: u64,
    debug_submits: &mut u32,
) -> Result<(bool, u64), String> {
    let _ = (
        device,
        context,
        stream_id,
        access_unit,
        timestamp,
        debug_submits,
    );
    Err(String::from("stateless VP9 hardware decode is disabled"))
}

#[cfg(feature = "vp9-stateless-hw")]
fn join_dump_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(feature = "vp9-stateless-hw")]
fn struct_bytes<T>(value: &T) -> &[u8] {
    // SAFETY: Scarlet video request structs are `repr(C)` plain data. The byte
    // slice is consumed synchronously while `value` remains alive.
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, mem::size_of::<T>()) }
}

#[cfg(feature = "vp9-stateless-hw")]
fn dump_file(path: &str, data: &[u8]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|error| format!("create {path} failed: {error}"))?;
    file.write_all(data)
        .map_err(|error| format!("write {path} failed: {error}"))
}

#[cfg(feature = "vp9-stateless-hw")]
fn dump_stateless_request(
    dir: &str,
    timestamp: u64,
    access_unit: &[u8],
    params: &ScarletVideoVp9StatelessParams,
) -> Result<(), String> {
    let prefix = format!("scarlet-vp9.{timestamp:016x}");
    dump_file(
        &join_dump_path(dir, &format!("{prefix}.input.bin")),
        access_unit,
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{prefix}.frame-params.bin")),
        struct_bytes(&params.frame),
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{prefix}.probs.bin")),
        &params.probabilities.data,
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{prefix}.tiles.bin")),
        struct_bytes(&params.tiles),
    )?;

    let manifest = format!(
        "format=scarlet-vp9-stateless\n\
timestamp={}\n\
input_len={}\n\
frame_width={}\n\
frame_height={}\n\
render_width={}\n\
render_height={}\n\
flags=0x{:x}\n\
profile={}\n\
bit_depth={}\n\
tile_count={}\n\
tile_cols_log2={}\n\
tile_rows_log2={}\n\
uncompressed_header_size={}\n\
compressed_header_size={}\n\
refresh_frame_flags=0x{:x}\n",
        timestamp,
        access_unit.len(),
        u32::from(params.frame.frame_width_minus_1) + 1,
        u32::from(params.frame.frame_height_minus_1) + 1,
        u32::from(params.frame.render_width_minus_1) + 1,
        u32::from(params.frame.render_height_minus_1) + 1,
        params.frame.flags,
        params.frame.profile,
        params.frame.bit_depth,
        params.tiles.tile_count,
        params.frame.tile_cols_log2,
        params.frame.tile_rows_log2,
        params.frame.uncompressed_header_size,
        params.frame.compressed_header_size,
        params.frame.refresh_frame_flags,
    );
    dump_file(
        &join_dump_path(dir, &format!("{prefix}.manifest.txt")),
        manifest.as_bytes(),
    )
}

#[cfg(feature = "vp9-stateless-hw")]
fn read_decoder_status(device: &Handle) -> String {
    let mut buffer = [0u8; 512];
    match read_device(device, &mut buffer) {
        Ok(0) | Err(_) => String::new(),
        Ok(read) => {
            let status = core::str::from_utf8(&buffer[..read]).unwrap_or("<non-utf8 status>");
            format!("; {status}")
        }
    }
}
