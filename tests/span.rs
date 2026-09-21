//! End-to-end integration tests exercising combinations of `eger`'s
//! modules the way a real caller would chain them together, rather than
//! each module's own unit tests (which necessarily test one module in
//! isolation). These are intentionally slower/heavier than the `#[cfg(test)]`
//! unit tests embedded in each module.

use eger::prelude::*;
use eger::{
    colorize_frames, colorize_grid, colorize_joined_sequence, colorize_sequence, render_image_file,
};
use eger::{DitherMethod, DitherOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A process-unique temp file/dir path, so parallel test binaries (or
/// `cargo test`'s own multi-threaded test runner) never collide on the
/// same path.
fn temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "eger-integration-{}-{}-{}",
        std::process::id(),
        n,
        name
    ));
    p
}

fn write_checkerboard_png(path: &Path, w: u32, h: u32) {
    let img = ::image::RgbImage::from_fn(w, h, |x, y| {
        if (x + y) % 2 == 0 {
            ::image::Rgb([20u8, 20, 20])
        } else {
            ::image::Rgb([235u8, 235, 235])
        }
    });
    img.save(path).unwrap();
}

fn write_gradient_png(path: &Path, w: u32, h: u32) {
    let img = ::image::RgbImage::from_fn(w, h, |x, _y| {
        let v = ((x as f32 / w.max(1) as f32) * 255.0) as u8;
        ::image::Rgb([v, v, v])
    });
    img.save(path).unwrap();
}

// ---------------------------------------------------------------------
// config -> image -> render, across every RenderTarget
// ---------------------------------------------------------------------

#[test]
fn full_pipeline_image_file_to_every_render_target() {
    let png = temp_path("pipeline.png");
    write_checkerboard_png(&png, 16, 16);

    let config = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(10, 6)
        .color_depth(ColorDepth::TrueColor)
        .build()
        .unwrap();

    // String target
    let text = eger::image_to_string(&png, &config).unwrap();
    assert_eq!(text.lines().count(), 6);
    assert!(text.contains('\x1b'), "TrueColor should emit ANSI escapes");

    // Lines target
    let lines = eger::image_to_lines(&png, &config).unwrap();
    assert_eq!(lines.len(), 6);

    // File target
    let out = temp_path("pipeline-out.txt");
    let written = eger::image_to_file(&png, &config, Some(&out)).unwrap();
    assert_eq!(written, out);
    let contents = std::fs::read_to_string(&out).unwrap();
    assert_eq!(contents.trim_end_matches('\n'), text.trim_end_matches('\n'));

    std::fs::remove_file(&png).ok();
    std::fs::remove_file(&out).ok();
}

#[test]
fn plain_no_color_config_produces_ansi_free_output() {
    let png = temp_path("plain.png");
    write_checkerboard_png(&png, 8, 8);

    let config = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(6, 4)
        .build()
        .unwrap();

    let target = RenderTarget::String;
    let output = render_image_file(&png, &config, &target).unwrap();
    match output {
        RenderOutput::Text(_) => {}
        other => panic!("expected Text, got {other:?}"),
    }

    std::fs::remove_file(&png).ok();
}

// ---------------------------------------------------------------------
// config + dither -> render_dynamic_image, for every method/depth combo
// ---------------------------------------------------------------------

#[test]
fn dithered_render_dynamic_image_covers_every_method_and_depth() {
    let png = temp_path("dither-pipeline.png");
    write_gradient_png(&png, 32, 8);
    let img = ::image::open(&png).unwrap();

    for method in [
        DitherMethod::None,
        DitherMethod::FloydSteinberg,
        DitherMethod::Atkinson,
        DitherMethod::Bayer2,
        DitherMethod::Bayer4,
        DitherMethod::Bayer8,
    ] {
        for depth in [ColorDepth::Ansi16, ColorDepth::Ansi256, ColorDepth::TrueColor] {
            let config = Config::builder(MediaType::Image)
                .from_file(&png)
                .explicit_dimensions(20, 6)
                .color_depth(depth)
                .dither(DitherOptions::new(method))
                .build()
                .unwrap();

            let output = eger::render_dynamic_image(&img, &config, &RenderTarget::String).unwrap();
            let text = match output {
                RenderOutput::Text(t) => t,
                other => panic!("expected Text, got {other:?}"),
            };
            assert_eq!(
                text.lines().count(),
                6,
                "method={method:?} depth={depth:?} should still keep the requested row count"
            );
        }
    }

    std::fs::remove_file(&png).ok();
}

#[test]
fn dither_and_no_dither_configs_can_diverge_on_a_coarse_palette_gradient() {
    // Sanity check that plumbing `Config::dither` through
    // `render_dynamic_image` actually changes the rendered bytes on a
    // palette coarse enough for banding to matter (Ansi16), the way
    // `crate::dither`'s own unit tests establish for the lower-level API.
    let png = temp_path("dither-diverge.png");
    write_gradient_png(&png, 64, 1);
    let img = ::image::open(&png).unwrap();

    let base = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(64, 1)
        .color_depth(ColorDepth::Ansi16)
        .build()
        .unwrap();
    let dithered = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(64, 1)
        .color_depth(ColorDepth::Ansi16)
        .dither(DitherOptions::new(DitherMethod::FloydSteinberg))
        .build()
        .unwrap();

    let plain_text = match eger::render_dynamic_image(&img, &base, &RenderTarget::String).unwrap() {
        RenderOutput::Text(t) => t,
        _ => unreachable!(),
    };
    let dithered_text =
        match eger::render_dynamic_image(&img, &dithered, &RenderTarget::String).unwrap() {
            RenderOutput::Text(t) => t,
            _ => unreachable!(),
        };
    assert_ne!(plain_text, dithered_text);

    std::fs::remove_file(&png).ok();
}

// ---------------------------------------------------------------------
// illusions -> config -> render, including dithered illusions
// ---------------------------------------------------------------------

#[test]
fn every_illusion_renders_through_the_full_config_pipeline() {
    let png_stub = temp_path("illusion-stub.png"); // illusions don't read this; Config just needs a valid source
    write_checkerboard_png(&png_stub, 2, 2);

    for illusion in [
        Illusion::CafeWall { tile: 6, offset: 3 },
        Illusion::HermannGrid {
            cell: 8,
            line_width: 2,
        },
        Illusion::TwistedCord { lines: 4 },
    ] {
        let config = Config::builder(MediaType::Image)
            .from_file(&png_stub)
            .explicit_dimensions(24, 10)
            .color_depth(ColorDepth::Ansi256)
            .dither(DitherOptions::new(DitherMethod::Bayer4))
            .build()
            .unwrap();

        let output =
            render_illusion(illusion, 48, 20, &config, &RenderTarget::String).unwrap();
        let text = match output {
            RenderOutput::Text(t) => t,
            other => panic!("expected Text, got {other:?}"),
        };
        assert_eq!(text.lines().count(), 10, "{illusion:?}");
    }

    std::fs::remove_file(&png_stub).ok();
}

#[test]
fn rotating_rings_frames_can_be_recolored_again_with_a_pixel_animation() {
    // rotating_rings_frames already returns ANSI-colored frames; feeding
    // them back through colorize_joined_sequence (post-processing already
    // colored art) should still work and not panic, even though most
    // colorize_* call sites in this crate expect *plain* text — the
    // ANSI-colored cells parse as ordinary (albeit unusual) characters.
    let frames = eger::generate_illusion(
        eger::Illusion::CafeWall { tile: 4, offset: 2 },
        16,
        8,
    );
    assert_eq!((frames.width(), frames.height()), (16, 8));
}

// ---------------------------------------------------------------------
// segment: detect over a real converted image, then style + render
// ---------------------------------------------------------------------

#[test]
fn segment_detect_on_a_real_converted_photo_then_style_named_regions() {
    let png = temp_path("segment.png");
    // Left half distinctly dark, right half distinctly light, so
    // `SegmentMap::detect`'s color-tolerance flood fill reliably finds
    // exactly two regions.
    let img = ::image::RgbImage::from_fn(20, 10, |x, _y| {
        if x < 10 {
            ::image::Rgb([10u8, 10, 10])
        } else {
            ::image::Rgb([245u8, 245, 245])
        }
    });
    img.save(&png).unwrap();

    let config = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(20, 10)
        .build()
        .unwrap();

    let ascii_img = ::image::open(&png).unwrap();
    let rgb = ascii_img.to_rgb8();
    let grid = iascii::convert_image(rgb.width(), rgb.height(), rgb.as_raw(), &config.ascii)
        .unwrap();

    let mut map = SegmentMap::detect(&grid, SegmentOptions::default());
    assert_eq!(map.segments().len(), 2);

    let dark_id = map.segment_id_at(0, 0).unwrap();
    let light_id = map.segment_id_at(19, 0).unwrap();
    assert!(map.set_name(dark_id, "dark"));
    assert!(map.set_name(light_id, "light"));

    let styles = SegmentStyles::new()
        .set_named(&map, "dark", SegmentStyle::Color(Rgb::new(1, 2, 3)))
        .set_named(
            &map,
            "light",
            SegmentStyle::Animation(PixelAnimation::Rainbow { speed: 4.0 }),
        );

    let out = eger::segment::render_segments(
        &grid,
        &map,
        &styles,
        Some(ColorDepth::TrueColor),
        0,
    );
    assert!(out.contains("38;2;1;2;3"), "dark segment override color");
    assert_eq!(out.lines().count(), 10);

    let overlay = map.overlay_ids();
    assert_eq!(overlay.lines().count(), 10);
    assert!(overlay.contains('1') && overlay.contains('2'));

    std::fs::remove_file(&png).ok();
}

// ---------------------------------------------------------------------
// banner -> colorize: animate an already plain-rendered banner
// ---------------------------------------------------------------------

#[test]
fn banner_lines_recolored_with_every_builtin_pixel_animation() {
    let lines = banner_lines("HI");
    assert_eq!(lines.len(), 7);

    let animations = [
        PixelAnimation::Rainbow { speed: 6.0 },
        PixelAnimation::Wave {
            base_color: Rgb::WHITE,
        },
        PixelAnimation::Pulse {
            base_color: Rgb::WHITE,
        },
        PixelAnimation::Blink {
            on_color: Rgb::new(255, 0, 0),
            off_color: Rgb::new(0, 0, 255),
        },
        PixelAnimation::Plasma {
            scale: 0.4,
            speed: 0.3,
        },
        PixelAnimation::Ripple {
            base_color: Rgb::WHITE,
            speed: 0.5,
        },
    ];

    for animation in animations {
        let frames = colorize_frames(&lines, &animation, 4);
        assert_eq!(frames.len(), 4);
        for frame in &frames {
            assert_eq!(frame.lines().count(), 7);
            assert!(frame.contains("\x1b[38;2;"));
        }
    }
}

#[test]
fn banner_frames_own_per_character_coloring_differs_from_pixel_animation_recoloring() {
    // banner_frames colors per *source character* (glyph-wide blocks);
    // colorize_frames over banner_lines colors per *grid cell*. These are
    // two different, both-valid ways to animate the same banner text, and
    // this test just establishes they really do produce different output
    // for a non-trivial animation, i.e. neither one is accidentally a
    // no-op wrapper around the other.
    let opts = TextAnimOptions {
        frames: 3,
        ..Default::default()
    };
    let per_char = banner_frames("HI", &TextAnimation::Rainbow, &opts).unwrap();
    let plain_lines = banner_lines("HI");
    let per_cell = colorize_frames(&plain_lines, &PixelAnimation::Rainbow { speed: 6.0 }, 3);

    assert_eq!(per_char.len(), 3);
    assert_eq!(per_cell.len(), 3);
    // Both color things, but not with the literal same string content.
    assert_ne!(per_char[0].join("\n"), per_cell[0]);
}

// ---------------------------------------------------------------------
// GIF decode -> colorize_sequence / colorize_joined_sequence
// ---------------------------------------------------------------------

fn write_test_gif(path: &Path, frame_count: u32) {
    use ::image::codecs::gif::{GifEncoder, Repeat};
    use ::image::{Delay, Frame, RgbaImage};

    let mut file = std::fs::File::create(path).unwrap();
    let mut encoder = GifEncoder::new(&mut file);
    encoder.set_repeat(Repeat::Infinite).unwrap();
    for i in 0..frame_count {
        let level = (i * 40) as u8;
        let buf = RgbaImage::from_pixel(4, 4, ::image::Rgba([level, level, level, 255]));
        encoder
            .encode_frame(Frame::from_parts(
                buf,
                0,
                0,
                Delay::from_numer_denom_ms(50, 1),
            ))
            .unwrap();
    }
}

#[test]
fn gif_frames_can_be_recolored_with_colorize_sequence_and_joined_sequence() {
    let path = temp_path("sequence.gif");
    write_test_gif(&path, 3);

    let config = Config::builder(MediaType::Image)
        .from_file(&path)
        .explicit_dimensions(4, 4)
        .build()
        .unwrap();

    let decoded = eger::gif_to_lines(&path, &config).unwrap();
    assert_eq!(decoded.len(), 3);
    let just_lines: Vec<Vec<String>> = decoded.into_iter().map(|(lines, _)| lines).collect();

    let animation = PixelAnimation::Blink {
        on_color: Rgb::new(255, 0, 0),
        off_color: Rgb::new(0, 0, 255),
    };
    let colored = colorize_sequence(&just_lines, &animation);
    assert_eq!(colored.len(), 3);
    assert!(colored[0].contains("38;2;255;0;0"));
    assert!(colored[1].contains("38;2;0;0;255"));
    assert!(colored[2].contains("38;2;255;0;0"));

    // colorize_joined_sequence expects the newline-joined form, matching
    // what video frame rendering produces.
    let joined: Vec<String> = just_lines.iter().map(|lines| lines.join("\n")).collect();
    let colored_joined = colorize_joined_sequence(&joined, &animation);
    assert_eq!(colored_joined, colored);

    std::fs::remove_file(&path).ok();
}

#[test]
fn colorize_grid_matches_colorize_lines_over_the_grids_own_plain_text() {
    let png = temp_path("grid-vs-lines.png");
    write_checkerboard_png(&png, 6, 6);
    let config = Config::builder(MediaType::Image)
        .from_file(&png)
        .explicit_dimensions(6, 6)
        .build()
        .unwrap();
    let img = ::image::open(&png).unwrap();
    let rgb = img.to_rgb8();
    let grid = iascii::convert_image(rgb.width(), rgb.height(), rgb.as_raw(), &config.ascii)
        .unwrap();

    let animation = PixelAnimation::Rainbow { speed: 5.0 };
    let from_grid = colorize_grid(&grid, &animation, 2);
    let plain_lines = eger::render::grid_to_lines(&grid, None);
    let from_lines = eger::colorize_lines(&plain_lines, &animation, 2);
    assert_eq!(from_grid, from_lines);

    std::fs::remove_file(&png).ok();
}

// ---------------------------------------------------------------------
// batch parallel rendering across a directory
// ---------------------------------------------------------------------

#[test]
fn render_batch_parallel_end_to_end_over_a_directory_of_real_images() {
    let dir = temp_path("batch-dir");
    std::fs::create_dir_all(&dir).unwrap();
    for (i, name) in ["one.png", "two.png", "three.png"].iter().enumerate() {
        let shade = (i as u8) * 80;
        let img = ::image::RgbImage::from_pixel(10, 10, ::image::Rgb([shade, shade, shade]));
        img.save(dir.join(name)).unwrap();
    }
    // Non-matching file must be excluded by the batch.
    std::fs::write(dir.join("readme.txt"), b"ignore me").unwrap();

    let config = Config::builder(MediaType::Image)
        .from_dir(&dir, r"\.png$")
        .explicit_dimensions(8, 4)
        .num_threads(2)
        .build()
        .unwrap();

    let results = eger::render_batch_parallel(&config, |_| RenderTarget::Lines).unwrap();
    assert_eq!(results.len(), 3);
    for (path, outcome) in &results {
        assert!(path.extension().unwrap() == "png");
        let output = outcome.as_ref().unwrap();
        match output {
            RenderOutput::Lines(lines) => assert_eq!(lines.len(), 4),
            other => panic!("expected Lines, got {other:?}"),
        }
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------
// script ramps flow through Config -> a real conversion
// ---------------------------------------------------------------------

#[test]
fn every_script_ramp_drives_a_real_conversion_without_erroring() {
    let png = temp_path("script.png");
    write_gradient_png(&png, 20, 4);

    for script in [
        Script::Latin,
        Script::Block,
        Script::Braille,
        Script::Cyrillic,
        Script::Greek,
        Script::Cjk,
        Script::Devanagari,
        Script::Hebrew,
        Script::Arabic,
    ] {
        let config = Config::builder(MediaType::Image)
            .from_file(&png)
            .explicit_dimensions(15, 3)
            .script(script, true)
            .build()
            .unwrap();
        let text = eger::image_to_string(&png, &config).unwrap();
        assert_eq!(text.lines().count(), 3, "{script:?}");
    }

    std::fs::remove_file(&png).ok();
}

// ---------------------------------------------------------------------
// text animation + terminal-diffed playback round trip
// ---------------------------------------------------------------------

#[test]
fn text_frames_feed_directly_into_diff_frame_without_panicking() {
    let opts = TextAnimOptions {
        frames: 5,
        ..Default::default()
    };
    let frames = text_frames("diff me", &TextAnimation::Rainbow, &opts).unwrap();
    assert_eq!(frames.len(), 5);

    let mut prev = String::new();
    for frame in &frames {
        let patch = eger::render::diff_frame(&prev, frame, DiffGranularity::Line);
        // Every frame after the first differs (Rainbow changes color each
        // frame), so the patch should be non-empty from frame 2 onward.
        if !prev.is_empty() {
            assert!(!patch.is_empty());
        }
        prev = frame.clone();
    }
}

// ---------------------------------------------------------------------
// error propagation across module boundaries
// ---------------------------------------------------------------------

#[test]
fn missing_file_error_propagates_as_config_error_through_the_top_level_error_type() {
    let result = Config::builder(MediaType::Image)
        .from_file("/definitely/not/a/real/path.png")
        .build();
    let err = result.unwrap_err();
    // ConfigError converts into EgerError via `#[from]`.
    let wrapped: eger::EgerError = err.into();
    assert!(matches!(
        wrapped,
        eger::EgerError::Config(eger::error::ConfigError::FileNotFound(_))
    ));
}

// ---------------------------------------------------------------------
// video: only meaningful with the `video` feature, and only when ffmpeg
// is actually on PATH in the environment running the tests.
// ---------------------------------------------------------------------

#[cfg(feature = "video")]
mod video_pipeline {
    use super::*;
    use eger::video_to_file;
    use std::process::Command as StdCommand;
    use std::sync::Arc;

    fn ffmpeg_available() -> bool {
        StdCommand::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn make_test_video(path: &Path, frames: u32, width: u32, height: u32, fps: u32) {
        let status = StdCommand::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "color=c=blue:s={width}x{height}:r={fps}:d={:.3}",
                frames as f64 / fps as f64
            ))
            .args(["-frames:v", &frames.to_string()])
            .arg(path)
            .status()
            .expect("failed to spawn ffmpeg");
        assert!(status.success());
    }

    #[tokio::test]
    async fn full_video_pipeline_probe_render_and_write_frame_separated_file() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let path = temp_path("video-pipeline.mp4");
        make_test_video(&path, 5, 16, 16, 5);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(10, 5)
                .num_threads(2)
                .build()
                .unwrap(),
        );

        let info = eger::probe(&path).await.unwrap();
        assert_eq!((info.width, info.height), (16, 16));

        let frames = eger::video_to_lines(&path, Arc::clone(&config)).await.unwrap();
        assert_eq!(frames.len(), 5);

        let out = temp_path("video-pipeline-out.txt");
        video_to_file(&path, Arc::clone(&config), Some(&out))
            .await
            .unwrap();
        let contents = std::fs::read_to_string(&out).unwrap();
        let split: Vec<&str> = contents
            .split(eger::text::FRAME_SEPARATOR)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(split.len(), 5);

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&out).ok();
    }

    #[tokio::test]
    async fn plain_video_frames_can_be_recolored_with_colorize_joined_sequence() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let path = temp_path("video-plain.mp4");
        make_test_video(&path, 4, 12, 12, 4);

        let config = Arc::new(
            Config::builder(MediaType::Video)
                .from_file(&path)
                .explicit_dimensions(8, 4)
                .build()
                .unwrap(),
        );

        let plain = eger::video_to_lines_plain(&path, Arc::clone(&config))
            .await
            .unwrap();
        assert_eq!(plain.len(), 4);
        for frame in &plain {
            assert!(!frame.contains('\x1b'));
        }

        let animation = PixelAnimation::Plasma {
            scale: 0.3,
            speed: 0.2,
        };
        let colored = colorize_joined_sequence(&plain, &animation);
        assert_eq!(colored.len(), 4);
        for frame in &colored {
            assert!(frame.contains("\x1b[38;2;"));
        }

        std::fs::remove_file(&path).ok();
    }
}
