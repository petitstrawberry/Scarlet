#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone)]
pub struct YoutubeSearchResult {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<String>,
    pub thumbnail_url: Option<String>,
}

impl YoutubeSearchResult {
    pub fn watch_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }
}

pub fn encode_search_results_tsv(results: &[YoutubeSearchResult]) -> String {
    let mut out = String::new();
    for result in results {
        push_escaped_field(&mut out, &result.video_id);
        out.push('\t');
        push_escaped_field(&mut out, &result.title);
        out.push('\t');
        push_escaped_field(&mut out, result.channel.as_deref().unwrap_or(""));
        out.push('\t');
        push_escaped_field(&mut out, result.duration.as_deref().unwrap_or(""));
        out.push('\t');
        push_escaped_field(&mut out, result.thumbnail_url.as_deref().unwrap_or(""));
        out.push('\n');
    }
    out
}

pub fn parse_search_results_tsv(text: &str) -> Vec<YoutubeSearchResult> {
    let mut results = Vec::new();
    for line in text.lines() {
        if let Some(result) = parse_search_result_tsv_line(line) {
            results.push(result);
        }
    }
    results
}

fn parse_search_result_tsv_line(line: &str) -> Option<YoutubeSearchResult> {
    let fields = split_escaped_tsv_line(line);
    if fields.len() < 5 || fields[0].is_empty() || fields[1].is_empty() {
        return None;
    }
    Some(YoutubeSearchResult {
        video_id: fields[0].clone(),
        title: fields[1].clone(),
        channel: non_empty(fields[2].clone()),
        duration: non_empty(fields[3].clone()),
        thumbnail_url: non_empty(fields[4].clone()),
    })
}

fn split_escaped_tsv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            match ch {
                't' => current.push('\t'),
                'n' => current.push('\n'),
                'r' => current.push('\r'),
                '\\' => current.push('\\'),
                ch => current.push(ch),
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\t' => {
                fields.push(current);
                current = String::new();
            }
            ch => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn push_escaped_field(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch => out.push(ch),
        }
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}
