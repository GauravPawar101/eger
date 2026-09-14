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
