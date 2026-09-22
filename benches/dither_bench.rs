//! Benchmarks for `eger::dither`: every DitherMethod across every
//! ColorDepth, at a couple of grid sizes, plus the plain (undithered)
//! `iascii::render::render_ansi` baseline for comparison.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eger::{DitherMethod, DitherOptions};
use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::convert::convert_image;
use iascii::grid::Grid;
use iascii::render::{render_ansi, ColorDepth};

/// A smooth horizontal gradient, the case dithering is meant to help with
/// (banding on a coarse palette).
fn gradient_grid(w: u32, h: u32) -> Grid {
    let cfg = AsciiConfigBuilder::new()
        .output_sizing(OutputSizing::Explicit {
            width: w as usize,
            height: h as usize,
        })
        .build()
        .unwrap();
    let mut buf = Vec::with_capacity((w * h * 3) as usize);
    for _y in 0..h {
        for x in 0..w {
            let v = ((x as f32 / w.max(1) as f32) * 255.0) as u8;
            buf.extend_from_slice(&[v, v, v]);
        }
    }
    convert_image(w, h, &buf, &cfg).unwrap()
}

const SIZES: [(u32, u32); 2] = [(80, 24), (240, 80)];

fn bench_dither_methods(c: &mut Criterion) {
    let methods = [
        DitherMethod::None,
        DitherMethod::FloydSteinberg,
        DitherMethod::Atkinson,
        DitherMethod::Bayer2,
        DitherMethod::Bayer4,
        DitherMethod::Bayer8,
    ];
    let depths = [
        ColorDepth::Ansi16,
        ColorDepth::Ansi256,
        ColorDepth::TrueColor,
    ];

    for (w, h) in SIZES {
        let grid = gradient_grid(w, h);
        let cells = (w * h) as u64;

        let mut group = c.benchmark_group(format!("dither_{w}x{h}"));
        group.throughput(Throughput::Elements(cells));

        for depth in depths {
            for method in methods {
                let options = DitherOptions::new(method);
                group.bench_with_input(
                    BenchmarkId::new(format!("{depth:?}"), format!("{method:?}")),
                    &(depth, options),
                    |b, (depth, options)| {
                        b.iter(|| {
                            black_box(eger::dither::render_ansi_dithered(
                                black_box(&grid),
                                black_box(*depth),
                                black_box(*options),
                            ))
                        });
                    },
                );
            }
        }

        group.bench_function("baseline_iascii_render_ansi", |b| {
            b.iter(|| {
                black_box(render_ansi(
                    black_box(&grid),
                    black_box(ColorDepth::TrueColor),
                ))
            });
        });

        group.finish();
    }
}

fn bench_truecolor_posterize_levels(c: &mut Criterion) {
    let grid = gradient_grid(160, 48);
    let mut group = c.benchmark_group("dither_truecolor_posterize_levels");
    for levels in [2u8, 4, 8, 16, 32] {
        group.bench_with_input(
            BenchmarkId::from_parameter(levels),
            &levels,
            |b, &levels| {
                let options = DitherOptions::new(DitherMethod::FloydSteinberg).levels(levels);
                b.iter(|| {
                    black_box(eger::dither::render_ansi_dithered(
                        black_box(&grid),
                        black_box(ColorDepth::TrueColor),
                        black_box(options),
                    ))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_dither_methods,
    bench_truecolor_posterize_levels
);
criterion_main!(benches);
