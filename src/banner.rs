//! Multi-line "big text" banners — a small built-in 5×7 block font plus
//! the existing [`crate::text`] animation styles, for animated banners
//! instead of just single colored lines.
//!
//! [`banner_lines`] renders plain (uncolored) block text. [`banner_frames`]
//! additionally animates it: each *source character* gets one color per
//! frame (computed the same way [`crate::text::builtin_frame`] colors a
//! single-line animation), and that color is applied across every glyph
//! cell belonging to that character, so e.g. `Rainbow` sweeps a gradient
//! across the letters of a banner just like it does across a plain line.
//!
//! Not every [`TextAnimation`] variant maps naturally onto multi-character
//! glyphs:
//! - `Marquee` scrolls a *character* window; against a banner it would
//!   have to scroll a window of whole glyphs, which isn't implemented
//!   here — it falls back to rendering the full, unscrolled banner in
//!   `opts.base_color`.
//! - `Custom` animations produce arbitrary text, not a color, so there is
//!   no defined way to recolor a glyph from one — it also falls back to
//!   `opts.base_color`.

use crate::error::Result;
use crate::text::{hsv_to_rgb, Rgb, TextAnimOptions, TextAnimation};

const GLYPH_HEIGHT: usize = 7;
const GLYPH_WIDTH: usize = 5;
/// Blank columns inserted between adjacent glyphs.
const GLYPH_SPACING: usize = 1;

/// Returns the 7-row, 5-wide bitmap for one character (`'#'` = lit pixel,
/// `'.'` = empty). Unsupported characters (anything outside `A-Z`, `0-9`,
/// space, and the handful of punctuation marks below) render as a blank
/// glyph rather than erroring, so arbitrary text degrades gracefully
/// instead of failing to render at all.
fn glyph(c: char) -> [&'static str; GLYPH_HEIGHT] {
    match c.to_ascii_uppercase() {
        'A' => [
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'B' => [
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ],
        'C' => [
            ".####", "#....", "#....", "#....", "#....", "#....", ".####",
        ],
        'D' => [
            "####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####.",
        ],
        'E' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
        'F' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ],
        'G' => [
            ".####", "#....", "#....", "#.###", "#...#", "#...#", ".####",
        ],
        'H' => [
            "#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'I' => [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####",
        ],
        'J' => [
            "..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##..",
        ],
        'K' => [
            "#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#",
        ],
        'L' => [
            "#....", "#....", "#....", "#....", "#....", "#....", "#####",
        ],
        'M' => [
            "#...#", "##.##", "#.#.#", "#...#", "#...#", "#...#", "#...#",
        ],
        'N' => [
            "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#",
        ],
        'O' => [
            ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        'P' => [
            "####.", "#...#", "#...#", "####.", "#....", "#....", "#....",
        ],
        'Q' => [
            ".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#",
        ],
        'R' => [
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ],
        'S' => [
            ".####", "#....", "#....", ".###.", "....#", "....#", "####.",
        ],
        'T' => [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
        'U' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        'V' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#..",
        ],
        'W' => [
            "#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#",
        ],
        'X' => [
            "#...#", ".#.#.", "..#..", "..#..", "..#..", ".#.#.", "#...#",
        ],
        'Y' => [
            "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
        'Z' => [
            "#####", "....#", "...#.", "..#..", ".#...", "#....", "#####",
        ],
        '0' => [
            ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
        ],
        '1' => [
            "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", "#####",
        ],
        '2' => [
            ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
        ],
        '3' => [
            "####.", "....#", "....#", ".###.", "....#", "....#", "####.",
        ],
        '4' => [
            "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
        ],
        '5' => [
            "#####", "#....", "#....", "####.", "....#", "....#", "####.",
        ],
        '6' => [
            ".###.", "#....", "#....", "####.", "#...#", "#...#", ".###.",
        ],
        '7' => [
            "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
        ],
        '8' => [
            ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
        ],
        '9' => [
            ".###.", "#...#", "#...#", ".####", "....#", "....#", ".###.",
        ],
        '!' => [
            "..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#..",
        ],
        '?' => [
            ".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#..",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".....", "..#..",
        ],
        ',' => [
            ".....", ".....", ".....", ".....", ".....", "..#..", ".#...",
        ],
        '-' => [
            ".....", ".....", ".....", "#####", ".....", ".....", ".....",
        ],
        ':' => [
            ".....", "..#..", ".....", ".....", "..#..", ".....", ".....",
        ],
        '\'' => [
            "..#..", "..#..", ".....", ".....", ".....", ".....", ".....",
        ],
        _ => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
    }
}

/// Renders `text` as plain (uncolored) block letters, one [`String`] per
/// output row (always [`GLYPH_HEIGHT`] rows, i.e. 7 lines).
pub fn banner_lines(text: &str) -> Vec<String> {
    let glyphs: Vec<[&'static str; GLYPH_HEIGHT]> = text.chars().map(glyph).collect();
    // Pre-size each row's buffer once instead of letting it grow via
    // repeated reallocation, and push spacing characters directly rather
    // than allocating a throwaway `String` per glyph via `.repeat()` —
    // this loop runs once per row per glyph per call, so both matter more
    // as `text` gets longer.
    let row_capacity = glyphs.len() * (GLYPH_WIDTH + GLYPH_SPACING);
    (0..GLYPH_HEIGHT)
        .map(|row| {
            let mut line = String::with_capacity(row_capacity);
            for (i, g) in glyphs.iter().enumerate() {
                if i > 0 {
                    line.extend(std::iter::repeat(' ').take(GLYPH_SPACING));
                }
                line.extend(g[row].chars().map(|c| if c == '.' { ' ' } else { c }));
            }
            line
        })
        .collect()
}

/// Computes the color animation `animation` assigns to the character at
/// `char_index` (of `total_chars`) on frame `frame_index`, or `None` when
/// that character shouldn't be drawn yet (only possible for `Typewriter`).
/// This mirrors [`crate::text::builtin_frame`]'s per-character logic but
/// returns a color instead of a formatted/escaped string, since a banner
/// glyph is many terminal cells wide and needs the same color applied to
/// all of them.
fn banner_char_color(
    animation: &TextAnimation,
    char_index: usize,
    total_chars: usize,
    frame_index: u64,
    opts: &TextAnimOptions,
) -> Option<Rgb> {
    match animation {
        TextAnimation::Typewriter => {
            if char_index <= frame_index as usize {
                Some(opts.base_color)
            } else {
                None
            }
        }
        TextAnimation::Rainbow => {
            let hue = ((frame_index as f32) * 12.0 + (char_index as f32) * 18.0) % 360.0;
            Some(hsv_to_rgb(hue, 1.0, 1.0))
        }
        TextAnimation::Wave => {
            // Reuses the single-line Wave sweep math exactly: the sweep
            // position cycles through the *whole* banner's length (plus
            // the same +8 padding `builtin_frame`'s Wave arm uses), not a
            // fixed window, so every character eventually gets swept over
            // no matter how long the banner text is. A hardcoded window
            // shorter than `total_chars` would leave characters past that
            // point stuck at the dim floor forever, since `char_index -
            // pos` would only ever grow for them.
            let span = (total_chars as i64 + 8).max(1);
            let pos = (frame_index as i64) % span;
            let dist = (char_index as i64 - pos).unsigned_abs() as f32;
            let brightness = (1.0 - (dist / 4.0).min(1.0)).max(0.15);
            Some(opts.base_color.scale(brightness))
        }
        TextAnimation::Blink {
            on_color,
            off_color,
        } => Some(if frame_index % 2 == 0 {
            *on_color
        } else {
            *off_color
        }),
        TextAnimation::Pulse => {
            let t = frame_index as f32 * 0.35;
            let brightness = (0.5 + 0.5 * t.sin()).clamp(0.15, 1.0);
            Some(opts.base_color.scale(brightness))
        }
        // Scrolling and program/closure-driven animations have no defined
        // per-character color; see the module doc for why.
        TextAnimation::Marquee { .. } | TextAnimation::Custom(_) => Some(opts.base_color),
    }
}

/// Generates every animated banner frame of `text` up front. Each frame is
/// a `Vec<String>` of [`GLYPH_HEIGHT`] ANSI-colored rows, analogous to
/// [`crate::text::text_frames`] but multi-line.
///
/// `Typewriter` stops once every character has been revealed, the same
/// early-exit behavior as the single-line version; all other animations
/// run for exactly `opts.frames` frames.
pub fn banner_frames(
    text: &str,
    animation: &TextAnimation,
    opts: &TextAnimOptions,
) -> Result<Vec<Vec<String>>> {
    let chars: Vec<char> = text.chars().collect();
    let glyphs: Vec<[&'static str; GLYPH_HEIGHT]> = chars.iter().map(|&c| glyph(c)).collect();
    let total_chars = chars.len();

    let mut frames = Vec::with_capacity(opts.frames as usize);
    for frame_index in 0..opts.frames {
        let colors: Vec<Option<Rgb>> = (0..total_chars)
            .map(|i| banner_char_color(animation, i, total_chars, frame_index, opts))
            .collect();

        if matches!(animation, TextAnimation::Typewriter) && frame_index as usize >= total_chars {
            // Fully revealed; stop like the single-line Typewriter does.
            // Also covers empty `text` (total_chars == 0): frame_index 0
            // already satisfies `0 >= 0`, so this breaks before pushing
            // any frame, matching `text_frames("", Typewriter, ..)`'s
            // empty result instead of emitting `opts.frames` blank frames.
            break;
        }

        // As in `banner_lines`: avoid a throwaway `String` allocation per
        // spacing/blank gap, and give each row's buffer a reasonable
        // starting capacity (glyph cells + a rough allowance for the ANSI
        // color escapes) so it doesn't have to repeatedly reallocate while
        // being built up character by character. This function already
        // runs the outer loop once per frame, so per-row allocation
        // overhead is the part most worth trimming here.
        let row_capacity = total_chars * (GLYPH_WIDTH + GLYPH_SPACING + 12);
        let rows: Vec<String> = (0..GLYPH_HEIGHT)
            .map(|row| {
                let mut line = String::with_capacity(row_capacity);
                for (i, g) in glyphs.iter().enumerate() {
                    if i > 0 {
                        line.extend(std::iter::repeat(' ').take(GLYPH_SPACING));
                    }
                    match colors[i] {
                        Some(color) => {
                            line.push_str(&color.ansi_fg());
                            line.extend(g[row].chars().map(|c| if c == '.' { ' ' } else { c }));
                            line.push_str("\x1b[0m");
                        }
                        None => line.extend(std::iter::repeat(' ').take(GLYPH_WIDTH)),
                    }
                }
                line
            })
            .collect();
        frames.push(rows);
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextAnimOptions;

    #[test]
    fn banner_lines_produces_seven_rows_and_widens_with_more_characters() {
        let one = banner_lines("A");
        let two = banner_lines("AB");
        assert_eq!(one.len(), GLYPH_HEIGHT);
        assert_eq!(two.len(), GLYPH_HEIGHT);
        for (l1, l2) in one.iter().zip(two.iter()) {
            assert!(l2.len() > l1.len());
        }
    }

    #[test]
    fn banner_lines_has_no_escape_codes() {
        let lines = banner_lines("HI");
        for line in &lines {
            assert!(!line.contains('\x1b'));
        }
    }

    #[test]
    fn unsupported_characters_render_as_a_blank_glyph_without_panicking() {
        let lines = banner_lines("A~B");
        assert_eq!(lines.len(), GLYPH_HEIGHT);
        assert!(lines.iter().any(|l| l.contains('#')));
    }

    #[test]
    fn typewriter_banner_stops_once_every_character_is_revealed() {
        let opts = TextAnimOptions {
            frames: 100,
            ..Default::default()
        };
        let frames = banner_frames("HI", &TextAnimation::Typewriter, &opts).unwrap();
        assert_eq!(
            frames.len(),
            2,
            "one frame per character revealed, then stop"
        );
    }

    #[test]
    fn typewriter_banner_hides_unrevealed_glyphs_as_blank_columns() {
        let opts = TextAnimOptions {
            frames: 100,
            ..Default::default()
        };
        let frames = banner_frames("HI", &TextAnimation::Typewriter, &opts).unwrap();
        let first_ink_rows: usize = frames[0].iter().filter(|l| l.contains('#')).count();
        let second_ink_rows: usize = frames[1].iter().filter(|l| l.contains('#')).count();
        assert!(second_ink_rows >= first_ink_rows);
    }

    #[test]
    fn rainbow_banner_colors_each_glyph_and_resets_per_row() {
        let opts = TextAnimOptions {
            frames: 3,
            ..Default::default()
        };
        let frames = banner_frames("AB", &TextAnimation::Rainbow, &opts).unwrap();
        assert_eq!(frames.len(), 3);
        for frame in &frames {
            for row in frame {
                if row.contains('#') {
                    assert!(row.contains("\x1b[38;2;"));
                    assert!(row.ends_with("\x1b[0m"));
                }
            }
        }
    }

    #[test]
    fn wave_banner_sweep_eventually_lights_up_every_character_in_a_long_banner() {
        // Regression test: `banner_char_color`'s Wave arm used to hardcode
        // the sweep's cycle length at 12, so characters past roughly
        // column 15 could never reach full brightness no matter how many
        // frames were generated. The sweep must span the whole banner
        // (mirroring `text.rs`'s single-line Wave, whose span is
        // `char_count + 8`), so every character should hit peak
        // brightness (a distance of 0 from the sweep's center) at some
        // frame, even in a banner longer than the old hardcoded window.
        let text = "HELLO WORLD ABCDEFGH"; // 21 characters
        let total_chars = text.chars().count();
        let opts = TextAnimOptions {
            frames: (total_chars as u64) + 8, // one full sweep cycle
            ..Default::default()
        };
        for char_index in 0..total_chars {
            let mut max_brightness = 0.0f32;
            for frame_index in 0..opts.frames {
                let color = banner_char_color(
                    &TextAnimation::Wave,
                    char_index,
                    total_chars,
                    frame_index,
                    &opts,
                )
                .expect("Wave always returns a color");
                // Reconstruct the brightness fraction from the scaled
                // white base color (255 * brightness, rounded).
                let brightness = color.0 as f32 / 255.0;
                if brightness > max_brightness {
                    max_brightness = brightness;
                }
            }
            assert!(
                max_brightness > 0.99,
                "char_index {char_index} in a {total_chars}-char banner never reached full brightness (max was {max_brightness})"
            );
        }
    }

    #[test]
    fn typewriter_banner_of_empty_text_produces_no_frames() {
        // Regression test: banner_frames used to require `total_chars > 0`
        // before allowing its early-exit check to fire, so an empty
        // string generated `opts.frames` blank frames instead of stopping
        // immediately like the single-line `text_frames("", Typewriter, ..)`
        // does.
        let opts = TextAnimOptions {
            frames: 40,
            ..Default::default()
        };
        let frames = banner_frames("", &TextAnimation::Typewriter, &opts).unwrap();
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn banner_frames_all_rows_have_matching_length_within_a_frame() {
        let opts = TextAnimOptions {
            frames: 2,
            ..Default::default()
        };
        let frames = banner_frames("OK", &TextAnimation::Pulse, &opts).unwrap();
        for frame in &frames {
            assert_eq!(frame.len(), GLYPH_HEIGHT);
        }
    }
}
