//! Benchmarks the video frame-conversion hot path via
//! `render_video_frames`/`stream_video_frames` against a short, tiny
//! synthetic clip generated on the fly with `ffmpeg`'s `color` source
//! filter (no input file/network access needed).
//!
//! Gated behind the `video` feature (it needs `tokio` and a real `ffmpeg`/
//! `ffprobe` on `PATH`) and skipped cleanly — with an explanatory message
//! instead of a failure — when `ffmpeg` isn't available, consistent with
//! how `video.rs`'s own tests handle a missing `ffmpeg`.
//!
//! Run with:
//! ```text
//! cargo bench --bench video_streaming --features video
//! ```

#[cfg(feature = "video")]
mod video_bench {
    use criterion::{black_box, BenchmarkId, Criterion};
    use eger::prelude::*;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;
    use std::sync::Arc;

    fn ffmpeg_available() -> bool {
        StdCommand::new("ffmpeg").arg("-version").output().is_ok()
    }

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
            .expect("failed to spawn ffmpeg to build bench fixture");
        assert!(status.success(), "ffmpeg failed to produce a bench video");
    }

    fn bench_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("eger-bench-video-{}-{name}", std::process::id()));
        path
    }

    pub fn run(c: &mut Criterion) {
        if !ffmpeg_available() {
            eprintln!(
                "skipping video_streaming benches: ffmpeg/ffprobe not found on PATH \
                 (install ffmpeg to run these)"
            );
            return;
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build a Tokio runtime for benchmarking");

        let mut group = c.benchmark_group("render_video_frames_by_frame_count");
        group.sample_size(10); // each iteration re-invokes ffmpeg + decodes; keep this cheap

        for &frame_count in &[8u32, 30] {
            let path = bench_path(&format!("{frame_count}.mp4"));
            make_test_video(&path, frame_count, 64, 64, 30);

            group.bench_with_input(
                BenchmarkId::from_parameter(frame_count),
                &path,
                |b, path| {
                    b.iter(|| {
                        let config = Arc::new(
                            Config::builder(MediaType::Video)
                                .from_file(path)
                                .max_width(80)
                                .build()
                                .expect("config should build"),
                        );
                        let frames = rt
                            .block_on(eger::video::render_video_frames(path, config, None))
                            .expect("video render should succeed");
                        black_box(frames);
                    });
                },
            );
            std::fs::remove_file(&path).ok();
        }
        group.finish();
    }
}

fn main() {
    let mut criterion = criterion::Criterion::default().configure_from_args();
    #[cfg(feature = "video")]
    video_bench::run(&mut criterion);
    #[cfg(not(feature = "video"))]
    {
        let _ = &mut criterion;
        eprintln!("video_streaming benches require `--features video`; skipping (nothing to run).");
    }
    criterion.final_summary();
}
