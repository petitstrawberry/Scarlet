// Context-owned GPU clock query; the backend supplies retirement and power.
use super::super::{
    GPU_RESULT_DEVICE_LOST, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT,
    GPU_RESULT_INVALID_STATE, GPU_RESULT_SUCCESS, GPU_RESULT_UNSUPPORTED, GpuBackendContext,
    GpuBackendSubmitError,
};
use scarlet_abi::gpu_timestamp::{GPU_TIMESTAMP_ABI_VERSION, GpuContextTimestamp};

pub(super) fn handle(context: &dyn GpuBackendContext, arg: usize) -> Result<i32, &'static str> {
    let mut request: GpuContextTimestamp = super::read_user_value(arg)?;
    sample(&mut request, || context.timestamp_ns());
    super::write_user_value(arg, &request)?;
    Ok(0)
}

fn sample(
    request: &mut GpuContextTimestamp,
    read: impl FnOnce() -> Result<Option<u64>, GpuBackendSubmitError>,
) {
    request.timestamp_ns = 0;
    request.result = if request.abi_version != GPU_TIMESTAMP_ABI_VERSION {
        GPU_RESULT_INVALID_ABI
    } else if request.flags != 0 || request.reserved != 0 {
        GPU_RESULT_INVALID_ARGUMENT
    } else {
        match read() {
            Ok(Some(ns)) => {
                request.timestamp_ns = ns;
                GPU_RESULT_SUCCESS
            }
            Ok(None) | Err(GpuBackendSubmitError::Unavailable(_)) => GPU_RESULT_UNSUPPORTED,
            Err(GpuBackendSubmitError::Rejected(_)) => GPU_RESULT_INVALID_STATE,
            Err(GpuBackendSubmitError::DeviceLost(_)) => GPU_RESULT_DEVICE_LOST,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn invalid_queries_never_sample_gpu_and_clear_stale_output() {
        for (version, flags, reserved, expected) in [
            (0, 0, 0, GPU_RESULT_INVALID_ABI),
            (1, 1, 0, GPU_RESULT_INVALID_ARGUMENT),
            (1, 0, 1, GPU_RESULT_INVALID_ARGUMENT),
        ] {
            let mut request = GpuContextTimestamp::new();
            request.abi_version = version;
            request.flags = flags;
            request.reserved = reserved;
            request.timestamp_ns = u64::MAX;
            sample(&mut request, || panic!("invalid request reached GPU"));
            assert_eq!(request.result, expected);
            assert_eq!(request.timestamp_ns, 0);
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn backend_errors_are_not_successful_zero_timestamps() {
        for (result, expected) in [
            (Ok(None), GPU_RESULT_UNSUPPORTED),
            (
                Err(GpuBackendSubmitError::Unavailable("off")),
                GPU_RESULT_UNSUPPORTED,
            ),
            (
                Err(GpuBackendSubmitError::Rejected("bad")),
                GPU_RESULT_INVALID_STATE,
            ),
            (
                Err(GpuBackendSubmitError::DeviceLost("fault")),
                GPU_RESULT_DEVICE_LOST,
            ),
        ] {
            let mut request = GpuContextTimestamp::new();
            request.timestamp_ns = u64::MAX;
            sample(&mut request, || result);
            assert_eq!(request.result, expected);
            assert_eq!(request.timestamp_ns, 0);
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn successful_sample_preserves_full_width_and_zero_epoch() {
        for ns in [0, 1, 1u64 << 40, u64::MAX] {
            let mut request = GpuContextTimestamp::new();
            request.result = GPU_RESULT_UNSUPPORTED;
            sample(&mut request, || Ok(Some(ns)));
            assert_eq!(request.result, GPU_RESULT_SUCCESS);
            assert_eq!(request.timestamp_ns, ns);
        }
    }
}
