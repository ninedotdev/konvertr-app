//! Checksums: drop any files, MD5/SHA-1/SHA-256 are computed automatically on
//! the background executor; paste an expected hash to verify against any of
//! the three. Nothing is written to disk (no history entry).

use std::path::PathBuf;

use gpui::{
    ClipboardItem, Context, Entity, ExternalPaths, PathPromptOptions, Subscription, Window, div,
    prelude::*, px,
};
use konvrt_core::hash::FileHashes;

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

#[derive(Clone)]
enum FileStatus {
    Hashing,
    Done(FileHashes),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: FileStatus,
}

pub struct HashTool {
    files: Vec<FileEntry>,
    expected: Entity<TextInput>,
    _subscription: Subscription,
}

impl HashTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let expected = cx.new(|cx| TextInput::new(cx, "paste an expected hash to verify"));
        let subscription = cx.subscribe(&expected, |_: &mut HashTool, _, event, cx| {
            let TextInputEvent::Edited = event;
            cx.notify();
        });
        Self {
            files: Vec::new(),
            expected,
            _subscription: subscription,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !path.is_file() {
                continue;
            }
            if self.files.iter().any(|f| f.path == path) {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.files.push(FileEntry {
                path: path.clone(),
                size,
                status: FileStatus::Hashing,
            });
            // Hash immediately — no convert button for checksums. Rows can be
            // removed while hashing, so the result is matched back by path.
            let key = path.clone();
            cx.spawn(async move |this, cx| {
                let hashed = cx
                    .background_spawn(async move { konvrt_core::hash::hash_file(&path) })
                    .await;
                this.update(cx, |tool, cx| {
                    if let Some(entry) = tool.files.iter_mut().find(|f| f.path == key) {
                        entry.status = match hashed {
                            Ok(hashes) => FileStatus::Done(hashes),
                            Err(e) => FileStatus::Error(format!("{e:#}")),
                        };
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |tool, cx| tool.add_paths(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::dropzone::drop_zone(
            theme,
            self.files.is_empty(),
            "drag & drop any files here",
            "md5 · sha-1 · sha-256, computed locally",
        )
        .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
            this.add_paths(paths.paths().to_vec(), cx);
        }))
    }

    fn render_hash_row(
        &self,
        ix: usize,
        algo: &'static str,
        value: &str,
        verified: Option<bool>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx);
        let copy_value = value.to_string();
        div()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_MD))
            .child(
                div()
                    .w(px(52.))
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(algo.to_uppercase()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(match verified {
                        Some(true) => theme.success,
                        _ => theme.text_muted,
                    })
                    .child(value.to_string()),
            )
            .child(
                div()
                    .id((algo, ix))
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
                    }))
                    .child("copy"),
            )
    }

    fn render_file_row(
        &self,
        ix: usize,
        expected: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx);
        let file = &self.files[ix];
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(Theme::SPACE_MD))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme.text)
                    .child(name),
            );

        let badge: Option<gpui::AnyElement> = match &file.status {
            FileStatus::Hashing => Some(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("hashing…")
                    .into_any_element(),
            ),
            FileStatus::Done(hashes) if !expected.is_empty() => {
                Some(match hashes.matches(expected) {
                    Some(algo) => div()
                        .text_size(px(11.))
                        .text_color(theme.success)
                        .child(format!("verified ✓ ({algo})"))
                        .into_any_element(),
                    None => div()
                        .text_size(px(11.))
                        .text_color(theme.danger)
                        .child("mismatch")
                        .into_any_element(),
                })
            }
            FileStatus::Done(_) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .child(format_size(file.size))
                    .into_any_element(),
            ),
            FileStatus::Error(_) => None,
        };
        if let Some(badge) = badge {
            header = header.child(badge);
        }
        header = header.child(
            div()
                .id(("remove", ix))
                .px(px(Theme::SPACE_XS))
                .text_color(theme.text_faint)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.danger))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.files.remove(ix);
                    cx.notify();
                }))
                .child("×"),
        );

        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .px(px(Theme::SPACE_MD))
            .py(px(Theme::SPACE_SM))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface)
            .child(header);

        match &file.status {
            FileStatus::Done(hashes) => {
                let matched = if expected.is_empty() {
                    None
                } else {
                    hashes.matches(expected)
                };
                for (algo, value) in [
                    ("md5", &hashes.md5),
                    ("sha1", &hashes.sha1),
                    ("sha256", &hashes.sha256),
                ] {
                    let verified = matched.map(|m| m == algo);
                    row = row.child(self.render_hash_row(ix, algo, value, verified, cx));
                }
            }
            FileStatus::Error(e) => {
                row = row.child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.danger)
                        .truncate()
                        .child(e.clone()),
                );
            }
            FileStatus::Hashing => {}
        }

        row
    }
}

fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

impl Render for HashTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colors out so the theme borrow doesn't overlap the
        // &mut cx uses below (Hsla is Copy).
        let theme = Theme::of(cx);
        let (text, border, text_muted, surface_hover) = (
            theme.text,
            theme.border,
            theme.text_muted,
            theme.surface_hover,
        );
        let expected = self.expected.read(cx).text().trim().to_ascii_lowercase();

        let mut pane = div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_MD))
            .child(div().text_size(px(13.)).text_color(text).child("checksums"))
            .child(self.render_drop_zone(cx));

        if !self.files.is_empty() {
            pane = pane.child(self.expected.clone());
            let mut rows = div()
                .id("file-list")
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(Theme::SPACE_XS))
                .overflow_y_scroll();
            for ix in 0..self.files.len() {
                rows = rows.child(self.render_file_row(ix, &expected, cx));
            }
            pane = pane.child(rows).child(
                div()
                    .id("clear")
                    .px(px(Theme::SPACE_MD))
                    .py(px(6.))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(border)
                    .text_color(text_muted)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(surface_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files.clear();
                        cx.notify();
                    }))
                    .child("clear all"),
            );
        }

        pane
    }
}
