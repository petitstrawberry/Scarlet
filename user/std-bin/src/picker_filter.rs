//! Extension-list wire filter used by Files and advertised through sbus.
//! Keep legacy MIME filters unchanged. Literal extensions have no leading dot.
pub const EXTENSION_FILTER_PREFIX: &str = "extensions:";
pub const EXTENSION_FILTER_CAPABILITY: &str = "extension-list-v1";

pub fn valid_filter(filter: &str) -> bool {
    let Some(list) = filter.strip_prefix(EXTENSION_FILTER_PREFIX) else {
        return true;
    };
    list.split(',').all(|extension| {
        !extension.is_empty()
            && extension
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    })
}

/// None denotes a legacy MIME filter. Malformed extension lists fail closed.
pub fn matches_extensions(name: &str, filter: &str) -> Option<bool> {
    let list = filter.strip_prefix(EXTENSION_FILTER_PREFIX)?;
    Some(
        valid_filter(filter)
            && name.rsplit_once('.').is_some_and(|(_, extension)| {
                list.split(',')
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            }),
    )
}

pub fn validate_selection(
    name: &str,
    is_directory: bool,
    select_directories: bool,
    save: bool,
    filter: &str,
) -> Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err("Choose a valid file name");
    }
    if is_directory {
        return if select_directories && !save {
            Ok(())
        } else {
            Err("Choose a file, not a folder")
        };
    }
    if select_directories {
        return Err("Choose a folder");
    }
    if matches_extensions(name, filter) == Some(false) {
        return Err("File name does not match the requested extensions");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_union_is_literal_and_case_insensitive() {
        let filter = "extensions:wav,flac,json";
        for name in ["sound.WAV", "sound.flac", "session.resonara.JSON"] {
            assert_eq!(matches_extensions(name, filter), Some(true));
        }
        for name in ["sound.wav.bak", "notes.txt", "wav", "sound."] {
            assert_eq!(matches_extensions(name, filter), Some(false));
        }
        assert_eq!(matches_extensions("sound.wav", "video/*"), None);
        assert!(valid_filter("video/*"));
    }
    #[test]
    fn malformed_extension_lists_never_match() {
        for filter in [
            "extensions:",
            "extensions:wav,",
            "extensions:,wav",
            "extensions:*.wav",
            "extensions:wa/v",
            "extensions:wav, flac",
            "extensions:wav\0",
            "extensions:.wav",
        ] {
            assert!(!valid_filter(filter));
            assert_eq!(matches_extensions("sound.wav", filter), Some(false));
        }
    }
    #[test]
    fn acceptance_rejects_wrong_extension_and_keeps_folder_navigation_separate() {
        for save in [false, true] {
            assert!(validate_selection("mix.WAV", false, false, save, "extensions:wav").is_ok());
            assert!(validate_selection("mix.txt", false, false, save, "extensions:wav").is_err());
            assert!(validate_selection("folder.wav", true, false, save, "extensions:wav").is_err());
        }
        assert!(validate_selection("folder", true, true, false, "extensions:wav").is_ok());
        for name in ["", ".", "..", "../mix.wav", "sub\\mix.wav", "mix\0.wav"] {
            assert!(validate_selection(name, false, false, true, "extensions:wav").is_err());
        }
    }
}
