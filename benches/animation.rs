//! Benchmarks the text/banner animation frame-generation hot paths used by
//! TUIs and terminal games for loading indicators, HUDs, and title
//! screens: `text_frames` (one line) and `banner_frames` (multi-line block
//! letters), across the built-in animation styles and a couple of text
//! lengths.
//!
//! Run with:
//! ```text
//! cargo bench --bench animation
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use eger::prelude::*;

const SHORT_TEXT: &str = "LOADING";
const LONG_TEXT: &str = "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789";

fn animations() -> Vec<(&'static str, TextAnimation)> {
    vec![
        ("typewriter", TextAnimation::Typewriter),
        ("rainbow", TextAnimation::Rainbow),
        ("wave", TextAnimation::Wave),
        ("pulse", TextAnimation::Pulse),
        (
            "blink",
            TextAnimation::Blink {
                on_color: Rgb::WHITE,
                off_color: Rgb::new(0, 0, 0),
            },
        ),
        ("marquee", TextAnimation::Marquee { width: 20 }),
    ]
}

fn bench_text_frames(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_frames");
    let opts = TextAnimOptions {
        frames: 120,
        ..Default::default()
    };

    for (name, animation) in animations() {
        for (text_name, text) in [("short", SHORT_TEXT), ("long", LONG_TEXT)] {
            group.bench_with_input(
                BenchmarkId::new(name, text_name),
                &(animation.clone(), text),
                |b, (animation, text)| {
                    b.iter(|| {
                        let frames = eger::text_frames(black_box(text), animation, &opts)
                            .expect("text_frames should succeed");
                        black_box(frames);
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_banner_frames(c: &mut Criterion) {
    let mut group = c.benchmark_group("banner_frames");
    let opts = TextAnimOptions {
        frames: 60, // banners are 7 rows tall per frame, so keep this smaller than text_frames
        ..Default::default()
    };

    // banner_frames doesn't support Marquee/Custom (falls back to a static
    // base color — see banner.rs's module docs), so benchmark the styles
    // it actually animates per-glyph.
    for (name, animation) in animations()
        .into_iter()
        .filter(|(name, _)| !matches!(*name, "marquee"))
    {
        for (text_name, text) in [("short", SHORT_TEXT), ("long", LONG_TEXT)] {
            group.bench_with_input(
                BenchmarkId::new(name, text_name),
                &(animation.clone(), text),
                |b, (animation, text)| {
                    b.iter(|| {
                        let frames = eger::banner_frames(black_box(text), animation, &opts)
                            .expect("banner_frames should succeed");
                        black_box(frames);
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_banner_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("banner_lines");
    for (text_name, text) in [("short", SHORT_TEXT), ("long", LONG_TEXT)] {
        group.bench_with_input(BenchmarkId::from_parameter(text_name), &text, |b, &text| {
            b.iter(|| {
                let lines = eger::banner_lines(black_box(text));
                black_box(lines);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_text_frames,
    bench_banner_frames,
    bench_banner_lines
);
criterion_main!(benches);
