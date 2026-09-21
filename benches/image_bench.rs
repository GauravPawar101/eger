//! Benchmarks for `eger::image`: single-image conversion at a few sizes,
//! and batch parallel rendering across a directory of real PNGs at
//! different thread-pool sizes.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eger::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("eger-bench-{}-{}-{}", std::process::id(), n, name));
    p
}

fn write_photo_like_png(path: &std::path::Path, w: u32, h: u32) {
    // A radial gradient with a bit of high-frequency noise mixed in, closer
    // to a real photo's statistics than a flat color or pure gradient.
    let mut state: u32 = 0xC0FFEE ^ w ^ h;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let max_d = (cx * cx + cy * cy).sqrt().max(1.0);
    let img = ::image::RgbImage::from_fn(w, h, |x, y| {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let d = (dx * dx + dy * dy).sqrt() / max_d;
        let base = (255.0 * (1.0 - d)) as i32;
        let noise = (next() % 32) as i32 - 16;
        let v = (base + noise).clamp(0, 255) as u8;
        ::image::Rgb([v, (v / 2).max(10), (255 - v)])
    });
    img.save(path).unwrap();
}

fn bench_single_image_conversion_by_size(c: &mut Criterion) {
    let png = temp_path("single.png");
    write_photo_like_png(&png, 512, 512);

    let mut group = c.benchmark_group("image_single_conversion_by_output_size");
    for (w, h) in [(40, 20), (80, 40), (160, 80), (320, 160)] {
        let config = Config::builder(MediaType::Image)
            .from_file(&png)
            .explicit_dimensions(w, h)
            .color_depth(ColorDepth::TrueColor)
            .build()
            .unwrap();
        group.throughput(Throughput::Elements((w * h) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{w}x{h}")),
            &config,
            |b, config| {
                b.iter(|| black_box(eger::image_to_string(black_box(&png), black_box(config)).unwrap()));
            },
        );
    }
    group.finish();

    std::fs::remove_file(&png).ok();
}

fn bench_batch_parallel_by_thread_count(c: &mut Criterion) {
    let dir = temp_path("batch-dir");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..8 {
        write_photo_like_png(&dir.join(format!("img{i}.png")), 200, 200);
    }

    let mut group = c.benchmark_group("image_batch_parallel_by_threads");
    group.sample_size(20);
    for threads in [1usize, 2, 4] {
        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .explicit_dimensions(60, 30)
            .num_threads(threads)
            .build()
            .unwrap();
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &config,
            |b, config| {
                b.iter(|| {
                    black_box(
                        eger::render_batch_parallel(black_box(config), |_| RenderTarget::Lines)
                            .unwrap(),
                    )
                });
            },
        );
    }
    group.finish();

    std::fs::remove_dir_all(&dir).ok();
}

fn bench_dithered_vs_plain_render(c: &mut Criterion) {
    let png = temp_path("dither-vs-plain.png");
    write_photo_like_png(&png, 300, 300);

    let plain = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(120, 60)
        .color_depth(ColorDepth::Ansi256)
        .build()
        .unwrap();
    let dithered = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(120, 60)
        .color_depth(ColorDepth::Ansi256)
        .dither(eger::DitherOptions::new(eger::DitherMethod::FloydSteinberg))
        .build()
        .unwrap();

    let mut group = c.benchmark_group("image_dithered_vs_plain_ansi256");
    group.bench_function("plain", |b| {
        b.iter(|| black_box(eger::image_to_string(black_box(&png), black_box(&plain)).unwrap()));
    });
    group.bench_function("dithered_floyd_steinberg", |b| {
        b.iter(|| black_box(eger::image_to_string(black_box(&png), black_box(&dithered)).unwrap()));
    });
    group.finish();

    std::fs::remove_file(&png).ok();
}

criterion_group!(
    benches,
    bench_single_image_conversion_by_size,
    bench_batch_parallel_by_thread_count,
    bench_dithered_vs_plain_render
);
criterion_main!(benches);
