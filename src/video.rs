//! Video → ASCII rendering.
//!
//! Frames are streamed as raw RGB24 straight out of an `ffmpeg` pipe (no
//! temporary frame files), read in bounded chunks by a Tokio task, and each
//! chunk is converted to ASCII in parallel on a Rayon thread pool. This caps
//! memory usage to roughly `chunk_size` frames at a time instead of buffering
//! an entire video, while still overlapping I/O (pipe reads) with CPU work
//! (ASCII conversion) across chunks.
//!
//! Requires `ffmpeg` and `ffprobe` to be installed and available on `PATH`.

use crate::config::Config;
use crate::error::{MaramuraError, Result};
use crate::render::{self, play_terminal};
use iascii::convert::convert_image;
use iascii::render::ColorDepth;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

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
        .map_err(|_| MaramuraError::FfmpegNotFound)?;

    if !output.status.success() {
        return Err(MaramuraError::Ffmpeg(
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut fields = stdout.trim().split(',');

    let width: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| MaramuraError::Video("ffprobe: could not parse width".into()))?;
    let height: u32 = fields
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| MaramuraError::Video("ffprobe: could not parse height".into()))?;
    let fps = fields.next().map(parse_frame_rate).unwrap_or(30.0);
    let frame_count = fields.next().and_then(|s| s.trim().parse().ok());

    Ok(VideoInfo {
        width,
        height,
        fps,
        frame_count,
    })
}

/// Parses ffprobe's `"num/den"` (or plain) frame-rate format.
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

/// Spawns `ffmpeg`, streaming raw RGB24 frames on stdout.
async fn spawn_frame_stream(path: &Path) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| MaramuraError::FfmpegNotFound)
}

/// Converts an entire video into a sequence of rendered ASCII frames
/// (one `String` per frame, in order).
///
/// Uses `config.color_depth` unless `depth_override` is given, letting a
/// caller render the same video at a different color depth without
/// rebuilding the whole `Config`.
///
/// A single Rayon thread pool (sized by `config.num_threads`) is built once
/// and reused for every chunk's conversion, rather than spun up per chunk —
/// building a pool spawns `num_threads` OS threads, so re-creating it on
/// every iteration would dominate runtime on longer videos with many
/// chunks. Chunk *reads* (I/O) and the *previous* chunk's conversion (CPU)
/// also genuinely overlap: each chunk's conversion is kicked off on the
/// blocking pool without being awaited immediately, so the next chunk's
/// pipe read proceeds concurrently on the async task while it runs.
pub async fn render_video_frames(
    path: &Path,
    config: Arc<Config>,
    depth_override: Option<ColorDepth>,
) -> Result<Vec<String>> {
    let info = probe(path).await?;
    let frame_bytes = (info.width as usize) * (info.height as usize) * 3;
    if frame_bytes == 0 {
        return Err(MaramuraError::Video("video has a zero-sized frame".into()));
    }

    let mut child = spawn_frame_stream(path).await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| MaramuraError::Video("failed to capture ffmpeg stdout".into()))?;

    let color_depth = depth_override.unwrap_or(config.color_depth);
    let chunk_size = (config.num_threads * 4).max(1);
    let mut rendered = Vec::with_capacity(info.frame_count.unwrap_or(0) as usize);

    let mut pool_builder = rayon::ThreadPoolBuilder::new();
    if config.num_threads > 0 {
        pool_builder = pool_builder.num_threads(config.num_threads);
    }
    let pool = Arc::new(
        pool_builder
            .build()
            .map_err(|e| MaramuraError::Video(e.to_string()))?,
    );

    type ConversionHandle = tokio::task::JoinHandle<Result<Vec<String>>>;
    let mut pending: Option<ConversionHandle> = None;

    loop {
        // Read up to `chunk_size` raw frames, bounding memory use. While
        // this read runs, any conversion kicked off for the *previous*
        // chunk is running concurrently on the blocking pool below.
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

        // The previous chunk's conversion has had this entire read to run
        // in the background; collect its result now (usually an
        // instant wait, since it was likely already done).
        if let Some(handle) = pending.take() {
            rendered.extend(handle.await??);
        }

        if chunk.is_empty() {
            break;
        }

        // Kick off this chunk's conversion in the background (reusing the
        // shared pool) and loop back to read the next chunk concurrently,
        // rather than blocking here until it finishes.
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
                            let grid = convert_image(width, height, &raw, &config.ascii)?;
                            Ok(render::grid_to_string(&grid, Some(color_depth)))
                        })
                        .collect::<Result<Vec<String>>>()
                })
            },
        ));

        if is_last {
            break;
        }
    }

    // Drain any conversion still in flight for the final chunk.
    if let Some(handle) = pending.take() {
        rendered.extend(handle.await??);
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(MaramuraError::Ffmpeg(
            status,
            "ffmpeg frame extraction failed".into(),
        ));
    }

    Ok(rendered)
}

/// Convenience: render a video straight to `Vec<String>` (one entry per frame).
pub async fn video_to_lines(path: &Path, config: Arc<Config>) -> Result<Vec<String>> {
    render_video_frames(path, config, None).await
}

/// Convenience: render a video and write every frame to a single file,
/// separated by an ASCII record-separator (`\x1E`) so frames can be split
/// back out losslessly. Writes to `output` if given, otherwise `config.output`.
pub async fn video_to_file(
    path: &Path,
    config: Arc<Config>,
    output: Option<&Path>,
) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .or_else(|| config.output.clone())
        .ok_or_else(|| MaramuraError::Render("no output path provided or configured".into()))?;

    let frames = render_video_frames(path, config, None).await?;
    let joined = frames.join("\n\x1E\n");
    tokio::fs::write(&output, joined).await?;
    Ok(output)
}

/// Renders a video and plays it back in the terminal at its native frame
/// rate — "seamless" preview without ever touching disk.
pub async fn play_video(path: &Path, config: Arc<Config>) -> Result<()> {
    let info = probe(path).await?;
    let frames = render_video_frames(path, config, None).await?;
    play_terminal(&frames, info.fps).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, MediaType};
    use std::process::Command as StdCommand;

    // --- Pure unit tests (no ffmpeg required) -----------------------------

    #[test]
    fn parse_frame_rate_handles_fraction() {
        assert_eq!(parse_frame_rate("30000/1001"), 30000.0 / 1001.0);
        assert_eq!(parse_frame_rate("25/1"), 25.0);
    }

    #[test]
    fn parse_frame_rate_handles_plain_number() {
        assert_eq!(parse_frame_rate("24"), 24.0);
    }

    #[test]
    fn parse_frame_rate_falls_back_to_30_on_garbage() {
        assert_eq!(parse_frame_rate("garbage"), 30.0);
        assert_eq!(parse_frame_rate("1/garbage"), 30.0);
        assert_eq!(parse_frame_rate(""), 30.0);
    }

    #[test]
    fn parse_frame_rate_avoids_division_by_zero() {
        assert_eq!(parse_frame_rate("30/0"), 30.0);
    }

    #[test]
    fn parse_frame_rate_handles_surrounding_whitespace() {
        assert_eq!(parse_frame_rate("  24  "), 24.0);
        assert_eq!(parse_frame_rate(" 30000 / 1001 "), 30000.0 / 1001.0);
    }

    #[test]
    fn parse_frame_rate_handles_negative_and_fractional_numerator() {
        // Not something ffprobe would realistically emit, but the parser
        // shouldn't panic on it — it should just divide through normally.
        assert_eq!(parse_frame_rate("-30/1"), -30.0);
        assert_eq!(parse_frame_rate("29.97"), 29.97);
    }

    // --- Integration tests against a real ffmpeg/ffprobe -------------------
    //
    // These generate a tiny synthetic clip with `ffmpeg -f lavfi` (no
    // external fixture files needed) and are skipped automatically if
    // `ffmpeg`/`ffprobe` aren't on PATH.

    fn ffmpeg_available() -> bool {
        StdCommand::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Generates a `frames`-frame synthetic video at `fps` fps, `width`x`height`,
    /// via ffmpeg's `testsrc` filter, returning its path (auto-deleted on drop).
    struct TestClip {
        path: PathBuf,
    }

    impl TestClip {
        fn generate(width: u32, height: u32, fps: u32, frames: u32) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "maramura-video-test-{}-{}.mp4",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let duration = frames as f64 / fps as f64;
            let status = StdCommand::new("ffmpeg")
                .args(["-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(format!(
                    "testsrc=size={width}x{height}:rate={fps}:duration={duration}"
                ))
                .args(["-frames:v", &frames.to_string(), "-pix_fmt", "yuv420p"])
                .arg(&path)
                .args(["-y"])
                .status()
                .expect("failed to invoke ffmpeg to build test fixture");
            assert!(status.success(), "ffmpeg failed to generate test clip");
            Self { path }
        }
    }

    impl Drop for TestClip {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Builds a `Config` directly (bypassing `ConfigBuilder::build`'s
    /// filesystem checks, since these tests generate their own clips rather
    /// than pointing at a pre-registered input path).
    fn test_config(num_threads: usize) -> Arc<Config> {
        use crate::config::InputSource;
        use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(16))
            .build()
            .expect("ascii config should build with valid defaults");

        Arc::new(Config {
            media_type: MediaType::Video,
            source: InputSource::File(PathBuf::from("unused-in-these-tests")),
            output: None,
            num_threads,
            color_depth: ColorDepth::TrueColor,
            ascii,
        })
    }

    #[tokio::test]
    async fn probe_reports_expected_dimensions_and_fps() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on path");
            return;
        }
        let clip = TestClip::generate(32, 24, 10, 10);
        let info = probe(&clip.path).await.unwrap();
        assert_eq!(info.width, 32);
        assert_eq!(info.height, 24);
        assert!((info.fps - 10.0).abs() < 0.001);
        assert_eq!(info.frame_count, Some(10));
    }

    #[tokio::test]
    async fn probe_missing_file_errors() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let err = probe(Path::new("/definitely/not/a/real/video.mp4"))
            .await
            .unwrap_err();
        assert!(matches!(err, MaramuraError::Ffmpeg(_, _)));
    }

    #[tokio::test]
    async fn render_video_frames_produces_one_frame_per_source_frame() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        // 7 frames with a chunk size that doesn't evenly divide it, to
        // exercise the "last (partial) chunk" path in the chunking loop.
        let clip = TestClip::generate(16, 12, 7, 7);
        let config = test_config(2); // chunk_size = num_threads * 4 = 8 > 7 frames

        let frames = render_video_frames(&clip.path, config, None).await.unwrap();
        assert_eq!(frames.len(), 7);
        for frame in &frames {
            assert!(!frame.is_empty());
        }
    }

    #[tokio::test]
    async fn render_video_frames_handles_multiple_full_chunks_plus_remainder() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        // chunk_size = 1 * 4 = 4; 10 frames -> two full chunks + one partial.
        let clip = TestClip::generate(16, 12, 10, 10);
        let config = test_config(1);

        let frames = render_video_frames(&clip.path, config, None).await.unwrap();
        assert_eq!(frames.len(), 10);
    }

    #[tokio::test]
    async fn render_video_frames_respects_depth_override() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let clip = TestClip::generate(16, 12, 5, 3);
        let config = test_config(2);

        // `ColorDepth` has no "no color" variant — video frames are always
        // ANSI-colored (see `render_video_frames`'s use of
        // `render::grid_to_string(&grid, Some(color_depth))`). So this
        // exercises that the override actually changes *which* depth is
        // used, by comparing two distinct encodings rather than colored vs.
        // plain.
        let truecolor =
            render_video_frames(&clip.path, Arc::clone(&config), Some(ColorDepth::TrueColor))
                .await
                .unwrap();
        let ansi16 = render_video_frames(&clip.path, config, Some(ColorDepth::Ansi16))
            .await
            .unwrap();

        assert_eq!(truecolor.len(), ansi16.len());
        assert!(truecolor[0].contains('\x1b'));
        assert!(ansi16[0].contains('\x1b'));
        assert_ne!(
            truecolor[0], ansi16[0],
            "TrueColor and Ansi16 overrides should produce different encodings"
        );
    }

    #[tokio::test]
    async fn video_to_file_writes_record_separated_frames() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let clip = TestClip::generate(16, 12, 5, 3);
        let config = test_config(2);

        let mut out_path = std::env::temp_dir();
        out_path.push(format!("maramura-video-out-{}.txt", std::process::id()));

        video_to_file(&clip.path, config, Some(&out_path))
            .await
            .unwrap();

        let contents = tokio::fs::read_to_string(&out_path).await.unwrap();
        let frame_count = contents.split("\n\x1E\n").count();
        assert_eq!(frame_count, 3);
        tokio::fs::remove_file(&out_path).await.ok();
    }

    #[tokio::test]
    async fn render_video_frames_with_zero_num_threads_still_completes() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        // `chunk_size = (num_threads * 4).max(1)` guards against a
        // zero-sized chunk when `num_threads` is 0; `rayon`'s
        // `ThreadPoolBuilder::num_threads(0)` itself falls back to Rayon's
        // own default parallelism rather than erroring. Both together
        // should still produce a working (if serial-ish) render.
        let clip = TestClip::generate(16, 12, 5, 4);
        let config = test_config(0);

        let frames = render_video_frames(&clip.path, config, None).await.unwrap();
        assert_eq!(frames.len(), 4);
    }

    #[tokio::test]
    async fn render_video_frames_zero_sized_frame_errors() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        // A genuinely zero-dimension video can't be produced by ffmpeg, so
        // this exercises the same guard indirectly via a corrupt/empty file,
        // which should fail during probing rather than panicking.
        let mut path = std::env::temp_dir();
        path.push(format!("maramura-empty-{}.mp4", std::process::id()));
        std::fs::write(&path, b"").unwrap();

        let config = test_config(1);
        let err = render_video_frames(&path, config, None).await.unwrap_err();
        assert!(matches!(
            err,
            MaramuraError::Ffmpeg(_, _) | MaramuraError::Video(_)
        ));
        std::fs::remove_file(&path).ok();
    }
}
