//! Bounded, read-only presentation of the current logd journal.

use log_protocol::{LogRecord, QueryEnd};
use std::collections::VecDeque;

pub const MAX_RECORDS: usize = 500;
pub const MAX_DISPLAY_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub sequence: u64,
    pub boot_id: u64,
    pub monotonic_ns: u64,
    pub unit: String,
    pub pid: i32,
    pub priority: u8,
    pub priority_name: String,
    pub stream: String,
    pub message: String,
    pub truncated: bool,
}

impl Entry {
    pub fn from_record(record: LogRecord) -> Self {
        let mut message = String::new();
        let mut truncated = false;
        for character in String::from_utf8_lossy(&record.message).chars() {
            let character = if character.is_control() && character != '\n' && character != '\t' {
                '�'
            } else {
                character
            };
            if message.len() + character.len_utf8() > MAX_DISPLAY_BYTES {
                truncated = true;
                break;
            }
            message.push(character);
        }
        Self {
            sequence: record.sequence,
            boot_id: record.boot_id,
            monotonic_ns: record.monotonic_ns,
            unit: record
                .unit
                .chars()
                .map(|c| if c.is_control() { '�' } else { c })
                .collect(),
            pid: record.pid,
            priority: record.priority.as_u8(),
            priority_name: record.priority.as_str().into(),
            stream: record.stream.as_str().into(),
            message,
            truncated,
        }
    }

    pub fn matches(&self, filter: &str, max_priority: u8) -> bool {
        self.priority <= max_priority
            && (filter.is_empty()
                || self.message.to_lowercase().contains(filter)
                || self.unit.to_lowercase().contains(filter)
                || self.pid.to_string().contains(filter))
    }

    pub fn summary(&self) -> String {
        let message: String = self
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect();
        let unit: String = self.unit.chars().take(20).collect();
        format!(
            "{:>6}.{:03}  {}  {}[{}]  {}",
            self.monotonic_ns / 1_000_000_000,
            self.monotonic_ns / 1_000_000 % 1000,
            self.priority_name,
            unit,
            self.pid,
            message
        )
    }

    pub fn details(&self) -> String {
        let mut text = format!(
            "{}[{}]  {} / {}  #{}\nBoot time: {}.{:03} seconds\n\n",
            self.unit,
            self.pid,
            self.priority_name,
            self.stream,
            self.sequence,
            self.monotonic_ns / 1_000_000_000,
            self.monotonic_ns / 1_000_000 % 1000
        );
        // The detail pane scrolls in both directions. Preserve journal line
        // boundaries instead of inserting hard breaks in words or identifiers.
        text.push_str(&self.message.replace('\t', "    "));
        if self.truncated {
            text.push_str("\n[Display truncated at 4096 bytes]");
        }
        text
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    pub entries: VecDeque<Entry>,
    pub after_sequence: u64,
    pub boot_id: Option<u64>,
    pub error: Option<String>,
    pub loading: bool,
    pub restarted: bool,
}

impl Journal {
    /// A daemon restart invalidates both the cursor and the old journal. The
    /// next finite query then obtains the new session's newest records.
    pub fn accept(&mut self, entries: Vec<Entry>, end: QueryEnd) {
        self.error = None;
        self.loading = false;
        if self.boot_id.is_some_and(|boot| boot != end.boot_id) {
            self.entries.clear();
            self.after_sequence = 0;
            self.boot_id = Some(end.boot_id);
            self.restarted = true;
            return;
        }
        self.boot_id = Some(end.boot_id);
        self.restarted = false;
        for entry in entries {
            if entry.boot_id != end.boot_id || entry.sequence <= self.after_sequence {
                continue;
            }
            self.after_sequence = entry.sequence;
            self.entries.push_back(entry);
            while self.entries.len() > MAX_RECORDS {
                self.entries.pop_front();
            }
        }
        self.after_sequence = self.after_sequence.max(end.last_sequence);
    }

    pub fn clear_display(&mut self) {
        self.entries.clear();
    }

    pub fn visible(&self, filter: &str, priority_index: usize) -> Vec<Entry> {
        let priority = match priority_index {
            1 => 4,
            2 => 3,
            _ => 7,
        };
        let filter = filter.trim().to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|entry| entry.matches(&filter, priority))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log_protocol::{LogPriority, LogStream};

    fn entry(sequence: u64, boot_id: u64) -> Entry {
        Entry::from_record(LogRecord {
            sequence,
            boot_id,
            monotonic_ns: 1,
            realtime_ns: u64::MAX,
            unit: "example.service".into(),
            pid: 42,
            stream: LogStream::Stderr,
            priority: LogPriority::Warning,
            message: b"Connection failed".to_vec(),
        })
    }

    #[test]
    fn history_is_bounded_deduplicated_and_newest_first() {
        let mut journal = Journal::default();
        journal.accept(
            (1..=550).map(|n| entry(n, 1)).collect(),
            QueryEnd {
                last_sequence: 550,
                boot_id: 1,
            },
        );
        assert_eq!(journal.entries.len(), MAX_RECORDS);
        assert_eq!(journal.entries.front().unwrap().sequence, 51);
        journal.accept(
            vec![entry(550, 1)],
            QueryEnd {
                last_sequence: 550,
                boot_id: 1,
            },
        );
        assert_eq!(journal.entries.len(), MAX_RECORDS);
        assert_eq!(journal.visible("", 0)[0].sequence, 550);
    }

    #[test]
    fn clear_keeps_cursor_and_cannot_repopulate_old_records() {
        let mut journal = Journal::default();
        journal.accept(
            vec![entry(1, 1)],
            QueryEnd {
                last_sequence: 1,
                boot_id: 1,
            },
        );
        journal.clear_display();
        journal.accept(
            vec![entry(1, 1), entry(2, 1)],
            QueryEnd {
                last_sequence: 2,
                boot_id: 1,
            },
        );
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].sequence, 2);
    }

    #[test]
    fn daemon_restart_resets_cursor_even_when_query_is_empty() {
        let mut journal = Journal::default();
        journal.accept(
            vec![entry(900, 1)],
            QueryEnd {
                last_sequence: 900,
                boot_id: 1,
            },
        );
        journal.accept(
            vec![],
            QueryEnd {
                last_sequence: 5,
                boot_id: 2,
            },
        );
        assert_eq!(journal.after_sequence, 0);
        assert!(journal.entries.is_empty());
        assert!(journal.restarted);
        journal.accept(
            vec![entry(5, 2)],
            QueryEnd {
                last_sequence: 5,
                boot_id: 2,
            },
        );
        assert_eq!(journal.entries[0].sequence, 5);
        assert!(!journal.restarted);
    }

    #[test]
    fn filter_combines_case_insensitive_text_pid_and_priority() {
        let mut journal = Journal::default();
        journal.accept(
            vec![entry(1, 1)],
            QueryEnd {
                last_sequence: 1,
                boot_id: 1,
            },
        );
        assert_eq!(journal.visible("FAILED", 1).len(), 1);
        assert_eq!(journal.visible("example", 0).len(), 1);
        assert_eq!(journal.visible("42", 0).len(), 1);
        assert!(journal.visible("failed", 2).is_empty());
        assert!(journal.visible("missing", 0).is_empty());
    }

    #[test]
    fn messages_are_utf8_bounded_and_control_characters_are_visible() {
        let mut record = LogRecord {
            sequence: 1,
            boot_id: 1,
            monotonic_ns: 0,
            realtime_ns: 0,
            unit: "unit".into(),
            pid: 1,
            stream: LogStream::Stdout,
            priority: LogPriority::Info,
            message: "界".repeat(3000).into_bytes(),
        };
        let large = Entry::from_record(record.clone());
        assert!(large.truncated);
        assert!(large.message.len() <= MAX_DISPLAY_BYTES);
        assert!(large.details().contains("Display truncated"));
        record.message = vec![0xff, 0x1b, b'X', b'\n'];
        assert_eq!(Entry::from_record(record).message, "��X\n");
    }
}
