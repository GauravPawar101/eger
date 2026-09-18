//! Benchmarks `render_batch_parallel` (the always-on, across-files Rayon
//! path) at a few batch sizes and thread-pool widths, to characterize how
//! much a batch job benefits from more workers versus how much it pays in
//! per-`Config` thread-pool setup.
//!
//! Run with:
//! ```text
//! cargo bench --bench batch_rendering
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::prelude::*;
use eger::render::RenderTarget;
use image::{Rgb as ImgRgb, RgbImage};
use std::path::PathBuf;

/// Writes `count` small, distinct PNGs into a fresh temp directory and
/// returns the directory path. Distinct per-file pixel data avoids the
/// benchmark accidentally measuring a filesystem/page cache best case for
/// one repeated identical file.
fn make_batch_dir(count: u32) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eger-bench-batch-{}-{count}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create bench temp dir");
    for i in 0..count {
        let buf = RgbImage::from_fn(64, 64, |x, y| {
            let seed = i.wrapping_mul(31);
            ImgRgb([
                ((x + seed) % 256) as u8,
                ((y + seed) % 256) as u8,
                ((x + y + seed) % 256) as u8,
            ])
        });
        image::DynamicImage::ImageRgb8(buf)
            .save(dir.join(format!("frame_{i:04}.png")))
            .expect("failed to write bench frame");
    }
    dir
}

fn bench_batch_by_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_batch_parallel_by_file_count");
    group.sample_size(20); // batches involve real file I/O; keep bench time reasonable

    for &count in &[4u32, 16, 64] {
        let dir = make_batch_dir(count);
        group.bench_with_input(BenchmarkId::from_parameter(count), &dir, |b, dir| {
            let config = Config::builder(MediaType::Image)
                .from_dir(dir, r"\.png$")
                .max_width(80)
                .build()
                .expect("config should build");
            b.iter(|| {
                let results = eger::render_batch_parallel(&config, |_path| RenderTarget::String)
                    .expect("batch render should succeed");
                black_box(results);
            });
        });
        std::fs::remove_dir_all(&dir).ok();
    }
    group.finish();
}

fn bench_batch_by_thread_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_batch_parallel_by_thread_count");
    group.sample_size(20);
    let dir = make_batch_dir(32);

    for &threads in &[1usize, 2, 4] {
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            |b, &threads| {
                b.iter_batched(
                    || {
                        // A fresh `Config` (and thus a fresh thread pool) per
                        // iteration: `Config::thread_pool` caches its pool for
                        // the `Config`'s lifetime, so reusing one `Config`
                        // across iterations would only ever measure the first
                        // iteration's pool-build cost.
                        Config::builder(MediaType::Image)
                            .from_dir(&dir, r"\.png$")
                            .max_width(80)
                            .num_threads(threads)
                            .build()
                            .expect("config should build")
                    },
                    |config| {
                        let results =
                            eger::render_batch_parallel(&config, |_path| RenderTarget::String)
                                .expect("batch render should succeed");
                        black_box(results);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    std::fs::remove_dir_all(&dir).ok();
    group.finish();
}

criterion_group!(benches, bench_batch_by_size, bench_batch_by_thread_count);
criterion_main!(benches);
