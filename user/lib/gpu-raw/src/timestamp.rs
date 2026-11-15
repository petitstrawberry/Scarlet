use super::*;

impl GpuContext {
    /// Sample a device GPU clock after prior context work retires. Values use
    /// nanoseconds with a GPU-defined epoch; unsupported kernels return error.
    pub fn timestamp_ns(&self) -> HandleResult<u64> {
        let mut request = GpuContextTimestamp::new();
        // SAFETY: shared fixed-width ABI, valid exclusively borrowed output.
        unsafe {
            self.handle.control(
                commands::GPU_CONTEXT_TIMESTAMP,
                &mut request as *mut _ as usize,
            )
        }?;
        result_to_handle_error(request.result)?;
        Ok(request.timestamp_ns)
    }
}
