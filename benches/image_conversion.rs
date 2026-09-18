//! Benchmarks the core image → ASCII conversion path
//! (`render_dynamic_image`, which every single-image, batch, and
//! GIF-frame call site funnels through) across a range of source sizes,
//! output widths, and color depths.
//!
//! Run with:
//! ```text
//! cargo bench --bench image_conversion
//! cargo bench --bench image_conversion --no-default-features   # sequential-only path
//! ```
//! The second form disables the default `parallel` feature so you can
//! compare the row-level-parallel `iascii::convert_with_pool` path against
//! the plain sequential `iascii::convert_image` path on the same machine.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eger::prelude::*;
use eger::render::RenderTarget;
use image::{Rgb as ImgRgb, RgbImage};
use std::path::Path;

/// Builds a small, deterministic synthetic image entirely in memory (no
/// filesystem I/O in the hot loop) with enough variation across pixels
/// that luminance sampling isn't just hitting a single solid color.
fn synthetic_image(width: u32, height: u32) -> image::DynamicImage {
    let buf = RgbImage::from_fn(width, height, |x, y| {
        let r = ((x * 7 + y * 3) % 256) as u8;
        let g = ((x * 3 + y * 11) % 256) as u8;
        let b = ((x + y * 17) % 256) as u8;
        ImgRgb([r, g, b])
    });
    image::DynamicImage::ImageRgb8(buf)
}

fn bench_by_source_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert_by_source_size");
    // Representative range: a thumbnail, a webcam-ish frame, and a
    // moderately large photo, all downsampled to the same 120-col output
    // so the differences here reflect decode/sampling cost scaling with
    // source resolution, not output size.
    for &(w, h) in &[(64u32, 64u32), (640, 480), (1920, 1080)] {
        let img = synthetic_image(w, h);
        group.throughput(Throughput::Elements((w as u64) * (h as u64)));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{w}x{h}")),
            &img,
            |b, img| {
                let config = Config::builder(MediaType::Image)
                    .from_file(dummy_existing_file())
                    .max_width(120)
                    .color_depth(ColorDepth::TrueColor)
                    .build()
                    .expect("config should build");
                b.iter(|| {
                    let out =
                        eger::render_dynamic_image(black_box(img), &config, &RenderTarget::String)
                            .expect("conversion should succeed");
                    black_box(out);
                });
            },
        );
    }
    group.finish();
}

fn bench_by_output_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert_by_output_width");
    // Fixed 1280x720 source, varying only the target ASCII width, to show
    // how conversion cost scales with the size of the thing actually being
    // produced (and, at the largest width, where `parallel_threshold`
    // starts to matter).
    let img = synthetic_image(1280, 720);
    for &width in &[40usize, 120, 300] {
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, &width| {
            let config = Config::builder(MediaType::Image)
                .from_file(dummy_existing_file())
                .max_width(width)
                .color_depth(ColorDepth::TrueColor)
                .build()
                .expect("config should build");
            b.iter(|| {
                let out =
                    eger::render_dynamic_image(black_box(&img), &config, &RenderTarget::String)
                        .expect("conversion should succeed");
                black_box(out);
            });
        });
    }
    group.finish();
}

fn bench_by_color_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert_by_color_depth");
    let img = synthetic_image(640, 480);
    for depth in [
        ColorDepth::TrueColor,
        ColorDepth::Ansi256,
        ColorDepth::Ansi16,
    ] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{depth:?}")),
            &depth,
            |b, &depth| {
                let config = Config::builder(MediaType::Image)
                    .from_file(dummy_existing_file())
                    .max_width(120)
                    .color_depth(depth)
                    .build()
                    .expect("config should build");
                b.iter(|| {
                    let out =
                        eger::render_dynamic_image(black_box(&img), &config, &RenderTarget::String)
                            .expect("conversion should succeed");
                    black_box(out);
                });
            },
        );
    }
    group.finish();
}

/// `Config::builder(..).from_file(..)` requires the path to exist on disk
/// (it's validated in `build()`), even though this benchmark never reads
/// image bytes from it — conversion here always runs on the in-memory
/// `DynamicImage` from `synthetic_image`. This writes a tiny placeholder
/// once per process and reuses its path.
fn dummy_existing_file() -> &'static Path {
    use std::sync::OnceLock;
    static PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut path = std::env::temp_dir();
        path.push(format!("eger-bench-dummy-{}.png", std::process::id()));
        std::fs::write(&path, b"placeholder").expect("failed to write dummy bench file");
        path
    })
}

criterion_group!(
    benches,
    bench_by_source_size,
    bench_by_output_width,
    bench_by_color_depth
);
criterion_main!(benches);
