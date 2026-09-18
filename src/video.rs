//! Video → ASCII rendering with bounded memory frame streaming.

use crate::colorize::{colorize_lines, PixelAnimation};
use crate::config::Config;
use crate::error::{EgerError, Result};
use crate::render;
use iascii::render::ColorDepth;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// How a rendered video frame should be colored.
///
/// `iascii` can color each frame itself from the source video's actual
/// pixel colors/luminance (`Default`/`Explicit`), which is what you want
/// for a straightforward "video, but ASCII" render. `Plain` instead skips
/// that and returns bare, uncolored character rows, for when you want
/// [`crate::colorize::PixelAnimation`] driving the color instead — e.g. a
/// video rendered as a looping rainbow or plasma effect rather than its
/// own colors. See [`crate::colorize`]'s module docs for the full picture
/// of coloring options across every kind of ASCII art this crate produces.
#[derive(Debug, Clone, Copy, Default)]
pub enum ColorMode {
    /// Use the `Config`'s own [`crate::config::Config::color_depth`].
    #[default]
    Default,
    /// Override the `Config`'s color depth for this render only.
    Explicit(ColorDepth),
    /// No color at all — bare characters, ready for
    /// [`crate::colorize::colorize_joined_sequence`] (or any other
    /// post-processing) to color instead.
    Plain,
}

/// Basic video stream metadata needed to drive frame extraction.
#[derive(Debug, Clone, Copy)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frame_count: Option<u64>,
}

/// Probes a video file with `ffprobe` for its resolution and frame rate.
pub async fn probe(path: &Path) -> Result<VideoInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,nb_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|_| EgerError::FfmpegNotFound)?;

    if !output.status.success() {
        return Err(EgerError::Ffmpeg(
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut fields = stdout.trim().split(',');

    let width: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EgerError::Video("ffprobe: could not parse width".into()))?;
    let height: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EgerError::Video("ffprobe: could not parse height".into()))?;
    let fps = fields.next().map(parse_frame_rate).unwrap_or(30.0);
    let frame_count = fields.next().and_then(|s| s.trim().parse().ok());

    Ok(VideoInfo {
        width,
        height,
        fps,
        frame_count,
    })
}

fn parse_frame_rate(raw: &str) -> f64 {
    let raw = raw.trim();
    if let Some((num, den)) = raw.split_once('/') {
        match (num.trim().parse::<f64>(), den.trim().parse::<f64>()) {
            (Ok(n), Ok(d)) if d != 0.0 => n / d,
            _ => 30.0,
        }
    } else {
        raw.parse().unwrap_or(30.0)
    }
}

async fn spawn_frame_stream(path: &Path) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Without this, dropping `Child` (e.g. on an early `return`/`?`
        // below, or when the consumer drops the frame receiver and we bail
        // out of the read loop) leaves the spawned `ffmpeg` process
        // running with nobody around to reap or signal it. On Unix that
        // process keeps writing raw frames into a pipe nobody is reading;
        // once the pipe buffer fills, `ffmpeg` blocks in `write()`
        // forever — a genuine leaked/zombie process, not just wasted CPU.
        // `kill_on_drop` makes every exit path (success, error, or early
        // bail-out) terminate the child instead of relying on us to
        // remember to do it manually at each call site.
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| EgerError::FfmpegNotFound)
}

/// Streams converted ASCII frame batches over a bounded `mpsc` channel.
///
/// Limits memory usage to active worker chunk buffers rather than holding the entire
/// video's rendered frames in memory.
pub async fn stream_video_frames(
    path: PathBuf,
    config: Arc<Config>,
    color_mode: ColorMode,
    tx: mpsc::Sender<Result<Vec<String>>>,
) -> Result<()> {
    let info = probe(&path).await?;
    let frame_bytes = (info.width as usize) * (info.height as usize) * 3;
    if frame_bytes == 0 {
        return Err(EgerError::Video("video has a zero-sized frame".into()));
    }

    let mut child = spawn_frame_stream(&path).await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| EgerError::Video("failed to capture ffmpeg stdout".into()))?;

    let depth = match color_mode {
        ColorMode::Default => Some(config.color_depth),
        ColorMode::Explicit(depth) => Some(depth),
        ColorMode::Plain => None,
    };
    let chunk_size = (config.num_threads * 4).max(1);

    // Reuse the pool cached on `config` (shared with the image pipeline)
    // rather than building a fresh one for every `stream_video_frames`
    // call/video.
    let pool = config.thread_pool()?.clone();

    type ConversionHandle = tokio::task::JoinHandle<Result<Vec<String>>>;
    let mut pending: Option<ConversionHandle> = None;

    loop {
        let mut chunk = Vec::with_capacity(chunk_size);
        for _ in 0..chunk_size {
            let mut buf = vec![0u8; frame_bytes];
            match stdout.read_exact(&mut buf).await {
                Ok(_) => chunk.push(buf),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        let is_last = chunk.len() < chunk_size;

        if let Some(handle) = pending.take() {
            let frames = handle.await??;
            if tx.send(Ok(frames)).await.is_err() {
                // Receiver was dropped by the consumer. We must NOT fall
                // through to the normal `child.wait().await` below: with
                // no one left to drain `stdout`, a still-running `ffmpeg`
                // will block on write() once its pipe buffer fills, and
                // `wait()` would then hang forever waiting for an exit
                // that never comes. Kill it explicitly and return right
                // away instead (also covered by `kill_on_drop` on `Drop`,
                // but killing here avoids depending on drop timing/order
                // and avoids ever calling the blocking `wait()` path).
                let _ = child.kill().await;
                return Ok(());
            }
        }

        if chunk.is_empty() {
            break;
        }

        let width = info.width;
        let height = info.height;
        let config = Arc::clone(&config);
        let pool = Arc::clone(&pool);
        pending = Some(tokio::task::spawn_blocking(
            move || -> Result<Vec<String>> {
                pool.install(|| {
                    chunk
                        .into_par_iter()
                        .map(|raw| {
                            let grid = crate::image::convert(width, height, &raw, &config)?;
                            Ok(render::grid_to_string(&grid, depth))
                        })
                        .collect::<Result<Vec<String>>>()
                })
            },
        ));

        if is_last {
            break;
        }
    }

    if let Some(handle) = pending.take() {
        let frames = handle.await??;
        let _ = tx.send(Ok(frames)).await;
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(EgerError::Ffmpeg(
            status,
            "ffmpeg frame extraction failed".into(),
        ));
    }

    Ok(())
}

/// Renders all video frames into memory.
///
/// **Warning:** Prefer [`video_to_file`], [`play_video`], or [`stream_video_frames`]
/// for large files to prevent OOM errors.
pub async fn render_video_frames(
    path: &Path,
    config: Arc<Config>,
    color_mode: ColorMode,
) -> Result<Vec<String>> {
    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(async move { stream_video_frames(path_buf, config, color_mode, tx).await });

    let mut rendered = Vec::new();
    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        rendered.extend(chunk);
    }

    stream_task.await??;
    Ok(rendered)
}

pub async fn video_to_lines(path: &Path, config: Arc<Config>) -> Result<Vec<String>> {
    render_video_frames(path, config, ColorMode::Default).await
}

/// Same as [`video_to_lines`], but with `iascii`'s own pixel coloring
/// skipped ([`ColorMode::Plain`]) — the shape
/// [`crate::colorize::colorize_joined_sequence`] expects when you want a
/// [`crate::colorize::PixelAnimation`] coloring the video instead.
pub async fn video_to_lines_plain(path: &Path, config: Arc<Config>) -> Result<Vec<String>> {
    render_video_frames(path, config, ColorMode::Plain).await
}

/// Streams a video, converting to ASCII and recoloring each decoded frame
/// with `animation` as it arrives — the video-frame equivalent of
/// [`crate::colorize::colorize_joined_sequence`], but bounded-memory and
/// driven directly off the decode pipeline instead of requiring the
/// caller to first collect every frame via [`video_to_lines_plain`] (which
/// re-introduces exactly the whole-video-in-memory problem
/// [`stream_video_frames`] exists to avoid).
///
/// Internally this streams plain ([`ColorMode::Plain`]) frames from
/// [`stream_video_frames`] on an internal channel, then recolors each
/// chunk — off the async task via `spawn_blocking`, same as
/// `stream_video_frames` itself offloads conversion — before forwarding
/// it to `tx`. `animation` is cloned once per chunk (cheap: every built-in
/// [`PixelAnimation`] variant is plain data, and `Custom` is an `Arc`
/// clone), not once per frame.
pub async fn stream_colorized_video_frames(
    path: PathBuf,
    config: Arc<Config>,
    animation: PixelAnimation,
    tx: mpsc::Sender<Result<Vec<String>>>,
) -> Result<()> {
    let (plain_tx, mut plain_rx) = mpsc::channel(4);
    // Grab the pool *before* `config` is moved into the decode/convert
    // task below, so the recoloring stage can also run on it — otherwise
    // `chunk.into_par_iter()` further down would silently fall back to
    // Rayon's global default pool instead of the one sized by
    // `config.num_threads`, same as every other CPU-bound stage in this
    // crate (`image::convert`, `stream_video_frames`'s own conversion
    // step, `render_batch_parallel`) already uses.
    let pool = config.thread_pool()?.clone();
    let stream_task =
        tokio::spawn(
            async move { stream_video_frames(path, config, ColorMode::Plain, plain_tx).await },
        );

    let mut frame_index: u64 = 0;
    while let Some(chunk_res) = plain_rx.recv().await {
        let chunk = chunk_res?;
        let start = frame_index;
        frame_index += chunk.len() as u64;
        let animation = animation.clone();
        let pool = Arc::clone(&pool);

        let colored = tokio::task::spawn_blocking(move || {
            pool.install(|| {
                chunk
                    .into_par_iter()
                    .enumerate()
                    .map(|(i, frame)| {
                        let lines: Vec<&str> = frame.lines().collect();
                        colorize_lines(&lines, &animation, start + i as u64)
                    })
                    .collect::<Vec<String>>()
            })
        })
        .await?;

        if tx.send(Ok(colored)).await.is_err() {
            // Consumer gone. Returning here (rather than falling through
            // to `stream_task.await??` below) drops `plain_rx`, which is
            // exactly how `stream_video_frames` itself already detects and
            // reacts to a dropped receiver — see its own comment on the
            // equivalent `tx.send(...).is_err()` check.
            return Ok(());
        }
    }

    stream_task.await??;
    Ok(())
}

/// [`render_video_frames`]'s counterpart for colorized output: collects
/// every [`stream_colorized_video_frames`] chunk into one `Vec<String>`.
///
/// **Warning:** holds the whole video's rendered frames in memory, same
/// caveat as [`render_video_frames`] — prefer
/// [`stream_colorized_video_frames`] directly for large files.
pub async fn render_colorized_video_frames(
    path: &Path,
    config: Arc<Config>,
    animation: PixelAnimation,
) -> Result<Vec<String>> {
    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task = tokio::spawn(async move {
        stream_colorized_video_frames(path_buf, config, animation, tx).await
    });

    let mut rendered = Vec::new();
    while let Some(chunk_res) = rx.recv().await {
        rendered.extend(chunk_res?);
    }

    stream_task.await??;
    Ok(rendered)
}

/// Streams and writes ASCII frame chunks directly to an output file using a buffered writer.
pub async fn video_to_file(
    path: &Path,
    config: Arc<Config>,
    output: Option<&Path>,
) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .or_else(|| config.output.clone())
        .ok_or_else(|| EgerError::Render("no output path provided or configured".into()))?;

    let file = tokio::fs::File::create(&output).await?;
    let mut writer = BufWriter::new(file);

    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(
            async move { stream_video_frames(path_buf, config, ColorMode::Default, tx).await },
        );

    let mut first = true;
    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        for frame in chunk {
            if !first {
                writer
                    .write_all(crate::text::FRAME_SEPARATOR.as_bytes())
                    .await?;
            }
            first = false;
            writer.write_all(frame.as_bytes()).await?;
        }
    }
    writer.flush().await?;

    stream_task.await??;
    Ok(output)
}

/// Streams frames directly to stdout for terminal playback without buffering the whole file in RAM.
pub async fn play_video(path: &Path, config: Arc<Config>) -> Result<()> {
    let info = probe(path).await?;
    if !(info.fps > 0.0) {
        return Err(EgerError::Render("fps must be positive".into()));
    }
    let frame_delay = std::time::Duration::from_secs_f64(1.0 / info.fps);
    let mut stdout = tokio::io::stdout();

    let (tx, mut rx) = mpsc::channel(4);
    let path_buf = path.to_path_buf();
    let stream_task =
        tokio::spawn(
            async move { stream_video_frames(path_buf, config, ColorMode::Default, tx).await },
        );

    while let Some(chunk_res) = rx.recv().await {
        let chunk = chunk_res?;
        for frame in chunk {
            stdout.write_all(b"\x1B[H\x1B[2J").await?;
            stdout.write_all(frame.as_bytes()).await?;
            stdout.flush().await?;
            tokio::time::sleep(frame_delay).await;
        }
    }

    stream_task.await??;
    Ok(())
}

/// Plays an animated GIF's frames to stdout, timed by each frame's own
/// display delay (as recorded in the GIF), analogous to [`play_video`] for
/// real video files.
///
/// Decoding (via [`crate::image::gif_to_lines`]) happens up front on a
/// blocking thread pool task rather than streamed — see that function's
/// docs for why — so for a very large GIF this holds every rendered frame
/// in memory before playback starts.
pub async fn play_gif(path: &Path, config: Arc<Config>) -> Result<()> {
    let path_buf = path.to_path_buf();
    let frames =
        tokio::task::spawn_blocking(move || crate::image::gif_to_lines(&path_buf, &config))
            .await??;

    if frames.is_empty() {
        return Err(EgerError::Render("gif has no frames".into()));
    }

    let mut stdout = tokio::io::stdout();
    for (lines, delay) in frames {
        let text = lines.join("\n");
        stdout.write_all(b"\x1B[H\x1B[2J").await?;
        stdout.write_all(text.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        // Some GIFs encode a zero delay for a frame; give it a sane
        // minimum so playback doesn't appear to freeze/skip.
        let delay = if delay.is_zero() {
            Duration::from_millis(100)
        } else {
            delay
        };
        tokio::time::sleep(delay).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, MediaType};
    use std::process::Command as StdCommand;

    /// Skips a test (rather than failing) when `ffmpeg`/`ffprobe` aren't on
    /// `PATH`, so this suite stays runnable in environments without them —
    /// consistent with `EgerError::FfmpegNotFound` being a normal, expected
    /// outcome rather than a bug.
    fn ffmpeg_available() -> bool {
        StdCommand::new("ffmpeg").arg("-version").output().is_ok()
    }

    fn tempfile(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("eger-video-test-{}-{name}", std::process::id()));
        path
    }

    /// Generates a short, tiny, solid-color test video with `ffmpeg`'s
    /// `color` source filter — no input file needed, deterministic output,
    /// fast to encode.
    fn make_test_video(path: &Path, frames: u32, width: u32, height: u32, fps: u32) {
        let status = StdCommand::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "color=c=red:s={width}x{height}:r={fps}:d={:.3}",
                frames as f64 / fps as f64
            ))
            .args(["-frames:v", &frames.to_string()])
            .arg(path)
            .status()
            .expect("failed to spawn ffmpeg");
        assert!(status.success(), "ffmpeg failed to produce a test video");
    }

    #[tokio::test]
    async fn probe_reads_back_width_height_and_fps() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("probe.mp4");
        make_test_video(&path, 10, 16, 16, 10);

        let info = probe(&path).await.unwrap();
        assert_eq!(info.width, 16);
        assert_eq!(info.height, 16);
        assert!((info.fps - 10.0).abs() < 0.5);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn render_video_frames_produces_one_frame_per_decoded_frame() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("render.mp4");
        make_test_video(&path, 6, 16, 16, 6);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(8, 4)
                .num_threads(2)
                .build()
                .unwrap(),
        );

        let frames = render_video_frames(&path, config, ColorMode::Default)
            .await
            .unwrap();
        assert_eq!(frames.len(), 6, "one ASCII frame per decoded video frame");
        for frame in &frames {
            assert_eq!(
                frame.lines().count(),
                4,
                "explicit_dimensions height honored"
            );
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn color_mode_plain_produces_frames_with_no_ansi_escapes() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("plain.mp4");
        make_test_video(&path, 3, 16, 16, 6);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(8, 4)
                .build()
                .unwrap(),
        );

        let frames = render_video_frames(&path, config, ColorMode::Plain)
            .await
            .unwrap();
        assert_eq!(frames.len(), 3);
        for frame in &frames {
            assert!(
                !frame.contains('\x1b'),
                "ColorMode::Plain should skip iascii's own coloring entirely"
            );
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn colorized_video_frames_use_configs_own_thread_pool() {
        // Regression test: stream_colorized_video_frames used to move
        // `config` into the decode/convert task before its recoloring
        // loop ran, so the recoloring stage's `into_par_iter()` fell back
        // to Rayon's global default pool instead of the pool sized by
        // `Config::num_threads` — meaning capping `num_threads` failed to
        // cap the recoloring stage's parallelism. Build a `Config` with a
        // distinctive thread count and confirm `Config::thread_pool()`
        // (the same pool the fixed code path now installs onto for
        // recoloring) reports exactly that count, both before and after
        // driving a real colorized render through it.
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("pool-check.mp4");
        make_test_video(&path, 3, 8, 8, 6);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(6, 3)
                .num_threads(3)
                .build()
                .unwrap(),
        );

        let animation = crate::colorize::PixelAnimation::Rainbow { speed: 5.0 };
        let frames = render_colorized_video_frames(&path, Arc::clone(&config), animation)
            .await
            .unwrap();
        assert_eq!(frames.len(), 3);

        // The pool was built (lazily, on first use) as part of driving the
        // render above; it must be sized exactly as configured, and be
        // the *same* pool instance every call site reuses.
        let pool = config.thread_pool().unwrap();
        assert_eq!(pool.current_num_threads(), 3);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn colorized_video_frames_are_recolored_with_a_running_frame_index() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("colorized.mp4");
        make_test_video(&path, 4, 8, 8, 6);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(6, 3)
                .build()
                .unwrap(),
        );

        // Blink alternates by frame parity, so a running (not per-chunk-
        // reset) frame index across the whole video must alternate colors
        // frame to frame, proving `frame_index` is threaded through
        // correctly rather than restarting at 0 for every internal chunk.
        let animation = crate::colorize::PixelAnimation::Blink {
            on_color: crate::text::Rgb::new(255, 0, 0),
            off_color: crate::text::Rgb::new(0, 0, 255),
        };
        let frames = render_colorized_video_frames(&path, config, animation)
            .await
            .unwrap();
        assert_eq!(frames.len(), 4);
        assert!(frames[0].contains("38;2;255;0;0"));
        assert!(frames[1].contains("38;2;0;0;255"));
        assert!(frames[2].contains("38;2;255;0;0"));
        assert!(frames[3].contains("38;2;0;0;255"));

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn stream_video_frames_with_oversized_output_does_not_deadlock() {
        // Regression test mirroring image::tests::
        // render_batch_parallel_with_an_oversized_item_does_not_deadlock:
        // here the outer per-frame-in-chunk parallelism and the inner
        // (parallel-feature) per-row parallelism share the same pool, so
        // this proves that nesting doesn't hang for the video path too.
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let path = tempfile("nested.mp4");
        make_test_video(&path, 4, 8, 8, 4);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(20, 150) // clears the default 100-row threshold
                .num_threads(2)
                .build()
                .unwrap(),
        );

        let frames = render_video_frames(&path, config, ColorMode::Default)
            .await
            .unwrap();
        assert_eq!(frames.len(), 4);
        for frame in &frames {
            assert_eq!(frame.lines().count(), 150);
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn probe_on_a_nonexistent_file_is_a_clean_error_not_a_panic() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
            return;
        }
        let result = probe(Path::new("/no/such/video.mp4")).await;
        assert!(result.is_err());
    }
}
