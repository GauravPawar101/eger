//! Per-cell (pixel-level) ANSI coloring and animation for **any** already
//! rendered block of ASCII text — a static image conversion, a decoded
//! GIF/video frame, or plain block-letter banner text — as opposed to
//! [`crate::text`]/[`crate::banner`], which only color a single line or a
//! banner's own generated glyphs.
//!
//! Where [`crate::text::TextAnimation`] assigns one color per *character*
//! of a line, and [`crate::banner`] assigns one color per *source
//! character* of a banner (applied across that character's whole glyph),
//! [`PixelAnimation`] assigns one color per *grid cell* (column `x`, row
//! `y`) of arbitrary ASCII art. That makes it work identically whether the
//! art came from [`crate::banner::banner_lines`], an `iascii` image
//! conversion ([`colorize_grid`]/[`colorize_grid_frames`]), a decoded GIF
//! frame ([`colorize_sequence`], e.g. from [`crate::image::gif_to_lines`]),
//! or already-rendered video frames ([`colorize_joined_sequence`], e.g.
//! from [`crate::video::render_video_frames`] with
//! [`crate::video::ColorMode::Plain`]) — all of these are ultimately just
//! rows of characters, and this module only ever looks at that.
//!
//! Built-in styles ([`PixelAnimation::Rainbow`], [`Wave`], [`Pulse`],
//! [`Blink`], [`Plasma`], [`Ripple`]) all boil down to a
//! `(x, y, frame) -> Rgb` function evaluated per cell.
//! [`PixelAnimation::Custom`] lets you supply that function yourself —
//! including a purely mathematical one (your own sine mixes, radial
//! gradients, noise, cellular automata, whatever `x`/`y`/`frame` can
//! drive) — for effects the built-ins don't cover. [`Plasma`] and
//! [`Ripple`] are themselves just built-in examples of that: nothing they
//! do can't also be written as a [`PixelAnimation::custom`] closure.
//!
//! Every built-in only ever needs `+`, `*`, `sin`/`sqrt`/`rem_euclid`, so a
//! [`PixelAnimation::Custom`] closure can freely mix in whatever other math
//! it wants (noise functions, distance fields, lookup tables, ...) without
//! needing any support from this module beyond the `(x, y, frame, ch)`
//! it's called with.
//!
//! Coloring skips whitespace cells (a space stays a plain, unescaped
//! space) rather than wrapping every blank cell in its own ANSI
//! reset/color pair — that keeps output size down and avoids painting a
//! colored "background" over what is, in ASCII art, usually meant to be
//! empty space.
//!
//! # Performance
//!
//! Adjacent (including across a row boundary) same-color cells are merged
//! into a single escape/reset pair rather than one pair per cell, and each
//! distinct color's escape string is computed once per frame and reused —
//! see [`colorize_lines`]'s implementation note. Generating many frames
//! ([`colorize_frames`], [`colorize_grid_frames`], [`colorize_sequence`],
//! [`colorize_joined_sequence`]) is parallelized across frames with Rayon,
//! unconditionally, the same "across independent units of work" kind of
//! parallelism [`crate::image::render_batch_parallel`] and
//! [`crate::video::stream_video_frames`] already use elsewhere in this
//! crate — no feature flag needed.

use crate::text::{hsv_to_rgb, Rgb};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

/// A per-cell color function: given a cell's column (`x`), row (`y`), the
/// current animation frame index, and the character occupying that cell,
/// returns the color to draw it in.
///
/// `ch` is passed through (rather than requiring the closure to be
/// generated from a fixed grid up front) so a function can, for example,
/// color a particular character differently from the rest — most
/// position/time-only effects simply ignore it.
pub type ColorFn = Arc<dyn Fn(usize, usize, u64, char) -> Rgb + Send + Sync>;

/// A per-cell color animation applied over an existing block of plain
/// (uncolored) ASCII text. See the module docs for how this differs from
/// [`crate::text::TextAnimation`]/[`crate::banner`]'s per-character
/// coloring.
#[derive(Clone)]
pub enum PixelAnimation {
    /// A hue sweep across both axes: `hue` depends on `frame` plus a
    /// per-cell offset along `x` and `y`, so the rainbow band visibly
    /// moves across the art over time instead of every cell cycling in
    /// lockstep. `speed` is degrees of hue shift per frame.
    Rainbow { speed: f32 },
    /// A brightness band sweeps left-to-right across the art against
    /// `base_color` — the same math as
    /// [`crate::text::TextAnimation::Wave`], extended from one line to a
    /// whole grid (every row sees the same horizontal sweep).
    Wave { base_color: Rgb },
    /// The whole grid breathes between dim and full brightness together
    /// (every cell shares one global brightness curve).
    Pulse { base_color: Rgb },
    /// Alternates every other frame between two flat colors.
    Blink { on_color: Rgb, off_color: Rgb },
    /// Classic demoscene "plasma": a sum of sine waves over `x`, `y`, and
    /// time, mapped onto hue. Purely mathematical, needs no base color.
    /// `scale` sets the spatial frequency (smaller = larger color blobs);
    /// `speed` sets how fast it animates.
    Plasma { scale: f32, speed: f32 },
    /// Concentric rings of brightness expanding outward from the grid's
    /// center against `base_color`; another purely math-driven built-in,
    /// provided as a second worked example alongside [`Plasma`]. `speed`
    /// controls how fast the rings travel outward.
    Ripple { base_color: Rgb, speed: f32 },
    /// A user-supplied [`ColorFn`] — see [`PixelAnimation::custom`].
    Custom(ColorFn),
}

impl PixelAnimation {
    /// Wraps a closure as [`PixelAnimation::Custom`]. The closure receives
    /// `(x, y, frame_index, ch)` for every non-space cell and returns that
    /// cell's color; use ordinary floating-point math over `x`/`y`/`frame`
    /// for anything from a simple linear gradient to arbitrary generative
    /// patterns.
    ///
    /// ```
    /// use eger::colorize::PixelAnimation;
    /// use eger::Rgb;
    ///
    /// // A diagonal gradient from red (top-left) to blue (bottom-right),
    /// // static in time (ignores `frame`).
    /// let gradient = PixelAnimation::custom(|x, y, _frame, _ch| {
    ///     let t = ((x + y) as f32 / 40.0).clamp(0.0, 1.0);
    ///     Rgb::new((255.0 * (1.0 - t)) as u8, 0, (255.0 * t) as u8)
    /// });
    /// ```
    pub fn custom(f: impl Fn(usize, usize, u64, char) -> Rgb + Send + Sync + 'static) -> Self {
        Self::Custom(Arc::new(f))
    }
}

/// Evaluates `animation` at one cell. `width`/`height` are the bounding
/// box of the art being colored, needed by animations (like [`Ripple`])
/// whose math depends on the grid's overall size, not just one cell's
/// coordinates.
///
/// [`PixelAnimation::Ripple`]: PixelAnimation::Ripple
fn pixel_color(
    animation: &PixelAnimation,
    x: usize,
    y: usize,
    frame: u64,
    ch: char,
    width: usize,
    height: usize,
) -> Rgb {
    match animation {
        PixelAnimation::Rainbow { speed } => {
            let hue = (frame as f32 * speed + x as f32 * 6.0 + y as f32 * 10.0) % 360.0;
            hsv_to_rgb(hue, 1.0, 1.0)
        }
        PixelAnimation::Wave { base_color } => {
            let span = (width as i64 + 8).max(1);
            let pos = (frame as i64) % span;
            let dist = (x as i64 - pos).unsigned_abs() as f32;
            let brightness = (1.0 - (dist / 4.0).min(1.0)).max(0.15);
            base_color.scale(brightness)
        }
        PixelAnimation::Pulse { base_color } => {
            let t = frame as f32 * 0.35;
            let brightness = (0.5 + 0.5 * t.sin()).clamp(0.15, 1.0);
            base_color.scale(brightness)
        }
        PixelAnimation::Blink {
            on_color,
            off_color,
        } => {
            if frame % 2 == 0 {
                *on_color
            } else {
                *off_color
            }
        }
        PixelAnimation::Plasma { scale, speed } => {
            let t = frame as f32 * speed;
            let fx = x as f32 * scale;
            let fy = y as f32 * scale;
            let v = (fx + t).sin()
                + (fy + t).sin()
                + (fx + fy + t).sin()
                + ((fx * fx + fy * fy).sqrt() - t).sin();
            // `v` ranges over roughly [-4, 4]; spread it across the full
            // hue wheel and let `rem_euclid` fold negative values back
            // into `0..360` instead of clamping/discarding them.
            let hue = (v * 45.0).rem_euclid(360.0);
            hsv_to_rgb(hue, 1.0, 1.0)
        }
        PixelAnimation::Ripple { base_color, speed } => {
            let cx = width as f32 / 2.0;
            let cy = height as f32 / 2.0;
            let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let phase = dist * 0.5 - frame as f32 * speed;
            let brightness = (0.5 + 0.5 * phase.sin()).clamp(0.15, 1.0);
            base_color.scale(brightness)
        }
        PixelAnimation::Custom(f) => f(x, y, frame, ch),
    }
}

/// Colors one frame of `lines` (plain, uncolored rows of characters —
/// e.g. from [`crate::banner::banner_lines`], [`crate::render::plain_string`]
/// split on `\n`, or [`crate::render::grid_to_lines`] with `depth: None`)
/// under `animation` at animation frame `frame`, returning a single
/// `\n`-joined ANSI string (the same convention [`crate::render::grid_to_string`]
/// uses).
///
/// Cells holding a space are left as a plain, unescaped space rather than
/// being wrapped in a color/reset pair — see the module docs. Generic over
/// `S: AsRef<str>` so callers already holding borrowed `&str` rows (e.g.
/// [`colorize_joined_sequence`] splitting one big `String` on `\n`) don't
/// have to clone them into owned `String`s just to call this.
pub fn colorize_lines<S: AsRef<str>>(
    lines: &[S],
    animation: &PixelAnimation,
    frame: u64,
) -> String {
    let (width, height) = line_dims(lines);
    colorize_lines_with_dims(lines, width, height, animation, frame)
}

/// The actual per-frame coloring work, taking `width`/`height` as inputs
/// instead of recomputing them — [`colorize_frames`]/[`colorize_grid_frames`]
/// compute them once for `lines` and reuse them across every generated
/// frame, since `lines` (and therefore its dimensions) doesn't change
/// frame to frame in either of those.
///
/// Adjacent same-color cells — including across a row boundary, since
/// ANSI color state isn't reset by a bare `\n` in any real terminal — are
/// merged into a single `escape .. reset` pair instead of one pair per
/// cell, and every distinct color's escape string is computed once per
/// call and reused (`colors` cache) rather than reformatted on every
/// occurrence. For a mostly-uniform-color frame (`Pulse`, `Blink`, ...)
/// this collapses what would otherwise be one escape pair per non-space
/// cell down to a small handful for the entire frame.
fn colorize_lines_with_dims<S: AsRef<str>>(
    lines: &[S],
    width: usize,
    height: usize,
    animation: &PixelAnimation,
    frame: u64,
) -> String {
    // Rough capacity: glyph cells plus a modest allowance for the
    // relatively small number of `\x1b[38;2;r;g;bm` / `\x1b[0m` escapes
    // this now actually emits, now that runs are merged rather than
    // repeated per cell.
    let mut out = String::with_capacity(height * (width + 1) + 64);
    let mut colors: HashMap<Rgb, String> = HashMap::new();
    let mut run_color: Option<Rgb> = None;

    for (y, line) in lines.iter().enumerate() {
        if y > 0 {
            out.push('\n');
        }
        for (x, ch) in line.as_ref().chars().enumerate() {
            if ch == ' ' {
                if run_color.take().is_some() {
                    out.push_str("\x1b[0m");
                }
                out.push(' ');
                continue;
            }
            let color = pixel_color(animation, x, y, frame, ch, width, height);
            if run_color != Some(color) {
                if run_color.is_some() {
                    out.push_str("\x1b[0m");
                }
                let escape = colors.entry(color).or_insert_with(|| color.ansi_fg());
                out.push_str(escape);
                run_color = Some(color);
            }
            out.push(ch);
        }
    }
    if run_color.is_some() {
        out.push_str("\x1b[0m");
    }
    out
}

fn line_dims<S: AsRef<str>>(lines: &[S]) -> (usize, usize) {
    let width = lines
        .iter()
        .map(|l| l.as_ref().chars().count())
        .max()
        .unwrap_or(0);
    (width, lines.len())
}

/// Generates `frames` animated frames of the *same* static `lines`,
/// analogous to [`crate::banner::banner_frames`] but for arbitrary ASCII
/// art instead of banner glyphs — use this to animate the color of a
/// single static image conversion, a banner, or any other one-shot piece
/// of ASCII art over time.
///
/// For art whose *content* is already changing frame to frame (a decoded
/// GIF or video), use [`colorize_sequence`]/[`colorize_joined_sequence`]
/// instead, which recolor each already-distinct frame using its own
/// position in the sequence as the time input.
///
/// Frames are generated in parallel with Rayon (unconditional, like
/// [`crate::image::render_batch_parallel`]/[`crate::video::stream_video_frames`]'s
/// own across-frame parallelism — no feature flag needed, since every
/// frame here is an independent, read-only computation over the same
/// `lines`).
pub fn colorize_frames<S: AsRef<str> + Sync>(
    lines: &[S],
    animation: &PixelAnimation,
    frames: u64,
) -> Vec<String> {
    let (width, height) = line_dims(lines);
    (0..frames)
        .into_par_iter()
        .map(|i| colorize_lines_with_dims(lines, width, height, animation, i))
        .collect()
}

/// Extracts an `iascii::grid::Grid`'s characters as plain (uncolored)
/// rows, discarding any per-cell color `iascii` itself computed from pixel
/// luminance/color — the starting point for driving a grid's color
/// entirely from a [`PixelAnimation`] instead.
fn grid_plain_lines(grid: &iascii::grid::Grid) -> Vec<String> {
    // Mirrors `crate::render::plain_string`'s own row/cell iteration
    // exactly (rather than assuming `Grid::rows()`'s row type exposes an
    // `.iter()` of its own), just collecting each row into a `String`
    // instead of one big newline-joined blob.
    grid.rows()
        .map(|row| {
            let mut line = String::new();
            for cell in row {
                line.push(cell.ch);
            }
            line
        })
        .collect()
}

/// [`colorize_lines`] for an `iascii` [`Grid`](iascii::grid::Grid) directly
/// — e.g. the output of [`crate::image::convert`] for a single image — so
/// a static image's ASCII art can be colored/animated by a
/// [`PixelAnimation`] instead of (or as well as) `iascii`'s own
/// pixel-luminance coloring.
pub fn colorize_grid(grid: &iascii::grid::Grid, animation: &PixelAnimation, frame: u64) -> String {
    colorize_lines(&grid_plain_lines(grid), animation, frame)
}

/// [`colorize_frames`] for an `iascii` [`Grid`](iascii::grid::Grid) — the
/// natural way to turn one converted image into an animated banner-style
/// piece: convert it once, then animate its color over as many frames as
/// you like without re-converting.
pub fn colorize_grid_frames(
    grid: &iascii::grid::Grid,
    animation: &PixelAnimation,
    frames: u64,
) -> Vec<String> {
    colorize_frames(&grid_plain_lines(grid), animation, frames)
}

/// Recolors a sequence of already-decoded, *already content-animated*
/// ASCII frames — e.g. the `Vec<String>` rows of each GIF frame from
/// [`crate::image::gif_to_lines`] (`frames.iter().map(|(lines, _)| lines)`)
/// — using `animation`, where each frame's position in `frames` is used as
/// its animation frame index. That keeps the color animation in sync with
/// the sequence's own timeline instead of restarting the color animation
/// from frame 0 on every call. Frames are recolored in parallel (see
/// [`colorize_frames`]'s docs on why that needs no feature flag here).
pub fn colorize_sequence(frames: &[Vec<String>], animation: &PixelAnimation) -> Vec<String> {
    frames
        .par_iter()
        .enumerate()
        .map(|(i, lines)| colorize_lines(lines, animation, i as u64))
        .collect()
}

/// Same as [`colorize_sequence`], but for frames already joined into one
/// `\n`-separated `String` per frame — the shape
/// [`crate::video::render_video_frames`]/[`crate::video::stream_video_frames`]
/// return, and the shape to ask for via
/// [`crate::video::ColorMode::Plain`] when you want a [`PixelAnimation`],
/// not `iascii`'s own pixel coloring, driving a video's ASCII output.
///
/// Each frame's `\n`-separated rows are split into borrowed `&str` slices
/// rather than cloned into owned `String`s before coloring — a `String`
/// per row was pure overhead, since [`colorize_lines`] never needed to own
/// them in the first place.
pub fn colorize_joined_sequence(frames: &[String], animation: &PixelAnimation) -> Vec<String> {
    frames
        .par_iter()
        .enumerate()
        .map(|(i, frame)| {
            let lines: Vec<&str> = frame.lines().collect();
            colorize_lines(&lines, animation, i as u64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn spaces_are_never_wrapped_in_color_escapes() {
        let lines = block(&["# #", " # "]);
        let out = colorize_lines(&lines, &PixelAnimation::Rainbow { speed: 10.0 }, 0);
        // Every space in the original stays a bare space in the output —
        // i.e. no "\x1b[...m \x1b[0m" pair was emitted for it.
        assert!(!out.contains("m \x1b[0m"));
    }

    #[test]
    fn rainbow_colors_every_non_space_cell_and_resets_after_each() {
        let lines = block(&["##", "##"]);
        let out = colorize_lines(&lines, &PixelAnimation::Rainbow { speed: 5.0 }, 2);
        assert_eq!(
            out.matches("\x1b[38;2;").count(),
            4,
            "one color per '#' cell"
        );
        assert_eq!(out.matches("\x1b[0m").count(), 4);
    }

    #[test]
    fn blink_alternates_between_the_two_configured_colors_by_frame() {
        let lines = block(&["#"]);
        let animation = PixelAnimation::Blink {
            on_color: Rgb::new(255, 0, 0),
            off_color: Rgb::new(0, 0, 255),
        };
        assert!(colorize_lines(&lines, &animation, 0).contains("38;2;255;0;0"));
        assert!(colorize_lines(&lines, &animation, 1).contains("38;2;0;0;255"));
    }

    #[test]
    fn colorize_frames_generates_the_requested_number_of_frames() {
        let lines = block(&["###"]);
        let frames = colorize_frames(
            &lines,
            &PixelAnimation::Plasma {
                scale: 0.3,
                speed: 0.2,
            },
            6,
        );
        assert_eq!(frames.len(), 6);
        for frame in &frames {
            assert!(frame.contains("\x1b[38;2;"));
        }
    }

    #[test]
    fn custom_closure_receives_the_expected_coordinates_and_character() {
        let lines = block(&["ab"]);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let animation = PixelAnimation::custom(move |x, y, frame, ch| {
            recorder.lock().unwrap().push((x, y, frame, ch));
            Rgb::WHITE
        });
        colorize_lines(&lines, &animation, 7);
        let calls = seen.lock().unwrap();
        assert_eq!(&*calls, &[(0, 0, 7, 'a'), (1, 0, 7, 'b')]);
    }

    #[test]
    fn colorize_sequence_uses_each_frames_own_index_as_animation_time() {
        // A Blink animation alternates by parity of the frame index, so
        // consecutive sequence entries (indices 0 and 1) must land on
        // opposite colors even though each is colorized independently.
        let animation = PixelAnimation::Blink {
            on_color: Rgb::new(255, 0, 0),
            off_color: Rgb::new(0, 0, 255),
        };
        let frames = vec![block(&["#"]), block(&["#"])];
        let out = colorize_sequence(&frames, &animation);
        assert!(out[0].contains("38;2;255;0;0"));
        assert!(out[1].contains("38;2;0;0;255"));
    }

    #[test]
    fn colorize_joined_sequence_splits_on_newlines_before_recoloring() {
        let animation = PixelAnimation::Blink {
            on_color: Rgb::new(255, 0, 0),
            off_color: Rgb::new(0, 0, 255),
        };
        let frames = vec!["##\n##".to_string(), "##\n##".to_string()];
        let out = colorize_joined_sequence(&frames, &animation);
        assert_eq!(out.len(), 2);
        assert!(out[0].lines().count() == 2);
        assert!(out[0].contains("38;2;255;0;0"));
        assert!(out[1].contains("38;2;0;0;255"));
    }

    #[test]
    fn adjacent_and_cross_row_same_color_cells_share_one_escape_pair() {
        // A custom animation that always returns the same color regardless
        // of position should collapse an entire multi-row, multi-column
        // frame into exactly one open + one reset (color state persists
        // across a bare '\n' on every real terminal), instead of one pair
        // per non-space cell.
        let lines = block(&["####", "####", "####"]);
        let animation = PixelAnimation::custom(|_, _, _, _| Rgb::new(10, 20, 30));
        let out = colorize_lines(&lines, &animation, 0);
        assert_eq!(out.matches("\x1b[38;2;10;20;30m").count(), 1);
        assert_eq!(out.matches("\x1b[0m").count(), 1);
        assert_eq!(out.lines().count(), 3, "row newlines are still present");
    }

    #[test]
    fn a_space_breaks_a_run_even_when_the_color_either_side_matches() {
        let lines = block(&["# #"]);
        let animation = PixelAnimation::custom(|_, _, _, _| Rgb::new(1, 2, 3));
        let out = colorize_lines(&lines, &animation, 0);
        // Two separate '#' runs either side of the space, so two opens and
        // two resets, not one merged run spanning the space.
        assert_eq!(out.matches("\x1b[38;2;1;2;3m").count(), 2);
        assert_eq!(out.matches("\x1b[0m").count(), 2);
    }

    #[test]
    fn repeated_non_adjacent_colors_still_produce_the_correct_escapes() {
        // Alternates by column parity, so on a wide row the same two
        // colors repeat many times non-contiguously — exercising the
        // escape-string cache's correctness (not just its speed): every
        // occurrence, cached or freshly formatted, must still be right.
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let animation =
            PixelAnimation::custom(move |x, _, _, _| if x % 2 == 0 { red } else { blue });
        let out = colorize_lines(&block(&["##########"]), &animation, 0);
        assert_eq!(out.matches("\x1b[38;2;255;0;0m").count(), 5);
        assert_eq!(out.matches("\x1b[38;2;0;0;255m").count(), 5);
    }

    #[test]
    fn colorize_lines_accepts_borrowed_str_slices_without_cloning() {
        // colorize_joined_sequence relies on this: `S = &str` must work
        // directly, not just `S = String`.
        let borrowed: Vec<&str> = vec!["#", "#"];
        let out = colorize_lines(&borrowed, &PixelAnimation::Rainbow { speed: 4.0 }, 0);
        assert!(out.contains("\x1b[38;2;"));
    }

    #[test]
    fn ripple_and_wave_do_not_panic_on_a_single_cell_grid() {
        // width/height of 1 exercises the smallest possible denominators
        // in Wave's `span` and Ripple's center-distance math.
        let lines = block(&["#"]);
        for animation in [
            PixelAnimation::Wave {
                base_color: Rgb::WHITE,
            },
            PixelAnimation::Ripple {
                base_color: Rgb::WHITE,
                speed: 0.3,
            },
        ] {
            let out = colorize_lines(&lines, &animation, 3);
            assert!(out.contains("\x1b[38;2;"));
        }
    }
}
