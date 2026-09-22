//! Benchmarks for `eger::colorize`: single-frame coloring cost for every
//! built-in `PixelAnimation`, plus the parallel multi-frame generation
//! paths (`colorize_frames`, `colorize_sequence`).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eger::{colorize_frames, colorize_lines, colorize_sequence};
use eger::{PixelAnimation, Rgb};

fn block_lines(w: usize, h: usize) -> Vec<String> {
    // Alternating '#' / ' ' checkerboard: non-trivial (exercises both the
    // "space stays uncolored" path and real per-cell coloring), unlike an
    // all-solid block which would collapse every animation into one big
    // escape/reset run.
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| if (x + y) % 3 == 0 { ' ' } else { '#' })
                .collect()
        })
        .collect()
}

fn animations() -> Vec<(&'static str, PixelAnimation)> {
    vec![
        ("rainbow", PixelAnimation::Rainbow { speed: 6.0 }),
        (
            "wave",
            PixelAnimation::Wave {
                base_color: Rgb::WHITE,
            },
        ),
        (
            "pulse",
            PixelAnimation::Pulse {
                base_color: Rgb::WHITE,
            },
        ),
        (
            "blink",
            PixelAnimation::Blink {
                on_color: Rgb::new(255, 0, 0),
                off_color: Rgb::new(0, 0, 255),
            },
        ),
        (
            "plasma",
            PixelAnimation::Plasma {
                scale: 0.35,
                speed: 0.3,
            },
        ),
        (
            "ripple",
            PixelAnimation::Ripple {
                base_color: Rgb::WHITE,
                speed: 0.4,
            },
        ),
        (
            "custom_gradient",
            PixelAnimation::custom(|x, y, _frame, _ch| {
                let t = ((x + y) as f32 / 80.0).clamp(0.0, 1.0);
                Rgb::new((255.0 * (1.0 - t)) as u8, 0, (255.0 * t) as u8)
            }),
        ),
    ]
}

fn bench_single_frame_per_animation(c: &mut Criterion) {
    let lines = block_lines(120, 40);
    let cells = (120 * 40) as u64;
    let mut group = c.benchmark_group("colorize_single_frame_120x40");
    group.throughput(Throughput::Elements(cells));
    for (name, animation) in animations() {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &animation,
            |b, animation| {
                b.iter(|| {
                    black_box(colorize_lines(
                        black_box(&lines),
                        black_box(animation),
                        black_box(3),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_frame_size_scaling(c: &mut Criterion) {
    let animation = PixelAnimation::Plasma {
        scale: 0.3,
        speed: 0.2,
    };
    let mut group = c.benchmark_group("colorize_frame_size_scaling");
    for (w, h) in [(40, 12), (80, 24), (160, 48), (320, 96)] {
        let lines = block_lines(w, h);
        group.throughput(Throughput::Elements((w * h) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{w}x{h}")),
            &lines,
            |b, lines| {
                b.iter(|| {
                    black_box(colorize_lines(
                        black_box(lines),
                        black_box(&animation),
                        black_box(1),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_colorize_frames_parallel(c: &mut Criterion) {
    let lines = block_lines(100, 30);
    let animation = PixelAnimation::Rainbow { speed: 5.0 };
    let mut group = c.benchmark_group("colorize_frames_parallel_generation");
    for frame_count in [8u64, 32, 128] {
        group.bench_with_input(
            BenchmarkId::from_parameter(frame_count),
            &frame_count,
            |b, &frame_count| {
                b.iter(|| {
                    black_box(colorize_frames(
                        black_box(&lines),
                        black_box(&animation),
                        black_box(frame_count),
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_colorize_sequence_parallel(c: &mut Criterion) {
    // Simulates recoloring a decoded GIF/video's worth of already-distinct
    // frames, as opposed to colorize_frames' "same static art, N colored
    // copies" case above.
    let frames: Vec<Vec<String>> = (0..32).map(|_| block_lines(80, 24)).collect();
    let animation = PixelAnimation::Ripple {
        base_color: Rgb::WHITE,
        speed: 0.3,
    };
    c.bench_function("colorize_sequence_32_frames_80x24", |b| {
        b.iter(|| black_box(colorize_sequence(black_box(&frames), black_box(&animation))));
    });
}

criterion_group!(
    benches,
    bench_single_frame_per_animation,
    bench_frame_size_scaling,
    bench_colorize_frames_parallel,
    bench_colorize_sequence_parallel
);
criterion_main!(benches);
