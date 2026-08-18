//! Session-wide conversion history, shown in the right sidebar. Tools push an
//! entry after every successful conversion; the shell observes the global.

use std::path::PathBuf;

use gpui::{App, Global};

pub struct HistoryEntry {
    /// Short tool tag shown as a badge ("img", "vid", "data", …).
    pub tool: &'static str,
    pub name: String,
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

#[derive(Default)]
pub struct HistoryStore {
    pub entries: Vec<HistoryEntry>,
}

impl Global for HistoryStore {}

pub fn init(cx: &mut App) {
    cx.set_global(HistoryStore::default());
}

pub fn push(cx: &mut App, entry: HistoryEntry) {
    cx.global_mut::<HistoryStore>().entries.push(entry);
}

pub struct Stats {
    pub conversions: usize,
    pub bytes_saved: u64,
    /// Seconds of upload-wait + ad-watching a web converter would have cost.
    pub seconds_dodged: u64,
    pub popups_dodged: u64,
}

impl HistoryStore {
    pub fn stats(&self) -> Stats {
        let conversions = self.entries.len();
        let bytes_saved = self
            .entries
            .iter()
            .map(|e| e.in_size.saturating_sub(e.out_size))
            .sum();
        Stats {
            conversions,
            bytes_saved,
            // 47s per file: upload + "preparing your download" + the ad you
            // can skip after 5 seconds. Scientifically rigorous.
            seconds_dodged: conversions as u64 * 47,
            popups_dodged: conversions as u64 * 4,
        }
    }
}

/// Rotating cheeky footer line for the history panel.
pub fn quip(stats: &Stats) -> String {
    if stats.conversions == 0 {
        return "convert something and watch the counters judge \
                every ad-riddled converter site you've ever used."
            .to_string();
    }
    match stats.conversions % 4 {
        0 => format!(
            "the average converter site shows ~4 popups per file. \
             that's {} popups you never saw. you're welcome.",
            stats.popups_dodged
        ),
        1 => "0 bytes uploaded. somewhere, a tracking pixel is crying.".to_string(),
        2 => format!(
            "~{} of \"preparing your download…\" skipped so far.",
            human_secs(stats.seconds_dodged)
        ),
        _ => "no accounts, no queues, no \"premium\" tier. just files.".to_string(),
    }
}

pub fn human_secs(s: u64) -> String {
    if s >= 3600 {
        format!("{:.1}h", s as f64 / 3600.0)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn human_bytes(b: u64) -> String {
    let f = b as f64;
    if f >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", f / (1024.0 * 1024.0 * 1024.0))
    } else if f >= 1024.0 * 1024.0 {
        format!("{:.1} MB", f / (1024.0 * 1024.0))
    } else if f >= 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else {
        format!("{b} B")
    }
}
