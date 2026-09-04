//! Stateless H.264 request preparation and Scarlet ABI submission.

#[cfg(feature = "h264-stateless-hw")]
use alloc::format;
use alloc::string::String;

#[cfg(feature = "h264-stateless-hw")]
use scarlet_codecs::H264RequestContext;
use scarlet_os::handle::Handle;

use crate::abi::ScarletVideoCapabilities;
#[cfg(feature = "h264-stateless-hw")]
use crate::abi::{
    SCARLET_VIDEO_CAP_STATELESS_H264, SCARLET_VIDEO_SUBMIT_H264_STATELESS,
    ScarletVideoH264ParamPtrs, ScarletVideoH264StatelessSubmit,
};
#[cfg(feature = "h264-stateless-hw")]
use crate::read_device;

#[cfg(feature = "h264-stateless-hw")]
#[derive(Default)]
pub(crate) struct Context {
    request: H264RequestContext,
}

#[cfg(not(feature = "h264-stateless-hw"))]
#[derive(Default)]
pub(crate) struct Context;

#[cfg(feature = "h264-stateless-hw")]
pub(crate) fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    caps.map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_H264))
        .unwrap_or(false)
}

#[cfg(not(feature = "h264-stateless-hw"))]
pub(crate) fn supported(caps: Option<ScarletVideoCapabilities>) -> bool {
    let _ = caps;
    false
}

#[cfg(feature = "h264-stateless-hw")]
pub(crate) fn reset_for_discontinuity(context: &mut Context) {
    context.request.reset_decode_state();
}

#[cfg(not(feature = "h264-stateless-hw"))]
pub(crate) fn reset_for_discontinuity(context: &mut Context) {
    let _ = context;
}

#[cfg(feature = "h264-stateless-hw")]
pub(crate) fn submit(
    device: &Handle,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    timestamp: u64,
) -> Result<u64, String> {
    let h264 = context
        .request
        .params_for_access_unit_with_timestamp(access_unit, timestamp)?;
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
        .control(
            SCARLET_VIDEO_SUBMIT_H264_STATELESS,
            &submit as *const _ as usize,
        )
        .map_err(|_| {
            let status = read_decoder_status(device);
            format!("hardware decoder stateless H.264 submit failed{status}")
        })?;
    Ok(h264.timestamp)
}

#[cfg(not(feature = "h264-stateless-hw"))]
pub(crate) fn submit(
    device: &Handle,
    context: &mut Context,
    stream_id: u32,
    access_unit: &[u8],
    timestamp: u64,
) -> Result<u64, String> {
    let _ = (device, context, stream_id, access_unit, timestamp);
    Err(String::from("stateless H.264 hardware decode is disabled"))
}

#[cfg(feature = "h264-stateless-hw")]
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
