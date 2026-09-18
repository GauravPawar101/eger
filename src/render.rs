//! Output targets for rendered ASCII art: stdout, a file, an owned `String`,
//! or a `Vec<String>` — plus terminal playback for frame sequences.

#[cfg(feature = "video")]
use crate::error::EgerError;
use crate::error::Result;
use iascii::grid::Grid;
use iascii::render::{render_ansi, ColorDepth};
use std::io::Write;
use std::path::PathBuf;
#[cfg(feature = "video")]
use std::time::Duration;
#[cfg(feature = "video")]
use tokio::io::AsyncWriteExt;

/// Where a rendered frame/image should be delivered.
#[derive(Debug, Clone)]
pub enum RenderTarget {
    Stdout,
    File(PathBuf),
    String,
    Lines,
}

/// The materialized result of a render, mirroring [`RenderTarget`].
#[derive(Debug)]
pub enum RenderOutput {
    Displayed,
    Written(PathBuf),
    Text(String),
    Lines(Vec<String>),
}

/// Renders a completed `Grid` to a string.
pub fn grid_to_string(grid: &Grid, depth: Option<ColorDepth>) -> String {
    match depth {
        Some(depth) => render_ansi(grid, depth),
        None => plain_string(grid),
    }
}

/// Renders a `Grid` without any color escape codes.
pub fn plain_string(grid: &Grid) -> String {
    let mut out = String::with_capacity((grid.width() as usize + 1) * grid.height() as usize);
    for row in grid.rows() {
        for cell in row {
            out.push(cell.ch);
        }
        out.push('\n');
    }
    out
}

/// Renders a `Grid` into one `String` per row.
pub fn grid_to_lines(grid: &Grid, depth: Option<ColorDepth>) -> Vec<String> {
    match depth {
        None => grid
            .rows()
            .map(|row| row.iter().map(|cell| cell.ch).collect())
            .collect(),
        Some(_) => grid_to_string(grid, depth)
            .lines()
            .map(str::to_owned)
            .collect(),
    }
}

/// Picks a [`ColorDepth`] appropriate for the current environment.
pub fn detect_render_depth() -> Option<ColorDepth> {
    use std::io::IsTerminal;
    detect_render_depth_from(
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("COLORTERM").ok(),
        std::env::var("TERM").ok(),
        std::io::stdout().is_terminal(),
    )
}

fn detect_render_depth_from(
    no_color: bool,
    colorterm: Option<String>,
    term: Option<String>,
    is_tty: bool,
) -> Option<ColorDepth> {
    if no_color || !is_tty {
        return None;
    }
    if term.as_deref() == Some("dumb") {
        return None;
    }
    if colorterm
        .as_deref()
        .is_some_and(|c| c == "truecolor" || c == "24bit")
    {
        return Some(ColorDepth::TrueColor);
    }
    if term.as_deref().is_some_and(|t| t.contains("256color")) {
        return Some(ColorDepth::Ansi256);
    }
    Some(ColorDepth::Ansi16)
}

/// Queries the current terminal's size in columns/rows, if stdout is
/// attached to one. Returns `None` when output is piped/redirected or the
/// size can't be determined (e.g. no controlling terminal, as in most CI
/// environments).
///
/// Backed by the `terminal_size` crate (`terminal_size::terminal_size`),
/// which reads the size via the platform's own mechanism (`ioctl(TIOCGWINSZ)`
/// on Unix, the console API on Windows) rather than environment variables,
/// so it stays correct across resizes without needing `SIGWINCH` handling.
pub fn terminal_size() -> Option<(u16, u16)> {
    terminal_size::terminal_size().map(|(terminal_size::Width(w), terminal_size::Height(h))| (w, h))
}

/// Controls how finely [`diff_frame`] repaints one frame over another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffGranularity {
    /// Redraw an entire line whenever any byte in it changed. Safe for
    /// ANSI-colored frames: an SGR color escape anywhere in the line
    /// changes its raw bytes even when the visible glyphs it wraps are
    /// unchanged, so cell-level diffing would misread it as a content
    /// change at the wrong column. This is the granularity to reach for
    /// unless you know your frames are plain, uncolored text.
    Line,
    /// Redraw only the individual character cells that changed within a
    /// line, computed by comparing the two lines' `char`s position by
    /// position. Only correct for plain (uncolored / no embedded escape
    /// sequences) frames — with ANSI escapes present, `char` position no
    /// longer lines up with the visible terminal column, and the emitted
    /// cursor moves would land in the wrong place.
    Cell,
}

/// Computes the minimal set of cursor-addressed writes needed to repaint
/// `next` over a terminal currently showing `prev`, instead of clearing and
/// redrawing the whole frame (as [`play_terminal`]/[`dispatch`]'s `Stdout`
/// target and [`crate::video::play_video`] do). Returns an empty string
/// when the two frames are identical.
///
/// Both frames are compared line by line (via `str::lines`); see
/// [`DiffGranularity`] for the tradeoff between the two granularities this
/// accepts.
pub fn diff_frame(prev: &str, next: &str, granularity: DiffGranularity) -> String {
    let prev_lines: Vec<&str> = prev.lines().collect();
    let next_lines: Vec<&str> = next.lines().collect();
    let max_lines = prev_lines.len().max(next_lines.len());

    let mut out = String::new();
    for row in 0..max_lines {
        let prev_line = prev_lines.get(row).copied().unwrap_or("");
        let next_line = next_lines.get(row).copied().unwrap_or("");
        if prev_line == next_line {
            continue;
        }
        match granularity {
            DiffGranularity::Line => {
                // \x1B[<row>;1H moves the cursor to the start of the row
                // (1-based); \x1B[K clears from the cursor to end-of-line
                // first so a next_line shorter than prev_line doesn't leave
                // stale trailing characters behind.
                out.push_str(&format!("\x1B[{};1H\x1B[K{next_line}", row + 1));
            }
            DiffGranularity::Cell => diff_row_cells(row, prev_line, next_line, &mut out),
        }
    }
    out
}

/// Appends the cursor-addressed writes needed to turn `prev_line` into
/// `next_line` on terminal row `row` (0-based) into `out`, changing only
/// the contiguous spans of characters that actually differ.
fn diff_row_cells(row: usize, prev_line: &str, next_line: &str, out: &mut String) {
    let prev_chars: Vec<char> = prev_line.chars().collect();
    let next_chars: Vec<char> = next_line.chars().collect();
    let max_len = prev_chars.len().max(next_chars.len());

    let mut col = 0;
    while col < max_len {
        let p = prev_chars.get(col).copied();
        let n = next_chars.get(col).copied();
        if p == n {
            col += 1;
            continue;
        }
        let start = col;
        let mut span = String::new();
        while col < max_len && prev_chars.get(col).copied() != next_chars.get(col).copied() {
            span.push(next_chars.get(col).copied().unwrap_or(' '));
            col += 1;
        }
        // Columns are 1-based in cursor-position escapes.
        out.push_str(&format!("\x1B[{};{}H{span}", row + 1, start + 1));
    }
    // next_line ran out before prev_line did: clear the leftover tail.
    if next_chars.len() < prev_chars.len() {
        out.push_str(&format!("\x1B[{};{}H\x1B[K", row + 1, next_chars.len() + 1));
    }
}

/// Plays back a sequence of already-rendered frames using cursor-addressed
/// diffing instead of [`play_terminal`]'s clear-and-redraw-every-frame
/// approach — much less flicker for animations where most of the frame is
/// unchanged between steps (a spinner, a status line, a slowly-panning
/// image). The first frame is always drawn in full; every frame after that
/// is patched in via [`diff_frame`].
#[cfg(feature = "video")]
pub async fn play_terminal_diffed(
    frames: &[String],
    fps: f64,
    granularity: DiffGranularity,
) -> Result<()> {
    if !(fps > 0.0) {
        return Err(EgerError::Render("fps must be positive".into()));
    }
    let Some((first, rest)) = frames.split_first() else {
        return Ok(());
    };

    let frame_delay = Duration::from_secs_f64(1.0 / fps);
    let mut stdout = tokio::io::stdout();

    stdout.write_all(b"\x1B[H\x1B[2J").await?;
    stdout.write_all(first.as_bytes()).await?;
    stdout.flush().await?;
    tokio::time::sleep(frame_delay).await;

    let mut prev = first;
    for next in rest {
        let patch = diff_frame(prev, next, granularity);
        if !patch.is_empty() {
            stdout.write_all(patch.as_bytes()).await?;
            stdout.flush().await?;
        }
        prev = next;
        tokio::time::sleep(frame_delay).await;
    }
    Ok(())
}

/// Dispatches a rendered `Grid` to `target`, synchronously.
pub fn dispatch(
    target: &RenderTarget,
    grid: &Grid,
    depth: Option<ColorDepth>,
) -> Result<RenderOutput> {
    match target {
        RenderTarget::Stdout => {
            let text = grid_to_string(grid, depth);
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(text.as_bytes())?;
            stdout.write_all(b"\n")?;
            Ok(RenderOutput::Displayed)
        }
        RenderTarget::File(path) => {
            let text = grid_to_string(grid, depth);
            std::fs::write(path, &text)?;
            Ok(RenderOutput::Written(path.clone()))
        }
        RenderTarget::String => Ok(RenderOutput::Text(grid_to_string(grid, depth))),
        RenderTarget::Lines => Ok(RenderOutput::Lines(grid_to_lines(grid, depth))),
    }
}

/// Async counterpart of [`dispatch`].
#[cfg(feature = "video")]
pub async fn dispatch_async(
    target: &RenderTarget,
    grid: &Grid,
    depth: Option<ColorDepth>,
) -> Result<RenderOutput> {
    match target {
        RenderTarget::Stdout => {
            let text = grid_to_string(grid, depth);
            let mut stdout = tokio::io::stdout();
            stdout.write_all(text.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
            Ok(RenderOutput::Displayed)
        }
        RenderTarget::File(path) => {
            let text = grid_to_string(grid, depth);
            tokio::fs::write(path, &text).await?;
            Ok(RenderOutput::Written(path.clone()))
        }
        RenderTarget::String => Ok(RenderOutput::Text(grid_to_string(grid, depth))),
        RenderTarget::Lines => Ok(RenderOutput::Lines(grid_to_lines(grid, depth))),
    }
}

/// Plays back a sequence of already-rendered ASCII frames in the terminal.
#[cfg(feature = "video")]
pub async fn play_terminal(frames: &[String], fps: f64) -> Result<()> {
    if !(fps > 0.0) {
        return Err(EgerError::Render("fps must be positive".into()));
    }
    let frame_delay = Duration::from_secs_f64(1.0 / fps);
    let mut stdout = tokio::io::stdout();
    for frame in frames {
        stdout.write_all(b"\x1B[H\x1B[2J").await?;
        stdout.write_all(frame.as_bytes()).await?;
        stdout.flush().await?;
        tokio::time::sleep(frame_delay).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
    use iascii::convert::convert_image;

    fn sample_grid() -> Grid {
        let ascii_config = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::Explicit {
                width: 2,
                height: 2,
            })
            .build()
            .expect("ascii config should build with defaults");

        #[rustfmt::skip]
        let pixels: [u8; 2 * 2 * 3] = [
            0, 0, 0,        255, 255, 255,
            255, 255, 255,  0, 0, 0,
        ];
        convert_image(2, 2, &pixels, &ascii_config)
            .expect("conversion of a valid buffer should succeed")
    }

    #[test]
    fn plain_string_has_no_escape_codes_and_expected_shape() {
        let grid = sample_grid();
        let s = plain_string(&grid);
        assert!(!s.contains('\x1b'));
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), grid.height() as usize);
        for line in &lines {
            assert_eq!(line.chars().count(), grid.width() as usize);
        }
    }

    #[test]
    fn checkerboard_pixels_map_to_two_distinct_characters() {
        let grid = sample_grid();
        let s = plain_string(&grid);
        let distinct: std::collections::HashSet<char> = s.chars().filter(|c| *c != '\n').collect();
        assert!(distinct.len() >= 2);
    }

    #[test]
    fn grid_to_lines_none_depth_matches_row_by_row_plain_reconstruction() {
        let grid = sample_grid();
        let expected: Vec<String> = plain_string(&grid).lines().map(str::to_owned).collect();
        assert_eq!(grid_to_lines(&grid, None), expected);
    }

    #[test]
    fn grid_to_lines_some_depth_splits_colored_output_by_row() {
        let grid = sample_grid();
        let lines = grid_to_lines(&grid, Some(ColorDepth::TrueColor));
        assert_eq!(lines.len(), grid.height() as usize);
        for line in &lines {
            assert!(line.contains('\x1b'));
        }
    }

    #[test]
    fn no_color_env_var_forces_plain_even_on_a_rich_terminal() {
        assert_eq!(
            detect_render_depth_from(
                true,
                Some("truecolor".into()),
                Some("xterm-256color".into()),
                true
            ),
            None
        );
    }

    #[test]
    fn non_terminal_stdout_forces_plain_regardless_of_env() {
        assert_eq!(
            detect_render_depth_from(
                false,
                Some("truecolor".into()),
                Some("xterm-256color".into()),
                false
            ),
            None
        );
    }

    #[test]
    fn dumb_terminal_forces_plain() {
        assert_eq!(
            detect_render_depth_from(false, None, Some("dumb".into()), true),
            None
        );
    }

    #[test]
    fn colorterm_truecolor_picked_over_lesser_term_signals() {
        assert_eq!(
            detect_render_depth_from(false, Some("truecolor".into()), Some("xterm".into()), true),
            Some(ColorDepth::TrueColor)
        );
    }

    #[test]
    fn term_256color_picked_when_colorterm_absent() {
        assert_eq!(
            detect_render_depth_from(false, None, Some("screen-256color".into()), true),
            Some(ColorDepth::Ansi256)
        );
    }

    #[test]
    fn falls_back_to_universally_supported_ansi16() {
        assert_eq!(
            detect_render_depth_from(false, None, Some("xterm".into()), true),
            Some(ColorDepth::Ansi16)
        );
    }

    #[test]
    fn diff_frame_is_empty_for_identical_frames() {
        let a = "line one\nline two\n";
        assert_eq!(diff_frame(a, a, DiffGranularity::Line), "");
        assert_eq!(diff_frame(a, a, DiffGranularity::Cell), "");
    }

    #[test]
    fn diff_frame_line_granularity_only_touches_changed_rows() {
        let prev = "same\nold\nsame";
        let next = "same\nnew\nsame";
        let patch = diff_frame(prev, next, DiffGranularity::Line);
        assert!(patch.contains("new"));
        assert!(!patch.contains("old"));
        // Only row 2 (1-based) should have been addressed.
        assert!(patch.contains("\x1B[2;1H"));
        assert!(!patch.contains("\x1B[1;1H"));
        assert!(!patch.contains("\x1B[3;1H"));
    }

    #[test]
    fn diff_frame_cell_granularity_only_rewrites_the_changed_span() {
        let prev = "abcXXXghi";
        let next = "abcYYYghi";
        let patch = diff_frame(prev, next, DiffGranularity::Cell);
        assert!(patch.contains("YYY"));
        // The unchanged prefix/suffix characters should not be re-sent.
        assert!(!patch.contains("abcYYY"));
        assert!(!patch.contains("YYYghi"));
    }

    #[test]
    fn diff_frame_cell_granularity_clears_a_shrunk_line_tail() {
        let prev = "hello world";
        let next = "hello";
        let patch = diff_frame(prev, next, DiffGranularity::Cell);
        // Should clear starting right after "hello" (column 6).
        assert!(patch.contains("\x1B[1;6H\x1B[K"));
    }

    #[test]
    fn diff_frame_handles_frames_with_different_line_counts() {
        let prev = "a\nb\nc";
        let next = "a\nb";
        let patch = diff_frame(prev, next, DiffGranularity::Line);
        // Row 3 existed in prev but not next; next_line is "" there, so it
        // should be cleared via the line-granularity path.
        assert!(patch.contains("\x1B[3;1H\x1B[K"));
    }

    #[test]
    fn terminal_size_does_not_panic_without_a_real_terminal() {
        // In a headless test runner stdout usually isn't a TTY, so this
        // should return None rather than erroring.
        let _ = terminal_size();
    }

    #[test]
    fn dispatch_file_target_writes_and_returns_path() {
        let mut path = std::env::temp_dir();
        path.push(format!("eger-render-test-{}.txt", std::process::id()));
        let grid = sample_grid();

        let out = dispatch(&RenderTarget::File(path.clone()), &grid, None).unwrap();
        match out {
            RenderOutput::Written(p) => assert_eq!(p, path),
            other => panic!("expected Written, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
