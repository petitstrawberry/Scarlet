//! Small settings persistence helpers, also tested against host-only temporary fixtures.
use std::fs;
use std::io::Write;

/// The destination remains intact if writing, flushing, or replacement fails.
pub fn atomic_write(path: &str, staging: &str, content: &[u8]) -> Result<(), &'static str> {
    let result = (|| {
        let mut file =
            fs::File::create(staging).map_err(|_| "could not create temporary config")?;
        file.write_all(content)
            .map_err(|_| "could not write temporary config")?;
        file.flush()
            .map_err(|_| "could not flush temporary config")?;
        drop(file);
        fs::rename(staging, path).map_err(|_| "could not replace config")
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

/// A failed snapshot is not retried until a new edit or an explicit retry.
pub struct Autosave<T> {
    attempted: T,
    pending: Option<T>,
    quiet_ticks: u8,
    retry: u64,
}

impl<T: Clone + PartialEq> Autosave<T> {
    pub fn new(initial: T, retry: u64) -> Self {
        Self {
            attempted: initial,
            pending: None,
            quiet_ticks: 0,
            retry,
        }
    }

    /// Called every 100ms. Save once after edits have been stable for 300ms.
    pub fn poll(&mut self, current: T, retry: u64) -> Option<T> {
        if retry != self.retry {
            self.retry = retry;
            self.attempted = current.clone();
            self.pending = None;
            return Some(current);
        }
        if current == self.attempted {
            self.pending = None;
            return None;
        }
        if self.pending.as_ref() != Some(&current) {
            self.pending = Some(current);
            self.quiet_ticks = 0;
            return None;
        }
        self.quiet_ticks += 1;
        if self.quiet_ticks < 3 {
            return None;
        }
        self.pending = None;
        self.attempted = current.clone();
        Some(current)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TimezoneCommitError {
    Unchanged,
    AppliedWithoutName,
}

/// Both files have already been staged. Never unlink the working localtime first.
pub fn commit_timezone(
    localtime: &str,
    localtime_staging: &str,
    timezone: &str,
    timezone_staging: &str,
    mut rename: impl FnMut(&str, &str) -> bool,
) -> Result<(), TimezoneCommitError> {
    if !rename(localtime_staging, localtime) {
        return Err(TimezoneCommitError::Unchanged);
    }
    if !rename(timezone_staging, timezone) {
        return Err(TimezoneCommitError::AppliedWithoutName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_coalesces_edits_and_does_not_repeat_failed_attempts() {
        let mut save = Autosave::new(0, 0);
        assert_eq!(save.poll(1, 0), None);
        assert_eq!(save.poll(2, 0), None);
        assert_eq!(save.poll(2, 0), None);
        assert_eq!(save.poll(2, 0), None);
        assert_eq!(save.poll(2, 0), Some(2));
        for _ in 0..20 {
            assert_eq!(save.poll(2, 0), None);
        }
        assert_eq!(save.poll(2, 1), Some(2));
        assert_eq!(save.poll(2, 1), None);
    }

    #[test]
    fn autosave_cancels_edits_reverted_before_save() {
        let mut save = Autosave::new(1, 0);
        assert_eq!(save.poll(2, 0), None);
        for _ in 0..5 {
            assert_eq!(save.poll(1, 0), None);
        }
    }

    #[test]
    fn timezone_commit_reports_which_part_was_applied() {
        let mut calls = std::vec::Vec::new();
        let result = commit_timezone(
            "localtime",
            "localtime.tmp",
            "timezone",
            "timezone.tmp",
            |a, b| {
                calls.push((a.to_owned(), b.to_owned()));
                false
            },
        );
        assert_eq!(result, Err(TimezoneCommitError::Unchanged));
        assert_eq!(calls.len(), 1);
        let mut calls = 0;
        let result = commit_timezone(
            "localtime",
            "localtime.tmp",
            "timezone",
            "timezone.tmp",
            |_, _| {
                calls += 1;
                calls == 1
            },
        );
        assert_eq!(result, Err(TimezoneCommitError::AppliedWithoutName));
        assert_eq!(commit_timezone("a", "b", "c", "d", |_, _| true), Ok(()));
    }

    #[test]
    fn atomic_save_replaces_complete_contents_and_preserves_destination_on_failure() {
        let root = std::env::temp_dir().join(format!("scarlet-settings-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("config");
        let staging = root.join("config.tmp");
        std::fs::write(&dest, "old").unwrap();
        atomic_write(
            dest.to_str().unwrap(),
            staging.to_str().unwrap(),
            b"complete new data",
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"complete new data");
        assert!(!staging.exists());
        assert!(
            atomic_write(
                dest.to_str().unwrap(),
                root.join("missing/tmp").to_str().unwrap(),
                b"bad"
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"complete new data");
        let directory = root.join("directory");
        std::fs::create_dir(&directory).unwrap();
        assert!(
            atomic_write(
                directory.to_str().unwrap(),
                staging.to_str().unwrap(),
                b"bad"
            )
            .is_err()
        );
        assert!(directory.is_dir());
        assert!(!staging.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// Read an os-release assignment as data; never evaluate shell expressions.
pub fn release_value(content: &str, key: &str) -> Option<std::string::String> {
    for line in content.lines() {
        let Some((name, value)) = line.trim().split_once('=') else {
            continue;
        };
        if name != key {
            continue;
        }
        let value = value.trim();
        let value = if value.starts_with('"') {
            value.strip_prefix('"')?.strip_suffix('"')?
        } else if value.starts_with('\'') {
            value.strip_prefix('\'')?.strip_suffix('\'')?
        } else {
            value
        };
        if value.is_empty() {
            return None;
        }
        let mut decoded = std::string::String::new();
        let mut characters = value.chars();
        while let Some(character) = characters.next() {
            if character == '\\' {
                let escaped = characters.next()?;
                if !matches!(escaped, '\\' | '"' | '$' | '`') {
                    decoded.push('\\');
                }
                decoded.push(escaped);
            } else {
                decoded.push(character);
            }
        }
        return Some(decoded);
    }
    None
}

#[cfg(test)]
mod release_tests {
    use super::release_value;
    #[test]
    fn release_metadata_is_decoded_without_evaluating_shell_syntax() {
        let content = "NAME=Scarlet\nPRETTY_NAME=\"Scarlet \\\"Akane\\\"\"\nVERSION_ID='1.2'\nBUG_REPORT_URL=$(echo secret)\n";
        assert_eq!(release_value(content, "NAME").as_deref(), Some("Scarlet"));
        assert_eq!(
            release_value(content, "PRETTY_NAME").as_deref(),
            Some("Scarlet \"Akane\"")
        );
        assert_eq!(release_value(content, "VERSION_ID").as_deref(), Some("1.2"));
        assert_eq!(
            release_value(content, "BUG_REPORT_URL").as_deref(),
            Some("$(echo secret)")
        );
        assert_eq!(release_value("NAME=\"unterminated", "NAME"), None);
        assert_eq!(release_value("NAME=", "NAME"), None);
    }
}
