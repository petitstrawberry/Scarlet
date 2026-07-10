use super::*;

#[cfg(feature = "h264-stateless-hw")]
use scarlet_codecs::H264RequestContext;
use std::fs::File;

#[cfg(feature = "h264-stateless-hw")]
const SCARLET_VIDEO_SUBMIT_H264_STATELESS: u32 = 0x5608;

#[cfg(feature = "h264-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264ParamPtrs {
    sps: u64,
    pps: u64,
    scaling_matrix: u64,
    pred_weights: u64,
    slice_params: u64,
    decode_params: u64,
}

#[cfg(feature = "h264-stateless-hw")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264StatelessSubmit {
    stream_id: u32,
    input_len: u32,
    timestamp: u64,
    params: ScarletVideoH264ParamPtrs,
    flags: u32,
    padding: u32,
}

#[cfg(feature = "h264-stateless-hw")]
#[derive(Default)]
pub struct Context {
    request: H264RequestContext,
}

#[cfg(not(feature = "h264-stateless-hw"))]
#[derive(Default)]
pub struct Context;

#[cfg(feature = "h264-stateless-hw")]
pub fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    caps.map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_H264))
        .unwrap_or(false)
}

#[cfg(not(feature = "h264-stateless-hw"))]
pub fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    let _ = caps;
    false
}

#[cfg(feature = "h264-stateless-hw")]
pub fn submit(
    device: &mut File,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
) -> Result<(), String> {
    let h264 = context.request.params_for_access_unit(access_unit)?;
    let params = &h264.params;
    let submit = ScarletVideoH264StatelessSubmit {
        stream_id,
        input_len: access_unit.len() as u32,
        timestamp: h264.timestamp,
        params: ScarletVideoH264ParamPtrs {
            sps: &params.sps as *const _ as usize as u64,
            pps: &params.pps as *const _ as usize as u64,
            scaling_matrix: &params.scaling_matrix as *const _ as usize as u64,
            pred_weights: &params.pred_weights as *const _ as usize as u64,
            slice_params: &params.slice_params as *const _ as usize as u64,
            decode_params: &params.decode_params as *const _ as usize as u64,
        },
        flags: 0,
        padding: 0,
    };
    device
        .as_handle()
        .control(
            SCARLET_VIDEO_SUBMIT_H264_STATELESS,
            &submit as *const _ as usize,
        )
        .map_err(|_| {
            let status = read_decoder_status(device);
            format!("hardware decoder stateless H.264 submit failed{status}")
        })?;
    Ok(())
}

#[cfg(not(feature = "h264-stateless-hw"))]
pub fn submit(
    device: &mut File,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
) -> Result<(), String> {
    let _ = (device, context, stream_id, access_unit);
    Err(String::from("stateless H.264 hardware decode is disabled"))
}

#[cfg(feature = "h264-stateless-hw")]
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
