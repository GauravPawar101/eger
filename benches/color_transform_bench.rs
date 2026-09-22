//! Benchmarks for `eger::color::ColorTransform`: per-color transform cost
//! for every built-in kind, a `Compose` chain, the `apply_rgb8_buffer`
//! whole-image path, and the transform-aware dithering entry point
//! (`dither::render_ansi_dithered_transformed`) against the untransformed
//! baseline.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eger::prelude::*;
use eger::ColorTransform;
use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::convert::convert_image;
use iascii::grid::Grid;

fn photo_like_grid(w: u32, h: u32) -> Grid {
    let cfg = AsciiConfigBuilder::new()
        .output_sizing(OutputSizing::Explicit {
            width: w as usize,
            height: h as usize,
        })
        .build()
        .unwrap();
    let mut state: u32 = 0xDEADBEEF;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut buf = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..(w * h) {
        let v = next();
        buf.push((v & 0xFF) as u8);
        buf.push(((v >> 8) & 0xFF) as u8);
        buf.push(((v >> 16) & 0xFF) as u8);
    }
    convert_image(w, h, &buf, &cfg).unwrap()
}

fn transforms() -> Vec<(&'static str, ColorTransform)> {
    vec![
        ("none", ColorTransform::None),
        ("grayscale", ColorTransform::Grayscale),
        ("invert", ColorTransform::Invert),
        ("sepia", ColorTransform::Sepia),
        ("brightness", ColorTransform::Brightness(1.4)),
        ("contrast", ColorTransform::Contrast(1.4)),
        ("saturate", ColorTransform::Saturate(1.4)),
        ("hue_rotate", ColorTransform::HueRotate(90.0)),
        (
            "tint",
            ColorTransform::Tint {
                color: Rgb::ORANGE,
                amount: 0.3,
            },
        ),
        (
            "compose_4",
            ColorTransform::Compose(vec![
                ColorTransform::Grayscale,
                ColorTransform::Brightness(1.2),
                ColorTransform::Contrast(1.1),
                ColorTransform::Sepia,
            ]),
        ),
    ]
}

fn bench_apply_single_color(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_transform_apply_single_color");
    for (name, transform) in transforms() {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &transform,
            |b, transform| {
                b.iter(|| black_box(transform.apply(black_box(Rgb::new(120, 60, 200)))));
            },
        );
    }
    group.finish();
}

fn bench_apply_rgb8_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_transform_apply_rgb8_buffer_200x200");
    group.throughput(Throughput::Elements(200 * 200));
    for (name, transform) in transforms() {
        let mut buf = vec![128u8; 200 * 200 * 3];
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &transform,
            |b, transform| {
                b.iter(|| {
                    transform.apply_rgb8_buffer(black_box(&mut buf));
                    black_box(&buf);
                });
            },
        );
    }
    group.finish();
}

fn bench_dither_transformed_vs_untransformed(c: &mut Criterion) {
    let grid = photo_like_grid(160, 60);
    let mut group = c.benchmark_group("dither_transformed_vs_untransformed_160x60");

    group.bench_function("untransformed", |b| {
        b.iter(|| {
            black_box(eger::dither::render_ansi_dithered(
                black_box(&grid),
                black_box(iascii::render::ColorDepth::Ansi256),
                black_box(DitherOptions::new(DitherMethod::FloydSteinberg)),
            ))
        });
    });

    for (name, transform) in [
        ("grayscale", ColorTransform::Grayscale),
        ("sepia", ColorTransform::Sepia),
        ("hue_rotate", ColorTransform::HueRotate(45.0)),
    ] {
        group.bench_with_input(
            BenchmarkId::new("transformed", name),
            &transform,
            |b, transform| {
                b.iter(|| {
                    black_box(eger::dither::render_ansi_dithered_transformed(
                        black_box(&grid),
                        black_box(iascii::render::ColorDepth::Ansi256),
                        black_box(DitherOptions::new(DitherMethod::FloydSteinberg)),
                        black_box(transform),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_colorize_transformed_animation(c: &mut Criterion) {
    let lines: Vec<String> = (0..30)
        .map(|y: usize| {
            (0..80)
                .map(|x: usize| if (x + y) % 3 == 0 { ' ' } else { '#' })
                .collect()
        })
        .collect();

    let mut group = c.benchmark_group("colorize_transformed_vs_plain_rainbow_80x30");
    group.bench_function("plain_rainbow", |b| {
        let animation = PixelAnimation::Rainbow { speed: 6.0 };
        b.iter(|| {
            black_box(colorize_lines(
                black_box(&lines),
                black_box(&animation),
                black_box(3),
            ))
        });
    });
    group.bench_function("transformed_rainbow_saturate", |b| {
        let animation = PixelAnimation::Transformed {
            base: Box::new(PixelAnimation::Rainbow { speed: 6.0 }),
            transform: ColorTransform::Saturate(0.4),
        };
        b.iter(|| {
            black_box(colorize_lines(
                black_box(&lines),
                black_box(&animation),
                black_box(3),
            ))
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_apply_single_color,
    bench_apply_rgb8_buffer,
    bench_dither_transformed_vs_untransformed,
    bench_colorize_transformed_animation
);
criterion_main!(benches);
