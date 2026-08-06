use super::*;

#[cfg(feature = "vp9-stateless-hw")]
use core::mem;
#[cfg(feature = "vp9-stateless-hw")]
use scarlet_codecs::{ScarletVideoVp9StatelessParams, Vp9RequestContext};
use std::fs::File;
#[cfg(feature = "vp9-stateless-hw")]
use std::sync::OnceLock;

#[cfg(feature = "vp9-stateless-hw")]
const SCARLET_VIDEO_SUBMIT_VP9_STATELESS: u32 = 0x5609;

#[cfg(feature = "vp9-stateless-hw")]
static VP9_STATELESS_DUMP_DIR: OnceLock<String> = OnceLock::new();

#[cfg(feature = "vp9-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoVp9ParamPtrs {
    frame: u64,
    probabilities: u64,
    tiles: u64,
}

#[cfg(feature = "vp9-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoVp9StatelessSubmit {
    stream_id: u32,
    input_len: u32,
    timestamp: u64,
    params: ScarletVideoVp9ParamPtrs,
    flags: u32,
    padding: u32,
}

#[cfg(feature = "vp9-stateless-hw")]
#[derive(Default)]
pub struct Context {
    request: Vp9RequestContext,
}

#[cfg(not(feature = "vp9-stateless-hw"))]
#[derive(Default)]
pub struct Context;

#[cfg(feature = "vp9-stateless-hw")]
pub fn enable_dump(path: &str) {
    if path.is_empty() {
        return;
    }
    let _ = std::fs::create_directory(path);
    let _ = VP9_STATELESS_DUMP_DIR.set(String::from(path));
    println!("[{}] VP9 stateless dump {}", APP_NAME, path);
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub fn enable_dump(path: &str) {
    let _ = path;
    println!(
        "[{}] --dump-vp9-stateless ignored because vp9-stateless-hw is disabled",
        APP_NAME
    );
}

#[cfg(feature = "vp9-stateless-hw")]
pub fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    caps.map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_VP9))
        .unwrap_or(false)
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    let _ = caps;
    false
}

pub fn reset_for_discontinuity(context: &mut Context) {
    *context = Context::default();
}

#[cfg(feature = "vp9-stateless-hw")]
pub fn submit(
    device: &mut File,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    debug_submits: &mut u32,
) -> Result<bool, String> {
    let log_submit = *debug_submits < 4;
    if log_submit {
        println!(
            "[{}] VP9 stateless prepare input={} stream={}",
            APP_NAME,
            access_unit.len(),
            stream_id
        );
    }
    let vp9 = context.request.params_for_frame(access_unit)?;
    let params: &ScarletVideoVp9StatelessParams = &vp9.params;
    let should_display =
        params.frame.flags & scarlet_codecs::SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME != 0;
    if log_submit {
        println!(
            "[{}] VP9 stateless parsed ts={} key={} show={} size={}x{} render={}x{} tiles={} uh={} ch={} flags=0x{:x}",
            APP_NAME,
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
        && let Err(err) = dump_stateless_request(dir, vp9.timestamp, access_unit, params)
    {
        println!("[{}] VP9 stateless dump failed: {}", APP_NAME, err);
    }
    if log_submit {
        println!(
            "[{}] VP9 stateless submit begin ts={} stream={}",
            APP_NAME, vp9.timestamp, stream_id
        );
    }
    device
        .as_handle()
        .control(
            SCARLET_VIDEO_SUBMIT_VP9_STATELESS,
            &submit as *const _ as usize,
        )
        .map_err(|_| {
            let status = read_decoder_status(device);
            format!("hardware decoder stateless VP9 submit failed{status}")
        })?;
    if log_submit {
        println!(
            "[{}] VP9 stateless submit ok ts={} stream={}",
            APP_NAME, vp9.timestamp, stream_id
        );
    }
    *debug_submits = debug_submits.saturating_add(1);
    Ok(should_display)
}

#[cfg(not(feature = "vp9-stateless-hw"))]
pub fn submit(
    device: &mut File,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    debug_submits: &mut u32,
) -> Result<bool, String> {
    let _ = (device, context, stream_id, access_unit, debug_submits);
    Err(String::from("stateless VP9 hardware decode is disabled"))
}

#[cfg(feature = "vp9-stateless-hw")]
fn join_dump_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{}{}", dir, name)
    } else {
        format!("{}/{}", dir, name)
    }
}

#[cfg(feature = "vp9-stateless-hw")]
fn struct_bytes<T>(value: &T) -> &[u8] {
    // SAFETY: Scarlet video request structs are #[repr(C)] plain data copied
    // through the kernel ABI. The byte slice is only used synchronously for a
    // diagnostic dump while `value` is still alive.
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, mem::size_of::<T>()) }
}

#[cfg(feature = "vp9-stateless-hw")]
fn dump_file(path: &str, data: &[u8]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|err| format!("create {} failed: {}", path, err))?;
    file.write_all(data)
        .map_err(|err| format!("write {} failed: {}", path, err))
}

#[cfg(feature = "vp9-stateless-hw")]
fn dump_stateless_request(
    dir: &str,
    timestamp: u64,
    access_unit: &[u8],
    params: &ScarletVideoVp9StatelessParams,
) -> Result<(), String> {
    let prefix = format!("scarlet-vp9.{:016x}", timestamp);
    dump_file(
        &join_dump_path(dir, &format!("{}.input.bin", prefix)),
        access_unit,
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{}.frame-params.bin", prefix)),
        struct_bytes(&params.frame),
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{}.probs.bin", prefix)),
        &params.probabilities.data,
    )?;
    dump_file(
        &join_dump_path(dir, &format!("{}.tiles.bin", prefix)),
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
        &join_dump_path(dir, &format!("{}.manifest.txt", prefix)),
        manifest.as_bytes(),
    )
}

#[cfg(feature = "vp9-stateless-hw")]
fn read_decoder_status(device: &mut File) -> String {
    let mut buffer = [0u8; 512];
    match device.read(&mut buffer) {
        Ok(0) | Err(_) => String::new(),
        Ok(read) => {
            let status = core::str::from_utf8(&buffer[..read]).unwrap_or("<non-utf8 status>");
            format!("; {status}")
        }
    }
}
