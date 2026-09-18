//! Text → ANSI rendering and animation.
//!
//! This module colors and animates a line of text in the terminal using raw
//! ANSI SGR escapes (it does not go through [`crate::render`]/`iascii`,
//! since there is no image `Grid` involved — just characters and color).
//!
//! Three ways to get frames:
//! - Built-in [`TextAnimation`] styles (`Typewriter`, `Rainbow`, `Wave`,
//!   `Blink`, `Marquee`, `Pulse`) — no external dependencies.
//! - [`CustomAnimation::Function`] — a Rust closure `Fn(&str, u64) -> String`
//!   you provide, called once per frame index. Fully in-process.
//! - [`CustomAnimation::Program`] — an external executable (a script, a
//!   compiled binary, anything on disk) that prints one or more frames to
//!   stdout, separated by [`FRAME_SEPARATOR`]. `eger` runs it once and
//!   splits its output into frames.
//!
//! [`play_text`] renders and plays an animation directly to stdout, blocking
//! the calling thread for the animation's duration; [`text_frames`] just
//! returns the frame strings so you can use them yourself.

use crate::error::{EgerError, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

/// Separator written between frames by multi-frame `eger` output (shared
/// with [`crate::video::video_to_file`]'s frame delimiter), and the
/// delimiter [`CustomAnimation::Program`] output is split on.
pub const FRAME_SEPARATOR: &str = "\n\x1E\n";

/// An RGB color used for text coloring / gradients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b)
    }

    pub(crate) fn ansi_fg(self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.0, self.1, self.2)
    }

    pub(crate) fn scale(self, brightness: f32) -> Self {
        let brightness = brightness.clamp(0.0, 1.0);
        Rgb(
            (f32::from(self.0) * brightness).round() as u8,
            (f32::from(self.1) * brightness).round() as u8,
            (f32::from(self.2) * brightness).round() as u8,
        )
    }
}

/// A custom, user-supplied animation source.
#[derive(Clone)]
pub enum CustomAnimation {
    /// Run an external program once; its stdout, split on
    /// [`FRAME_SEPARATOR`], becomes the frame sequence.
    ///
    /// The program is invoked as `path [args...] <text> <frame_count>` —
    /// `args` (e.g. `["-c", "some_script.sh"]` for `/bin/sh`) come first so
    /// interpreter flags stay attached to the interpreter, and `text` /
    /// `frame_count` are appended as the program's final two positional
    /// arguments.
    Program { path: PathBuf, args: Vec<String> },

    /// A closure called once per frame index (0-based) with the source
    /// text, returning that frame's (already ANSI-colored, if desired)
    /// content. Runs in-process — no subprocess overhead.
    Function(Arc<dyn Fn(&str, u64) -> String + Send + Sync>),
}

impl CustomAnimation {
    /// Convenience constructor for the common case of invoking a plain
    /// executable (no interpreter flags) with no extra arguments.
    pub fn program(path: impl Into<PathBuf>) -> Self {
        Self::Program {
            path: path.into(),
            args: Vec::new(),
        }
    }

    /// Wraps a closure as a [`CustomAnimation::Function`].
    pub fn function(f: impl Fn(&str, u64) -> String + Send + Sync + 'static) -> Self {
        Self::Function(Arc::new(f))
    }
}

impl std::fmt::Debug for CustomAnimation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Program { path, args } => f
                .debug_struct("Program")
                .field("path", path)
                .field("args", args)
                .finish(),
            Self::Function(_) => f.write_str("Function(<closure>)"),
        }
    }
}

/// A text animation style: one of the built-in defaults, or a
/// [`CustomAnimation`].
#[derive(Debug, Clone)]
pub enum TextAnimation {
    /// Reveals the text one character at a time.
    Typewriter,
    /// Cycles each character's hue through the color wheel over time.
    Rainbow,
    /// A brightness band sweeps across the text against a dim base color.
    Wave,
    /// Alternates between two colors every frame.
    Blink { on_color: Rgb, off_color: Rgb },
    /// Scrolls the text through a fixed-width window, wrapping around.
    Marquee { width: usize },
    /// The whole line breathes between dim and full brightness.
    Pulse,
    /// A user-supplied program or closure; see [`CustomAnimation`].
    Custom(CustomAnimation),
}

/// Options controlling how a [`TextAnimation`] is generated and played.
#[derive(Debug, Clone)]
pub struct TextAnimOptions {
    /// Frames played per second.
    pub fps: f64,
    /// How many frames to generate for animations that don't have a
    /// natural end (`Rainbow`, `Wave`, `Blink`, `Marquee`, `Pulse`, and
    /// custom animations). `Typewriter` stops on its own once the text is
    /// fully revealed, whichever comes first.
    pub frames: u64,
    /// Replay the frame sequence indefinitely once [`play_text`] reaches
    /// the end, instead of returning after one pass.
    pub loop_forever: bool,
    /// Base color used by animations that need one (`Typewriter`, `Wave`,
    /// `Pulse`).
    pub base_color: Rgb,
    /// Clear the terminal before drawing each frame.
    ///
    /// Ignored when [`TextAnimOptions::diff`] is `Some`: diffed playback
    /// repaints only the cells/lines that changed instead of clearing.
    pub clear_each_frame: bool,
    /// When set, [`play_text`]/[`play_text_async`] repaint each frame with
    /// cursor-addressed updates instead of clearing and redrawing the whole
    /// line — see [`crate::render::DiffGranularity`] for the tradeoff
    /// between the two granularities. `None` (the default) keeps the
    /// original clear-and-redraw behavior.
    pub diff: Option<crate::render::DiffGranularity>,
}

impl Default for TextAnimOptions {
    fn default() -> Self {
        Self {
            fps: 12.0,
            frames: 40,
            loop_forever: false,
            base_color: Rgb::WHITE,
            clear_each_frame: true,
            diff: None,
        }
    }
}

/// Generates every frame of `animation` for `text` up front.
///
/// For [`CustomAnimation::Program`], this blocks on running the program to
/// completion and capturing its full output before returning — simple and
/// robust, at the cost of not starting playback until the program exits.
/// (A future `stream_text_frames` could mirror
/// [`crate::video::stream_video_frames`] for programs that themselves run
/// for a long time and emit frames incrementally.)
pub fn text_frames(
    text: &str,
    animation: &TextAnimation,
    opts: &TextAnimOptions,
) -> Result<Vec<String>> {
    match animation {
        TextAnimation::Custom(CustomAnimation::Function(f)) => {
            Ok((0..opts.frames).map(|i| f(text, i)).collect())
        }
        TextAnimation::Custom(CustomAnimation::Program { path, args }) => {
            run_program_frames(text, path, args, opts.frames)
        }
        builtin => {
            let mut frames = Vec::with_capacity(opts.frames as usize);
            for i in 0..opts.frames {
                match builtin_frame(text, builtin, i, opts) {
                    Some(frame) => frames.push(frame),
                    None => break,
                }
            }
            Ok(frames)
        }
    }
}

/// Renders and plays `animation` for `text` directly to stdout, blocking
/// the current thread. See [`crate::video::play_video`]/[`crate::render::play_terminal`]
/// for the async, video-frame equivalent.
pub fn play_text(text: &str, animation: &TextAnimation, opts: &TextAnimOptions) -> Result<()> {
    if !(opts.fps > 0.0) {
        return Err(EgerError::Animation("fps must be positive".into()));
    }
    let frames = text_frames(text, animation, opts)?;
    if frames.is_empty() {
        return Err(EgerError::Animation("animation produced no frames".into()));
    }

    let frame_delay = Duration::from_secs_f64(1.0 / opts.fps);
    let mut stdout = std::io::stdout().lock();
    loop {
        // Reset at the start of every pass (including repeats when
        // `loop_forever` is set) so the first frame of each pass is always
        // a full redraw; only frames *within* a pass are diffed against
        // their immediate predecessor.
        let mut prev: Option<&str> = None;
        for frame in &frames {
            match (opts.diff, prev) {
                (Some(granularity), Some(prev_frame)) => {
                    let patch = crate::render::diff_frame(prev_frame, frame, granularity);
                    if !patch.is_empty() {
                        stdout.write_all(patch.as_bytes())?;
                    }
                }
                _ => {
                    if opts.clear_each_frame || opts.diff.is_some() {
                        stdout.write_all(b"\x1B[H\x1B[2J")?;
                    }
                    stdout.write_all(frame.as_bytes())?;
                    stdout.write_all(b"\n")?;
                }
            }
            stdout.flush()?;
            prev = Some(frame.as_str());
            std::thread::sleep(frame_delay);
        }
        if !opts.loop_forever {
            break;
        }
    }
    Ok(())
}

/// Async counterpart of [`play_text`], for callers already inside a Tokio
/// runtime (e.g. alongside [`crate::video::play_video`]). Frame generation
/// (including running a [`CustomAnimation::Program`]) still happens
/// synchronously before playback starts; only the per-frame delay is async.
#[cfg(feature = "video")]
pub async fn play_text_async(
    text: &str,
    animation: &TextAnimation,
    opts: &TextAnimOptions,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    if !(opts.fps > 0.0) {
        return Err(EgerError::Animation("fps must be positive".into()));
    }
    let frames = text_frames(text, animation, opts)?;
    if frames.is_empty() {
        return Err(EgerError::Animation("animation produced no frames".into()));
    }

    let frame_delay = Duration::from_secs_f64(1.0 / opts.fps);
    let mut stdout = tokio::io::stdout();
    loop {
        let mut prev: Option<&str> = None;
        for frame in &frames {
            match (opts.diff, prev) {
                (Some(granularity), Some(prev_frame)) => {
                    let patch = crate::render::diff_frame(prev_frame, frame, granularity);
                    if !patch.is_empty() {
                        stdout.write_all(patch.as_bytes()).await?;
                    }
                }
                _ => {
                    if opts.clear_each_frame || opts.diff.is_some() {
                        stdout.write_all(b"\x1B[H\x1B[2J").await?;
                    }
                    stdout.write_all(frame.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                }
            }
            stdout.flush().await?;
            prev = Some(frame.as_str());
            tokio::time::sleep(frame_delay).await;
        }
        if !opts.loop_forever {
            break;
        }
    }
    Ok(())
}

/// Streams frames from a [`CustomAnimation::Program`] as they are produced
/// by the child process, instead of [`text_frames`]'s behavior of blocking
/// until the whole subprocess output is captured. Mirrors
/// [`crate::video::stream_video_frames`]'s incremental design: a
/// long-running generator program (e.g. one that reacts to live system
/// stats and keeps emitting frames) can push frames to `tx` as soon as it
/// writes a [`FRAME_SEPARATOR`]-delimited chunk to stdout, so playback can
/// start before the program exits.
///
/// Only meaningful for a program-driven [`CustomAnimation::Program`]; the
/// built-in styles and [`CustomAnimation::Function`] are already
/// synchronous/in-process and should keep using [`text_frames`].
///
/// The program is invoked the same way as in [`text_frames`]/
/// `run_program_frames`: `path [args...] <text> <frame_count>`.
#[cfg(feature = "video")]
pub async fn stream_text_frames(
    text: String,
    path: PathBuf,
    args: Vec<String>,
    frame_count: u64,
    tx: tokio::sync::mpsc::Sender<Result<String>>,
) -> Result<()> {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command as TokioCommand;

    let mut child = TokioCommand::new(&path)
        .args(&args)
        .arg(&text)
        .arg(frame_count.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(EgerError::AnimationSpawn)?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| EgerError::Animation("failed to capture program stdout".into()))?;

    // Incrementally read raw bytes and split on FRAME_SEPARATOR as
    // complete frames arrive, rather than waiting for EOF like
    // `run_program_frames` does.
    let mut buffer = String::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stdout.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buffer.push_str(&String::from_utf8_lossy(&chunk[..n]));
        while let Some(pos) = buffer.find(FRAME_SEPARATOR) {
            let frame = buffer[..pos].to_owned();
            buffer.drain(..pos + FRAME_SEPARATOR.len());
            if frame.is_empty() {
                continue;
            }
            if tx.send(Ok(frame)).await.is_err() {
                // Receiver dropped; stop reading and let the process be
                // cleaned up below rather than leaking it.
                let _ = child.kill().await;
                return Ok(());
            }
        }
    }
    // Flush any trailing partial frame the program didn't terminate with a
    // separator (e.g. it just exited after its last `print`).
    if !buffer.is_empty() {
        let _ = tx.send(Ok(buffer)).await;
    }

    let mut stderr_buf = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_buf).await;
    }
    let status = child.wait().await?;
    if !status.success() {
        let _ = tx
            .send(Err(EgerError::AnimationProgram(status, stderr_buf)))
            .await;
    }
    Ok(())
}

fn builtin_frame(
    text: &str,
    animation: &TextAnimation,
    index: u64,
    opts: &TextAnimOptions,
) -> Option<String> {
    match animation {
        TextAnimation::Typewriter => {
            let total_chars = text.chars().count();
            let chars_to_show = index as usize + 1;
            if chars_to_show > total_chars {
                return None;
            }
            let visible: String = text.chars().take(chars_to_show).collect();
            Some(format!("{}{visible}\x1b[0m", opts.base_color.ansi_fg()))
        }
        TextAnimation::Rainbow => {
            let hue_shift = (index as f32) * 12.0;
            let colored: String = text
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let hue = (hue_shift + i as f32 * 18.0) % 360.0;
                    let color = hsv_to_rgb(hue, 1.0, 1.0);
                    format!("{}{c}", color.ansi_fg())
                })
                .collect();
            Some(format!("{colored}\x1b[0m"))
        }
        TextAnimation::Wave => {
            let char_count = text.chars().count() as i64;
            if char_count == 0 {
                return Some(String::new());
            }
            let span = char_count + 8;
            let pos = (index as i64) % span;
            let colored: String = text
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let dist = (i as i64 - pos).unsigned_abs() as f32;
                    let brightness = (1.0 - (dist / 4.0).min(1.0)).max(0.15);
                    format!("{}{c}", opts.base_color.scale(brightness).ansi_fg())
                })
                .collect();
            Some(format!("{colored}\x1b[0m"))
        }
        TextAnimation::Blink {
            on_color,
            off_color,
        } => {
            let color = if index % 2 == 0 {
                *on_color
            } else {
                *off_color
            };
            Some(format!("{}{text}\x1b[0m", color.ansi_fg()))
        }
        TextAnimation::Marquee { width } => {
            if *width == 0 || text.is_empty() {
                return Some(String::new());
            }
            let padded = format!("{text}    ");
            let len = padded.chars().count();
            let offset = (index as usize) % len;
            let window: String = padded.chars().cycle().skip(offset).take(*width).collect();
            Some(format!("{}{window}\x1b[0m", opts.base_color.ansi_fg()))
        }
        TextAnimation::Pulse => {
            let t = index as f32 * 0.35;
            let brightness = (0.5 + 0.5 * t.sin()).clamp(0.15, 1.0);
            Some(format!(
                "{}{text}\x1b[0m",
                opts.base_color.scale(brightness).ansi_fg()
            ))
        }
        TextAnimation::Custom(_) => {
            unreachable!("custom animations are handled directly in text_frames")
        }
    }
}

/// Runs `path` as `path [args...] <text> <frame_count>` and splits its
/// captured stdout on [`FRAME_SEPARATOR`] into frames.
fn run_program_frames(
    text: &str,
    path: &Path,
    args: &[String],
    frame_count: u64,
) -> Result<Vec<String>> {
    let output = Command::new(path)
        .args(args)
        .arg(text)
        .arg(frame_count.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(EgerError::AnimationSpawn)?;

    if !output.status.success() {
        return Err(EgerError::AnimationProgram(
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let frames: Vec<String> = stdout
        .split(FRAME_SEPARATOR)
        .map(str::to_owned)
        .filter(|f| !f.is_empty())
        .collect();
    Ok(frames)
}

/// Converts an HSV color (`h` in `0..360`, `s`/`v` in `0.0..=1.0`) to RGB.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Rgb(
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(frames: u64) -> TextAnimOptions {
        TextAnimOptions {
            frames,
            ..Default::default()
        }
    }

    #[test]
    fn typewriter_reveals_one_more_character_per_frame_then_stops() {
        let frames = text_frames("hi!", &TextAnimation::Typewriter, &opts(10)).unwrap();
        assert_eq!(frames.len(), 3, "should stop once text is fully revealed");
        assert!(frames[0].contains('h') && !frames[0].contains('i'));
        assert!(frames[2].contains("hi!"));
    }

    #[test]
    fn rainbow_colors_every_character_and_resets_at_the_end() {
        let frames = text_frames("ab", &TextAnimation::Rainbow, &opts(5)).unwrap();
        assert_eq!(frames.len(), 5);
        for frame in &frames {
            assert!(frame.ends_with("\x1b[0m"));
            assert_eq!(frame.matches("\x1b[38;2;").count(), 2, "one color per char");
        }
    }

    #[test]
    fn blink_alternates_between_the_two_configured_colors() {
        let animation = TextAnimation::Blink {
            on_color: Rgb::new(255, 0, 0),
            off_color: Rgb::new(0, 0, 255),
        };
        let frames = text_frames("x", &animation, &opts(4)).unwrap();
        assert!(frames[0].contains("38;2;255;0;0"));
        assert!(frames[1].contains("38;2;0;0;255"));
        assert!(frames[2].contains("38;2;255;0;0"));
    }

    #[test]
    fn marquee_window_has_the_requested_width_and_wraps() {
        let animation = TextAnimation::Marquee { width: 3 };
        let frames = text_frames("ab", &animation, &opts(20)).unwrap();
        for frame in &frames {
            let visible: String = frame
                .trim_end_matches("\x1b[0m")
                .rsplit('m')
                .next()
                .unwrap()
                .to_string();
            assert_eq!(visible.chars().count(), 3);
        }
    }

    #[test]
    fn function_custom_animation_calls_the_closure_once_per_frame() {
        let animation =
            TextAnimation::Custom(CustomAnimation::function(|text, i| format!("{text}-{i}")));
        let frames = text_frames("go", &animation, &opts(3)).unwrap();
        assert_eq!(frames, vec!["go-0", "go-1", "go-2"]);
    }

    #[cfg(unix)]
    #[test]
    fn program_custom_animation_splits_stdout_on_the_frame_separator() {
        let animation = TextAnimation::Custom(CustomAnimation::Program {
            path: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_string(),
                format!(
                    "printf 'one%sfour%sfive' '{sep}' '{sep}'",
                    sep = FRAME_SEPARATOR
                ),
            ],
        });
        let frames = text_frames("ignored", &animation, &opts(0)).unwrap();
        assert_eq!(frames, vec!["one", "four", "five"]);
    }

    #[cfg(unix)]
    #[test]
    fn program_custom_animation_surfaces_a_nonzero_exit_as_an_error() {
        let animation = TextAnimation::Custom(CustomAnimation::Program {
            path: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), "echo boom >&2; exit 3".to_string()],
        });
        let err = text_frames("x", &animation, &opts(0)).unwrap_err();
        assert!(matches!(err, EgerError::AnimationProgram(_, msg) if msg.contains("boom")));
    }

    #[test]
    fn play_text_rejects_non_positive_fps() {
        let mut bad = TextAnimOptions::default();
        bad.fps = 0.0;
        let err = play_text("hi", &TextAnimation::Typewriter, &bad).unwrap_err();
        assert!(matches!(err, EgerError::Animation(_)));
    }

    #[test]
    fn play_text_with_diff_mode_succeeds_without_panicking() {
        let opts = TextAnimOptions {
            frames: 3,
            fps: 1000.0, // keep the test fast
            diff: Some(crate::render::DiffGranularity::Line),
            ..Default::default()
        };
        let result = play_text("hi", &TextAnimation::Rainbow, &opts);
        assert!(result.is_ok());
    }

    #[cfg(all(unix, feature = "video"))]
    #[tokio::test]
    async fn stream_text_frames_delivers_frames_as_the_program_emits_them() {
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::channel(8);
        let args = vec![
            "-c".to_string(),
            format!(
                "printf 'one%stwo%sthree' '{sep}' '{sep}'",
                sep = FRAME_SEPARATOR
            ),
        ];
        let handle = tokio::spawn(stream_text_frames(
            "ignored".to_string(),
            PathBuf::from("/bin/sh"),
            args,
            0,
            tx,
        ));

        let mut received = Vec::new();
        while let Some(frame) = rx.recv().await {
            received.push(frame.unwrap());
        }
        handle.await.unwrap().unwrap();
        assert_eq!(received, vec!["one", "two", "three"]);
    }

    #[cfg(all(unix, feature = "video"))]
    #[tokio::test]
    async fn stream_text_frames_surfaces_a_nonzero_exit_as_an_error() {
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::channel(8);
        let args = vec!["-c".to_string(), "echo boom >&2; exit 3".to_string()];
        let handle = tokio::spawn(stream_text_frames(
            "x".to_string(),
            PathBuf::from("/bin/sh"),
            args,
            0,
            tx,
        ));

        let mut saw_error = false;
        while let Some(frame) = rx.recv().await {
            if let Err(EgerError::AnimationProgram(_, msg)) = frame {
                assert!(msg.contains("boom"));
                saw_error = true;
            }
        }
        handle.await.unwrap().unwrap();
        assert!(saw_error, "expected the nonzero exit to be reported");
    }

    #[test]
    fn hsv_to_rgb_primary_hues_are_exact() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), Rgb::new(0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), Rgb::new(0, 0, 255));
    }
}
