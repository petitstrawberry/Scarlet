//! Timed animations require an explicitly recognized, actually selected GPU.
pub(super) fn backend_is_accelerated(selected_backend: &str) -> bool {
    matches!(selected_backend,
        "wgpu" | "metal" | "scarlet-virgl" | "scarlet-adreno" | "scarlet-maxwell")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_gpu_backends_include_dynamically_loaded_maxwell() {
        for name in ["wgpu", "metal", "scarlet-virgl", "scarlet-adreno", "scarlet-maxwell"] {
            assert!(backend_is_accelerated(name), "{name}");
        }
    }

    #[test]
    fn unknown_or_software_backend_does_not_enable_timed_animation() {
        for name in ["", "cpu", "software", "dynamic", "unknown-plugin", "scarlet-maxwell-software"] {
            assert!(!backend_is_accelerated(name), "{name}");
        }
    }
}
