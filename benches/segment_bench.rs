//! Benchmarks for `eger::segment`: automatic `SegmentMap::detect` flood
//! fill over grids with varying numbers of distinct color regions, and
//! `render_segments` styling cost.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::prelude::*;
use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::convert::convert_image;
use iascii::grid::Grid;

/// A grid of `blocks_per_side * blocks_per_side` distinct solid-color
/// tiles, so `SegmentMap::detect`'s flood fill has to discover
/// `blocks_per_side^2` separate regions.
fn tiled_grid(size: u32, blocks_per_side: u32) -> Grid {
    let cfg = AsciiConfigBuilder::new()
        .output_sizing(OutputSizing::Explicit {
            width: size as usize,
            height: size as usize,
        })
        .build()
        .unwrap();
    let tile = (size / blocks_per_side).max(1);
    let mut buf = Vec::with_capacity((size * size * 3) as usize);
    for y in 0..size {
        for x in 0..size {
            let bx = x / tile;
            let by = y / tile;
            let seed = bx.wrapping_mul(37).wrapping_add(by.wrapping_mul(101));
            buf.push((seed * 53 % 255) as u8);
            buf.push((seed * 97 % 255) as u8);
            buf.push((seed * 151 % 255) as u8);
        }
    }
    convert_image(size, size, &buf, &cfg).unwrap()
}

fn bench_detect_by_region_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_detect_by_region_count");
    for blocks_per_side in [2u32, 4, 8] {
        let grid = tiled_grid(60, blocks_per_side);
        group.bench_with_input(
            BenchmarkId::from_parameter(blocks_per_side * blocks_per_side),
            &grid,
            |b, grid| {
                b.iter(|| {
                    black_box(SegmentMap::detect(
                        black_box(grid),
                        SegmentOptions::default(),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_detect_by_grid_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_detect_by_grid_size");
    for size in [30u32, 60, 120] {
        let grid = tiled_grid(size, 4);
        group.bench_with_input(BenchmarkId::from_parameter(size), &grid, |b, grid| {
            b.iter(|| {
                black_box(SegmentMap::detect(
                    black_box(grid),
                    SegmentOptions::default(),
                ))
            });
        });
    }
    group.finish();
}

fn bench_render_segments_with_mixed_styles(c: &mut Criterion) {
    let grid = tiled_grid(60, 6);
    let map = SegmentMap::detect(&grid, SegmentOptions::default());

    let mut styles = SegmentStyles::new();
    for (i, seg) in map.segments().iter().enumerate() {
        let style = match i % 4 {
            0 => SegmentStyle::Bold,
            1 => SegmentStyle::Color(Rgb::new(200, 50, 50)),
            2 => SegmentStyle::Animation(PixelAnimation::Rainbow { speed: 5.0 }),
            _ => continue, // leave every fourth segment unstyled
        };
        styles = styles.set(seg.id, style);
    }

    c.bench_function("segment_render_segments_mixed_styles_60x60", |b| {
        b.iter(|| {
            black_box(eger::segment::render_segments(
                black_box(&grid),
                black_box(&map),
                black_box(&styles),
                black_box(Some(ColorDepth::TrueColor)),
                black_box(0),
            ))
        });
    });
}

fn bench_manual_segment_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_manual_construction_by_region_count");
    for n in [4usize, 16, 64] {
        let side = (n as f64).sqrt().ceil() as u32;
        let regions: Vec<(Option<String>, Rect)> = (0..n)
            .map(|i| {
                let x = (i as u32 % side) * 5;
                let y = (i as u32 / side) * 5;
                (None, Rect::new(x, y, 5, 5))
            })
            .collect();
        let dim = side * 5;
        group.bench_with_input(BenchmarkId::from_parameter(n), &regions, |b, regions| {
            b.iter(|| {
                black_box(SegmentMap::manual(
                    black_box(dim),
                    black_box(dim),
                    black_box(regions.clone()),
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_detect_by_region_count,
    bench_detect_by_grid_size,
    bench_render_segments_with_mixed_styles,
    bench_manual_segment_construction
);
criterion_main!(benches);
