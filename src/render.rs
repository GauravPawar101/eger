//! Output targets for rendered ASCII art: stdout, a file, an owned `String`,
//! or a `Vec<String>` (one entry per row/frame) — plus terminal playback for
//! frame sequences.

#[cfg(feature = "video")]
use crate::error::MaramuraError;
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
    /// Print directly to stdout.
    Stdout,
    /// Write to a single file.
    File(PathBuf),
    /// Return as one owned `String`.
    String,
    /// Return as `Vec<String>`, one entry per row.
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

/// Renders a completed `Grid` to a string. Pass `Some(depth)` for an
/// ANSI-colored terminal string, or `None` for plain characters only.
pub fn grid_to_string(grid: &Grid, depth: Option<ColorDepth>) -> String {
    match depth {
        Some(depth) => render_ansi(grid, depth),
        None => plain_string(grid),
    }
}

/// Renders a `Grid` without any color escape codes — just characters and
/// newlines, one row per line.
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
        // Build each row's String directly from the grid in one pass,
        // instead of rendering the whole grid to one big String and then
        // re-splitting it on '\n' (which both allocates and copies twice).
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

/// Picks a [`ColorDepth`] appropriate for the current environment, or
/// `None` to mean "render plain, uncolored text" — a drop-in value for any
/// `Option<ColorDepth>` parameter in this module (or `render_video_frames`'s
/// `depth_override`).
///
/// This exists for accessibility and cross-terminal compatibility: blasting
/// 24-bit truecolor escape codes at a terminal that only understands 16
/// colors renders garbled output, and forcing color at all onto output
/// that's been redirected to a file or another process (or onto a screen
/// reader, or for a user who's opted out) pollutes it with escape bytes
/// nobody asked for. Honors two conventions:
/// - [`NO_COLOR`](https://no-color.org): if set (to any value), or if
///   `TERM=dumb`, or if stdout isn't actually a terminal, this returns
///   `None` (plain text).
/// - `COLORTERM`/`TERM`: otherwise picks the richest depth the terminal
///   advertises support for (`COLORTERM=truecolor`/`24bit` → `TrueColor`;
///   `TERM` containing `256color` → `Ansi256`; otherwise the universally
///   supported `Ansi16`).
///
/// This is purely a convenience default — anything passed explicitly (e.g.
/// `Config::color_depth`, or a literal `Some(depth)`) always takes
/// precedence; nothing in this crate calls this function automatically.
pub fn detect_render_depth() -> Option<ColorDepth> {
    use std::io::IsTerminal;
    detect_render_depth_from(
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("COLORTERM").ok(),
        std::env::var("TERM").ok(),
        std::io::stdout().is_terminal(),
    )
}

/// Pure decision logic behind [`detect_render_depth`], taking its inputs as
/// plain values so it's testable without mutating real process
/// environment/state (which would otherwise make tests racy against each
/// other, since env vars and stdout are global, shared, and mutable).
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

/// Dispatches a rendered `Grid` to `target`, synchronously (uses blocking
/// std I/O — safe to call from Rayon workers or plain sync code).
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

/// Async counterpart of [`dispatch`], for use inside Tokio tasks (the video
/// pipeline, or batch I/O that shouldn't block the runtime).
///
/// Requires the `video` feature (which pulls in `tokio`).
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

/// Plays back a sequence of already-rendered ASCII frames in the terminal at
/// a fixed frame rate, clearing the screen between frames. Used for video
/// preview (see [`crate::video::play_video`]).
///
/// Requires the `video` feature (which pulls in `tokio`).
#[cfg(feature = "video")]
pub async fn play_terminal(frames: &[String], fps: f64) -> Result<()> {
    if !(fps > 0.0) {
        return Err(MaramuraError::Render("fps must be positive".into()));
    }
    let frame_delay = Duration::from_secs_f64(1.0 / fps);
    let mut stdout = tokio::io::stdout();
    for frame in frames {
        // Cursor home + clear screen, then draw the next frame.
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

    /// Builds a real 2x2 `Grid` (via `iascii::convert::convert_image`, the
    /// only way to construct one — `Grid`/`Cell` have no public
    /// constructors) from a checkerboard of pure-black/pure-white pixels.
    /// `Explicit` sizing matching the source resolution means each output
    /// cell samples exactly one source pixel, so black and white land on
    /// opposite ends of whatever ramp `iascii` uses by default — giving
    /// predictable *structure* (two distinct characters, 2 rows of 2) even
    /// though we don't hardcode which literal characters those are.
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
        // 2 rows, each newline-terminated, each holding `width` characters.
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), grid.height() as usize);
        for line in &lines {
            assert_eq!(line.chars().count(), grid.width() as usize);
        }
    }

    #[test]
    fn checkerboard_pixels_map_to_two_distinct_characters() {
        // Sanity check that conversion is actually pixel-sensitive: a pure
        // black cell and a pure white cell shouldn't collapse to the same
        // glyph under the default ramp.
        let grid = sample_grid();
        let s = plain_string(&grid);
        let distinct: std::collections::HashSet<char> = s.chars().filter(|c| *c != '\n').collect();
        assert!(
            distinct.len() >= 2,
            "expected black/white pixels to render as different characters, got {s:?}"
        );
    }

    #[test]
    fn grid_to_lines_none_depth_matches_row_by_row_plain_reconstruction() {
        // grid_to_lines(None) takes a fast path that builds each line
        // directly from grid.rows() instead of grid_to_string(None) + split.
        // Verify it still agrees with that slower reference construction.
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

    // --- detect_render_depth_from (accessibility / terminal compatibility) --

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
        // e.g. output piped to a file or another process.
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
        assert_eq!(
            detect_render_depth_from(false, Some("24bit".into()), None, true),
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
        assert_eq!(
            detect_render_depth_from(false, None, None, true),
            Some(ColorDepth::Ansi16)
        );
    }

    #[test]
    fn grid_to_string_with_none_depth_matches_plain_string() {
        let grid = sample_grid();
        assert_eq!(grid_to_string(&grid, None), plain_string(&grid));
    }

    #[test]
    fn grid_to_string_none_depth_omits_color_and_some_depth_colors() {
        let grid = sample_grid();
        let plain = grid_to_string(&grid, None);
        let colored = grid_to_string(&grid, Some(ColorDepth::TrueColor));
        assert!(!plain.contains('\x1b'));
        assert!(colored.contains('\x1b'));
        // The colored rendering should still carry the same glyphs as plain,
        // just wrapped in escape codes.
        for ch in plain.chars().filter(|c| *c != '\n') {
            assert!(colored.contains(ch));
        }
    }

    #[test]
    fn different_color_depths_produce_different_encodings() {
        let grid = sample_grid();
        let truecolor = grid_to_string(&grid, Some(ColorDepth::TrueColor));
        let ansi16 = grid_to_string(&grid, Some(ColorDepth::Ansi16));
        assert!(truecolor.contains('\x1b'));
        assert!(ansi16.contains('\x1b'));
        assert_ne!(
            truecolor, ansi16,
            "TrueColor and Ansi16 should use different escape encodings"
        );
    }

    #[test]
    fn grid_to_lines_splits_one_string_per_row() {
        let grid = sample_grid();
        let lines = grid_to_lines(&grid, None);
        assert_eq!(lines.len(), grid.height() as usize);
        assert_eq!(lines.join("\n") + "\n", plain_string(&grid));
    }

    #[test]
    fn dispatch_string_target_returns_text() {
        let grid = sample_grid();
        let out = dispatch(&RenderTarget::String, &grid, None).unwrap();
        match out {
            RenderOutput::Text(t) => assert_eq!(t, plain_string(&grid)),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_lines_target_returns_lines() {
        let grid = sample_grid();
        let out = dispatch(&RenderTarget::Lines, &grid, None).unwrap();
        match out {
            RenderOutput::Lines(lines) => assert_eq!(lines, grid_to_lines(&grid, None)),
            other => panic!("expected Lines, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_lines_target_with_color_depth_matches_grid_to_lines() {
        let grid = sample_grid();
        let out = dispatch(&RenderTarget::Lines, &grid, Some(ColorDepth::Ansi256)).unwrap();
        match out {
            RenderOutput::Lines(lines) => {
                assert_eq!(lines, grid_to_lines(&grid, Some(ColorDepth::Ansi256)));
                assert!(lines.iter().all(|l| l.contains('\x1b')));
            }
            other => panic!("expected Lines, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_file_target_with_color_depth_writes_ansi_escapes() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "maramura-render-color-test-{}.txt",
            std::process::id()
        ));
        let grid = sample_grid();

        dispatch(
            &RenderTarget::File(path.clone()),
            &grid,
            Some(ColorDepth::TrueColor),
        )
        .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains('\x1b'));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn detect_render_depth_from_unrecognized_colorterm_value_falls_back_to_ansi16() {
        // An unrecognized COLORTERM value (not "truecolor"/"24bit") should
        // not be treated as a TrueColor signal — it should fall through to
        // checking TERM, and then to the Ansi16 baseline.
        assert_eq!(
            detect_render_depth_from(
                false,
                Some("something-unexpected".into()),
                Some("xterm".into()),
                true
            ),
            Some(ColorDepth::Ansi16)
        );
    }

    #[test]
    fn dispatch_file_target_writes_and_returns_path() {
        let mut path = std::env::temp_dir();
        path.push(format!("maramura-render-test-{}.txt", std::process::id()));
        let grid = sample_grid();

        let out = dispatch(&RenderTarget::File(path.clone()), &grid, None).unwrap();
        match out {
            RenderOutput::Written(p) => assert_eq!(p, path),
            other => panic!("expected Written, got {other:?}"),
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, plain_string(&grid));
        std::fs::remove_file(&path).ok();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn dispatch_async_string_target_matches_sync_dispatch() {
        let grid = sample_grid();
        let sync_out = dispatch(&RenderTarget::String, &grid, Some(ColorDepth::TrueColor)).unwrap();
        let async_out = dispatch_async(&RenderTarget::String, &grid, Some(ColorDepth::TrueColor))
            .await
            .unwrap();
        match (sync_out, async_out) {
            (RenderOutput::Text(a), RenderOutput::Text(b)) => assert_eq!(a, b),
            _ => panic!("expected matching Text outputs"),
        }
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn dispatch_async_file_target_writes_file() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "maramura-render-async-test-{}.txt",
            std::process::id()
        ));
        let grid = sample_grid();

        dispatch_async(&RenderTarget::File(path.clone()), &grid, None)
            .await
            .unwrap();
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(contents, plain_string(&grid));
        tokio::fs::remove_file(&path).await.ok();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn play_terminal_rejects_non_positive_fps() {
        let frames = vec!["frame".to_string()];
        let err = play_terminal(&frames, 0.0).await.unwrap_err();
        assert!(matches!(err, MaramuraError::Render(_)));

        let err = play_terminal(&frames, -1.0).await.unwrap_err();
        assert!(matches!(err, MaramuraError::Render(_)));
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn play_terminal_plays_all_frames_without_error() {
        // Small frame count and high fps keeps this test fast.
        let frames = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        play_terminal(&frames, 1000.0).await.unwrap();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn play_terminal_handles_empty_frame_list() {
        let frames: Vec<String> = vec![];
        play_terminal(&frames, 30.0).await.unwrap();
    }
}
