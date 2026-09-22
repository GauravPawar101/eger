//! Benchmarks for `eger::illusions`: raw pixel-buffer generation for each
//! static Illusion variant, the full render_illusion pipeline (generate +
//! convert + render), and the animated rotating-rings frame generator.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "eger-illusions-bench-{}-{}-{}",
        std::process::id(),
        n,
        name
    ));
    p
}

fn bench_generate_by_illusion(c: &mut Criterion) {
    let illusions: Vec<(&str, Illusion)> = vec![
        (
            "cafe_wall",
            Illusion::CafeWall {
                tile: 16,
                offset: 8,
            },
        ),
        (
            "hermann_grid",
            Illusion::HermannGrid {
                cell: 20,
                line_width: 3,
            },
        ),
        ("twisted_cord", Illusion::TwistedCord { lines: 8 }),
    ];

    let mut group = c.benchmark_group("illusions_generate_640x360");
    for (name, illusion) in illusions {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &illusion,
            |b, &illusion| {
                b.iter(|| {
                    black_box(generate_illusion(
                        black_box(illusion),
                        black_box(640),
                        black_box(360),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_render_illusion_full_pipeline(c: &mut Criterion) {
    // render_illusion needs a Config with a valid (if unused-by-generation)
    // source file, matching how the crate's own Config::files()/build()
    // validation works.
    let stub = temp_path("stub.png");
    ::image::RgbImage::from_pixel(2, 2, ::image::Rgb([0, 0, 0]))
        .save(&stub)
        .unwrap();

    let config = Config::builder(MediaType::Image)
        .from_file(&stub)
        .explicit_dimensions(120, 50)
        .color_depth(ColorDepth::TrueColor)
        .build()
        .unwrap();

    c.bench_function("illusions_render_cafe_wall_full_pipeline", |b| {
        b.iter(|| {
            black_box(
                render_illusion(
                    black_box(Illusion::CafeWall {
                        tile: 12,
                        offset: 6,
                    }),
                    black_box(480),
                    black_box(270),
                    black_box(&config),
                    black_box(&RenderTarget::String),
                )
                .unwrap(),
            )
        });
    });

    std::fs::remove_file(&stub).ok();
}

fn bench_rotating_rings_frames(c: &mut Criterion) {
    let mut group = c.benchmark_group("illusions_rotating_rings_frames_by_count");
    for frame_count in [10u64, 40, 120] {
        group.bench_with_input(
            BenchmarkId::from_parameter(frame_count),
            &frame_count,
            |b, &frame_count| {
                b.iter(|| {
                    black_box(eger::illusions::rotating_rings_frames(
                        black_box(80),
                        black_box(40),
                        black_box(6),
                        black_box(0.5),
                        black_box(frame_count),
                        black_box('#'),
                    ))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_generate_by_illusion,
    bench_render_illusion_full_pipeline,
    bench_rotating_rings_frames
);
criterion_main!(benches);
