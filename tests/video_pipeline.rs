//! Black-box integration tests for the video → ASCII pipeline: only
//! `eger`'s public API is exercised. Requires the `video` feature to be
//! enabled (this whole file is a no-op otherwise) and `ffmpeg`/`ffprobe` on
//! `PATH`; each test skips itself at runtime (rather than failing) when
//! ffmpeg is absent. Run with `cargo test --features video`.
#![cfg(feature = "video")]

use eger::prelude::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A synthetic test clip generated via ffmpeg's `lavfi testsrc`, deleted on drop.
struct TestClip(PathBuf);

impl TestClip {
    fn generate(width: u32, height: u32, fps: u32, frames: u32) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "eger-it-video-{}-{}.mp4",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let duration = frames as f64 / fps as f64;
        let status = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc=size={width}x{height}:rate={fps}:duration={duration}"
            ))
            .args(["-frames:v", &frames.to_string(), "-pix_fmt", "yuv420p"])
            .arg(&path)
            .arg("-y")
            .status()
            .expect("failed to invoke ffmpeg");
        assert!(status.success(), "ffmpeg failed to build fixture clip");
        Self(path)
    }
}

impl Drop for TestClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

macro_rules! skip_if_no_ffmpeg {
    () => {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg/ffprobe not found on PATH");
            return;
        }
    };
}

#[tokio::test]
async fn end_to_end_video_to_lines_via_public_builder() {
    skip_if_no_ffmpeg!();
    let clip = TestClip::generate(32, 24, 8, 8);

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(16)
            .num_threads(2)
            .build()
            .expect("config should build for a real clip"),
    );

    let frames = eger::video_to_lines(&clip.0, config)
        .await
        .expect("rendering should succeed");
    assert_eq!(frames.len(), 8);
    assert!(frames.iter().all(|f| !f.is_empty()));
}

#[tokio::test]
async fn end_to_end_video_to_file_round_trip() {
    skip_if_no_ffmpeg!();
    let clip = TestClip::generate(32, 24, 6, 6);
    let mut out = std::env::temp_dir();
    out.push(format!("eger-it-video-out-{}.txt", std::process::id()));

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(16)
            .output(&out)
            .build()
            .unwrap(),
    );

    let written = eger::video_to_file(&clip.0, config, None).await.unwrap();
    assert_eq!(written, out);

    let contents = tokio::fs::read_to_string(&out).await.unwrap();
    assert_eq!(contents.split("\n\x1E\n").count(), 6);

    tokio::fs::remove_file(&out).await.ok();
}

#[tokio::test]
async fn probe_then_render_are_consistent_about_frame_count() {
    skip_if_no_ffmpeg!();
    let clip = TestClip::generate(16, 16, 5, 12);

    let info = eger::probe(&clip.0).await.unwrap();
    assert_eq!(info.frame_count, Some(12));

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(8)
            .build()
            .unwrap(),
    );
    let frames = eger::video_to_lines(&clip.0, config).await.unwrap();
    assert_eq!(frames.len(), info.frame_count.unwrap() as usize);
}

/// Stress test: a longer clip with a small thread count (so the chunking
/// loop in `render_video_frames` runs many chunk iterations, including
/// several full chunks and a final partial one) — verifying frame count,
/// frame ordering is preserved is implicit (frames are pushed in read order
/// within a chunk, and chunks themselves are processed and appended in
/// order), and nothing panics or deadlocks under the smaller pool.
#[tokio::test]
async fn stress_many_frames_through_a_small_thread_pool() {
    skip_if_no_ffmpeg!();
    // chunk_size = num_threads * 4 = 4, so a 37-frame clip forces 9 full
    // chunks plus a 1-frame remainder.
    let clip = TestClip::generate(24, 16, 30, 37);

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(12)
            .num_threads(1)
            .build()
            .unwrap(),
    );

    let frames = eger::video_to_lines(&clip.0, config).await.unwrap();
    assert_eq!(frames.len(), 37);
    assert!(frames.iter().all(|f| !f.is_empty()));
}

#[tokio::test]
async fn probing_a_nonexistent_file_errors_cleanly() {
    skip_if_no_ffmpeg!();
    let err = eger::probe(std::path::Path::new("/no/such/video.mp4"))
        .await
        .unwrap_err();
    assert!(matches!(err, EgerError::Ffmpeg(_, _)));
}

#[tokio::test]
async fn play_video_completes_without_error_on_a_short_clip() {
    skip_if_no_ffmpeg!();
    // `play_video` writes straight to the terminal; there's no return value
    // to inspect, so this just confirms the full probe -> render -> play
    // pipeline runs end-to-end without erroring on a short, fast clip.
    let clip = TestClip::generate(16, 12, 20, 4);

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(8)
            .build()
            .unwrap(),
    );

    eger::play_video(&clip.0, config).await.unwrap();
}

#[tokio::test]
async fn video_to_file_output_setter_is_used_when_no_explicit_path_given() {
    skip_if_no_ffmpeg!();
    let clip = TestClip::generate(16, 12, 6, 3);
    let mut out = std::env::temp_dir();
    out.push(format!(
        "eger-it-video-config-output-{}.txt",
        std::process::id()
    ));

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&clip.0)
            .max_width(8)
            .output(&out) // set via the builder, not passed explicitly below
            .build()
            .unwrap(),
    );

    let written = eger::video_to_file(&clip.0, config, None).await.unwrap();
    assert_eq!(written, out);
    assert!(tokio::fs::metadata(&out).await.is_ok());

    tokio::fs::remove_file(&out).await.ok();
}

#[tokio::test]
async fn rendering_a_corrupt_file_errors_instead_of_panicking() {
    skip_if_no_ffmpeg!();
    let mut path = std::env::temp_dir();
    path.push(format!("eger-it-corrupt-{}.mp4", std::process::id()));
    std::fs::write(&path, b"this is not a real video file at all").unwrap();

    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file(&path)
            .max_width(8)
            .build()
            .unwrap(),
    );

    let err = eger::video_to_lines(&path, config).await.unwrap_err();
    assert!(matches!(err, EgerError::Ffmpeg(_, _) | EgerError::Video(_)));

    std::fs::remove_file(&path).ok();
}
