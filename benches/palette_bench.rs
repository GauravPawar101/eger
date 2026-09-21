//! Benchmarks for `eger`'s palette quantization helpers (`crate::palette`,
//! exercised indirectly since it's a private module) via the public
//! surface that drives it: `dither::render_ansi_dithered` at
//! `DitherMethod::None` (pure per-cell quantization, no error diffusion
//! overhead) isolates quantization cost from diffusion cost, and a direct
//! microbenchmark of `iascii`'s own nearest-color search gives a
//! deeper-in-the-hot-path number for comparison.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::{DitherMethod, DitherOptions};
use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::convert::convert_image;
use iascii::grid::Grid;
use iascii::render::{render_ansi, ColorDepth};

/// Random-ish (but deterministic) RGB noise: the worst case for a
/// nearest-palette search, since real photos have far more spatial
/// coherence (and thus cache-friendly repeated lookups) than this.
fn noisy_grid(w: u32, h: u32) -> Grid {
    let cfg = AsciiConfigBuilder::new()
        .output_sizing(OutputSizing::Explicit {
            width: w as usize,
            height: h as usize,
        })
        .build()
        .unwrap();
    let mut buf = Vec::with_capacity((w * h * 3) as usize);
    // A small xorshift PRNG so the benchmark has no external `rand`
    // dependency and stays perfectly reproducible run to run.
    let mut state: u32 = 0x9E3779B9;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for _ in 0..(w * h) {
        let v = next();
        buf.push((v & 0xFF) as u8);
        buf.push(((v >> 8) & 0xFF) as u8);
        buf.push(((v >> 16) & 0xFF) as u8);
    }
    convert_image(w, h, &buf, &cfg).unwrap()
}

fn bench_quantization_by_depth(c: &mut Criterion) {
    let grid = noisy_grid(160, 60);
    let mut group = c.benchmark_group("palette_quantize_none_dither");
    for depth in [ColorDepth::Ansi16, ColorDepth::Ansi256, ColorDepth::TrueColor] {
        group.bench_with_input(BenchmarkId::from_parameter(format!("{depth:?}")), &depth, |b, &depth| {
            let options = DitherOptions::new(DitherMethod::None);
            b.iter(|| {
                black_box(eger::dither::render_ansi_dithered(
                    black_box(&grid),
                    black_box(depth),
                    black_box(options),
                ))
            });
        });
    }
    group.finish();
}

fn bench_iascii_render_ansi_by_depth(c: &mut Criterion) {
    let grid = noisy_grid(160, 60);
    let mut group = c.benchmark_group("palette_iascii_render_ansi");
    for depth in [ColorDepth::Ansi16, ColorDepth::Ansi256, ColorDepth::TrueColor] {
        group.bench_with_input(BenchmarkId::from_parameter(format!("{depth:?}")), &depth, |b, &depth| {
            b.iter(|| black_box(render_ansi(black_box(&grid), black_box(depth))));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_quantization_by_depth,
    bench_iascii_render_ansi_by_depth
);
criterion_main!(benches);
