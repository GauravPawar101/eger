//! Benchmarks for `eger::banner` and `eger::text`: plain banner/glyph
//! generation, animated banner frame generation per TextAnimation style,
//! and single-line text animation frame generation.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::prelude::*;

const SHORT: &str = "HI";
const MEDIUM: &str = "HELLO WORLD";
const LONG: &str = "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 123";

fn bench_banner_lines_by_length(c: &mut Criterion) {
    let mut group = c.benchmark_group("banner_lines_by_text_length");
    for (name, text) in [("short", SHORT), ("medium", MEDIUM), ("long", LONG)] {
        group.bench_with_input(BenchmarkId::from_parameter(name), &text, |b, &text| {
            b.iter(|| black_box(banner_lines(black_box(text))));
        });
    }
    group.finish();
}

fn bench_banner_frames_by_animation(c: &mut Criterion) {
    let opts = TextAnimOptions {
        frames: 30,
        ..Default::default()
    };
    let animations: Vec<(&str, TextAnimation)> = vec![
        ("typewriter", TextAnimation::Typewriter),
        ("rainbow", TextAnimation::Rainbow),
        ("wave", TextAnimation::Wave),
        (
            "blink",
            TextAnimation::Blink {
                on_color: Rgb::new(255, 0, 0),
                off_color: Rgb::new(0, 0, 255),
            },
        ),
        ("pulse", TextAnimation::Pulse),
    ];

    let mut group = c.benchmark_group("banner_frames_by_animation_medium_text");
    for (name, animation) in &animations {
        group.bench_with_input(BenchmarkId::from_parameter(*name), animation, |b, animation| {
            b.iter(|| black_box(banner_frames(black_box(MEDIUM), black_box(animation), black_box(&opts)).unwrap()));
        });
    }
    group.finish();
}

fn bench_text_frames_by_animation(c: &mut Criterion) {
    let opts = TextAnimOptions {
        frames: 60,
        ..Default::default()
    };
    let animations: Vec<(&str, TextAnimation)> = vec![
        ("typewriter", TextAnimation::Typewriter),
        ("rainbow", TextAnimation::Rainbow),
        ("wave", TextAnimation::Wave),
        (
            "blink",
            TextAnimation::Blink {
                on_color: Rgb::new(255, 0, 0),
                off_color: Rgb::new(0, 0, 255),
            },
        ),
        ("marquee", TextAnimation::Marquee { width: 20 }),
        ("pulse", TextAnimation::Pulse),
        (
            "custom_function",
            TextAnimation::Custom(CustomAnimation::function(|text, i| format!("{text}-{i}"))),
        ),
    ];

    let mut group = c.benchmark_group("text_frames_by_animation");
    for (name, animation) in &animations {
        group.bench_with_input(BenchmarkId::from_parameter(*name), animation, |b, animation| {
            b.iter(|| black_box(text_frames(black_box(LONG), black_box(animation), black_box(&opts)).unwrap()));
        });
    }
    group.finish();
}

fn bench_banner_frames_scaling_with_frame_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("banner_frames_scaling_with_frame_count");
    for frames in [10u64, 40, 120] {
        let opts = TextAnimOptions {
            frames,
            ..Default::default()
        };
        group.bench_with_input(BenchmarkId::from_parameter(frames), &opts, |b, opts| {
            b.iter(|| {
                black_box(banner_frames(black_box(MEDIUM), black_box(&TextAnimation::Rainbow), black_box(opts)).unwrap())
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_banner_lines_by_length,
    bench_banner_frames_by_animation,
    bench_text_frames_by_animation,
    bench_banner_frames_scaling_with_frame_count
);
criterion_main!(benches);
