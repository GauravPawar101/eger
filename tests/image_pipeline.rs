//! Black-box integration tests for the image → ASCII pipeline: only
//! `eger`'s public API (as re-exported from `eger::prelude`) is used,
//! the way an external consumer of the crate would use it.

use eger::prelude::*;
use std::path::PathBuf;

fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "eger-it-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn write_checkerboard_png(path: &std::path::Path, width: u32, height: u32) {
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        if (x / 2 + y / 2) % 2 == 0 {
            image::Rgb([250, 250, 250])
        } else {
            image::Rgb([5, 5, 5])
        }
    });
    img.save(path).expect("failed to write fixture PNG");
}

#[test]
fn end_to_end_image_to_string_via_public_builder() {
    let input = temp_path("input.png");
    write_checkerboard_png(&input, 40, 30);

    let config = Config::builder(MediaType::Image)
        .from_file(&input)
        .max_width(20)
        .color_depth(ColorDepth::TrueColor)
        .build()
        .expect("config should build for a real file");

    let ascii = eger::image_to_string(&input, &config).expect("rendering should succeed");
    assert!(!ascii.is_empty());
    // A checkerboard should render more than one distinct character.
    assert!(
        ascii
            .chars()
            .filter(|c| *c != '\n')
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1
    );

    std::fs::remove_file(&input).ok();
}

#[test]
fn end_to_end_image_to_file_round_trip() {
    let input = temp_path("input.png");
    let output = temp_path("output.txt");
    write_checkerboard_png(&input, 24, 24);

    let config = Config::builder(MediaType::Image)
        .from_file(&input)
        .max_width(12)
        .output(&output)
        .build()
        .unwrap();

    let written = eger::image_to_file(&input, &config, None).unwrap();
    assert_eq!(written, output);
    let contents = std::fs::read_to_string(&output).unwrap();
    assert!(!contents.is_empty());

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

#[test]
fn batch_directory_pipeline_processes_all_matching_files_in_parallel() {
    let dir = temp_path("batch");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..8 {
        write_checkerboard_png(&dir.join(format!("frame_{i:03}.png")), 16, 16);
    }
    // A file that should be excluded by the pattern.
    std::fs::write(dir.join("readme.txt"), b"not an image").unwrap();

    let config = Config::builder(MediaType::Image)
        .from_dir(&dir, r"\.png$")
        .max_width(8)
        .num_threads(4)
        .build()
        .unwrap();

    let results = eger::render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
    assert_eq!(results.len(), 8);
    for (path, outcome) in &results {
        assert!(
            outcome.is_ok(),
            "expected {path:?} to render successfully, got {outcome:?}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "video")]
#[tokio::test]
async fn async_batch_pipeline_matches_sync_batch_pipeline_file_count() {
    let dir = temp_path("batch_async");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..5 {
        write_checkerboard_png(&dir.join(format!("f{i}.png")), 12, 12);
    }

    let config = std::sync::Arc::new(
        Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .max_width(8)
            .build()
            .unwrap(),
    );

    let results = eger::render_batch_async(config, |_| RenderTarget::String)
        .await
        .unwrap();
    assert_eq!(results.len(), 5);
    for (_path, outcome) in &results {
        assert!(outcome.is_ok());
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nonexistent_directory_produces_folder_not_found_error() {
    let err = Config::builder(MediaType::Image)
        .from_dir("/this/path/should/not/exist/anywhere", ".*")
        .build()
        .unwrap_err();
    assert!(matches!(err, ConfigError::FolderNotFound(_)));
}

#[test]
fn empty_matching_set_surfaces_as_files_error_not_a_panic() {
    let dir = temp_path("empty_batch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("nope.gif"), b"stub").unwrap();

    let config = Config::builder(MediaType::Image)
        .from_dir(&dir, r"\.png$")
        .build()
        .unwrap();

    let err = config.files().unwrap_err();
    assert!(matches!(err, ConfigError::NoMatchingFiles { .. }));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn color_depth_setting_changes_output_encoding_end_to_end() {
    let input = temp_path("input.png");
    write_checkerboard_png(&input, 20, 20);

    let truecolor_config = Config::builder(MediaType::Image)
        .from_file(&input)
        .max_width(10)
        .color_depth(ColorDepth::TrueColor)
        .build()
        .unwrap();
    let ansi16_config = Config::builder(MediaType::Image)
        .from_file(&input)
        .max_width(10)
        .color_depth(ColorDepth::Ansi16)
        .build()
        .unwrap();

    let truecolor_out = eger::image_to_string(&input, &truecolor_config).unwrap();
    let ansi16_out = eger::image_to_string(&input, &ansi16_config).unwrap();

    // `image_to_string` (and every other convenience helper in `image.rs`)
    // always renders through `Some(config.color_depth)` — there's no plain,
    // uncolored path through the public single-image API. Both outputs
    // should therefore carry ANSI escapes, and different depths should
    // encode those escapes differently.
    assert!(truecolor_out.contains('\x1b'));
    assert!(ansi16_out.contains('\x1b'));
    assert_ne!(
        truecolor_out, ansi16_out,
        "TrueColor and Ansi16 configs should produce different escape encodings"
    );

    std::fs::remove_file(&input).ok();
}

#[test]
fn single_file_input_via_from_dir_pattern_matching_only_that_file() {
    let dir = temp_path("single_match_dir");
    std::fs::create_dir_all(&dir).unwrap();
    write_checkerboard_png(&dir.join("keep.png"), 10, 10);
    std::fs::write(dir.join("skip.jpg"), b"not a png").unwrap();

    let config = Config::builder(MediaType::Image)
        .from_dir(&dir, r"^keep\.png$")
        .max_width(5)
        .build()
        .unwrap();

    let files = config.files().unwrap();
    assert_eq!(files, vec![dir.join("keep.png")]);

    let results = eger::render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].1.is_ok());

    std::fs::remove_dir_all(&dir).ok();
}

/// Stress test: a larger batch across a constrained thread pool, checking
/// that parallel rendering doesn't drop, duplicate, or corrupt any frame
/// and that output ordering from `render_batch_parallel` covers every input
/// file exactly once regardless of scheduling order.
#[test]
fn stress_large_batch_with_constrained_thread_pool() {
    const N: usize = 64;
    let dir = temp_path("stress_batch");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..N {
        write_checkerboard_png(&dir.join(format!("img_{i:04}.png")), 20, 20);
    }

    let config = Config::builder(MediaType::Image)
        .from_dir(&dir, r"\.png$")
        .max_width(10)
        .num_threads(2) // deliberately fewer threads than files
        .build()
        .unwrap();

    let results = eger::render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
    assert_eq!(results.len(), N);

    let mut seen = std::collections::HashSet::new();
    for (path, outcome) in results {
        assert!(outcome.is_ok(), "{path:?} failed to render");
        assert!(
            seen.insert(path),
            "duplicate result for the same input file"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}
