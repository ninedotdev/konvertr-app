//! Animated pixel-ASCII banner: "LOADER" in a 5×5 block font whose pixels
//! shift through shade glyphs as a wave sweeps across. Pure function of
//! `phase` (0..1, looping); the caller owns the tick.

use gpui::{SharedString, div, prelude::*, px};

use crate::theme::Theme;

const GLYPH_H: usize = 5;
const LETTER_W: usize = 5;

fn glyph(c: char) -> [&'static str; GLYPH_H] {
    match c {
        'L' => ["X....", "X....", "X....", "X....", "XXXXX"],
        'O' => [".XXX.", "X...X", "X...X", "X...X", ".XXX."],
        'A' => [".XXX.", "X...X", "XXXXX", "X...X", "X...X"],
        'D' => ["XXXX.", "X...X", "X...X", "X...X", "XXXX."],
        'E' => ["XXXXX", "X....", "XXXX.", "X....", "XXXXX"],
        'R' => ["XXXX.", "X...X", "XXXX.", "X..X.", "X...X"],
        _ => ["     ", "     ", "     ", "     ", "     "],
    }
}

/// Shade for a lit pixel by distance to the sweeping wave crest.
fn shade(col: usize, total: usize, phase: f32) -> char {
    let pos = col as f32 / total.max(1) as f32;
    // wrapping distance to the crest
    let mut d = (pos - phase).abs();
    if d > 0.5 {
        d = 1.0 - d;
    }
    match d {
        d if d < 0.06 => '█',
        d if d < 0.14 => '▓',
        d if d < 0.26 => '▒',
        _ => '░',
    }
}

fn rows(word: &str, phase: f32) -> Vec<SharedString> {
    let letters: Vec<[&'static str; GLYPH_H]> = word.chars().map(glyph).collect();
    let total = letters.len() * (LETTER_W + 1);
    (0..GLYPH_H)
        .map(|row| {
            let mut line = String::with_capacity(total);
            for (li, letter) in letters.iter().enumerate() {
                for (ci, cell) in letter[row].chars().enumerate() {
                    let col = li * (LETTER_W + 1) + ci;
                    line.push(if cell == 'X' {
                        shade(col, total, phase)
                    } else {
                        ' '
                    });
                }
                line.push(' ');
            }
            SharedString::from(line)
        })
        .collect()
}

pub fn loader_banner(theme: &Theme, phase: f32) -> impl IntoElement {
    let mut banner = div().flex().flex_col().items_center().gap(px(1.)).child(
        div()
            .pb(px(Theme::SPACE_XS))
            .text_size(px(10.))
            .text_color(theme.text_faint)
            .child("konvertr"),
    );
    for line in rows("LOADER", phase) {
        banner = banner.child(
            div()
                .text_size(px(11.))
                .line_height(px(10.))
                .text_color(theme.text)
                .child(line),
        );
    }
    banner
}
