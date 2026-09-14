//! Image → ASCII rendering, with single-file, Rayon-parallel batch, and
//! Tokio-async entry points.
//!
//! Note: this module refers to the external `image` crate via a leading
//! `::image::...` path throughout, since the module itself is named `image`.

use crate::config::Config;
use crate::error::{MaramuraError, Result};
use crate::render::{self, RenderOutput, RenderTarget};
use iascii::convert::convert_image;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
#[cfg(feature = "video")]
use std::sync::Arc;

/// Renders a single image file to `target` using `config`.
///
/// Synchronous and blocking (decodes + converts on the calling thread) — use
/// `render_image_file_async` (requires the `video` feature) from async code.
pub fn render_image_file(
    path: &Path,
    config: &Config,
    target: &RenderTarget,
) -> Result<RenderOutput> {
    let img = ::image::open(path)?;
    render_dynamic_image(&img, config, target)
}

/// Renders an already-decoded image to `target` using `config`.
pub fn render_dynamic_image(
    img: &::image::DynamicImage,
    config: &Config,
    target: &RenderTarget,
) -> Result<RenderOutput> {
    let rgb = img.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();
    let grid = convert_image(width, height, rgb.as_raw(), &config.ascii)?;
    render::dispatch(target, &grid, Some(config.color_depth))
}

/// Async wrapper around [`render_image_file`]. Decoding and ASCII conversion
/// are both CPU-bound, so the work runs on Tokio's blocking thread pool and
/// never stalls the async runtime.
///
/// Requires the `video` feature (which pulls in `tokio`).
#[cfg(feature = "video")]
pub async fn render_image_file_async(
    path: PathBuf,
    config: Arc<Config>,
    target: RenderTarget,
) -> Result<RenderOutput> {
    tokio::task::spawn_blocking(move || render_image_file(&path, &config, &target)).await?
}

/// Renders every file resolved by `config.files()` in parallel on a
/// dedicated Rayon thread pool sized by `config.num_threads`.
///
/// `target_for` chooses the [`RenderTarget`] per input path (e.g. mirror an
/// output directory, or always return `String`/`Lines`).
pub fn render_batch_parallel(
    config: &Config,
    target_for: impl Fn(&Path) -> RenderTarget + Sync,
) -> Result<Vec<(PathBuf, Result<RenderOutput>)>> {
    let files = config.files()?;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.num_threads)
        .build()
        .map_err(|e| MaramuraError::Render(e.to_string()))?;

    let results = pool.install(|| {
        files
            .into_par_iter()
            .map(|path| {
                let target = target_for(&path);
                let outcome = render_image_file(&path, config, &target);
                (path, outcome)
            })
            .collect::<Vec<_>>()
    });

    Ok(results)
}

/// Async batch entrypoint: spawns one Tokio task per file so per-file I/O
/// overlaps, while each task's CPU-bound decode + convert step still runs on
/// the blocking pool via [`render_image_file_async`].
///
/// Requires the `video` feature (which pulls in `tokio`).
#[cfg(feature = "video")]
pub async fn render_batch_async(
    config: Arc<Config>,
    target_for: impl Fn(&Path) -> RenderTarget + Send + Sync + 'static,
) -> Result<Vec<(PathBuf, Result<RenderOutput>)>> {
    let files = config.files()?;
    let target_for = Arc::new(target_for);
    let mut set = tokio::task::JoinSet::new();

    for path in files {
        let config = Arc::clone(&config);
        let target_for = Arc::clone(&target_for);
        set.spawn(async move {
            let target = target_for(&path);
            let outcome = render_image_file_async(path.clone(), config, target).await;
            (path, outcome)
        });
    }

    let mut results = Vec::with_capacity(set.len());
    while let Some(joined) = set.join_next().await {
        results.push(joined?);
    }
    Ok(results)
}

/// Convenience: render one image straight to an owned `String`.
pub fn image_to_string(path: &Path, config: &Config) -> Result<String> {
    match render_image_file(path, config, &RenderTarget::String)? {
        RenderOutput::Text(text) => Ok(text),
        _ => unreachable!("RenderTarget::String always yields RenderOutput::Text"),
    }
}

/// Convenience: render one image straight to `Vec<String>` (one entry per row).
pub fn image_to_lines(path: &Path, config: &Config) -> Result<Vec<String>> {
    match render_image_file(path, config, &RenderTarget::Lines)? {
        RenderOutput::Lines(lines) => Ok(lines),
        _ => unreachable!("RenderTarget::Lines always yields RenderOutput::Lines"),
    }
}

/// Convenience: render one image straight to a file — `output` if given,
/// otherwise `config.output`.
pub fn image_to_file(path: &Path, config: &Config, output: Option<&Path>) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .or_else(|| config.output.clone())
        .ok_or_else(|| MaramuraError::Render("no output path provided or configured".into()))?;

    match render_image_file(path, config, &RenderTarget::File(output))? {
        RenderOutput::Written(path) => Ok(path),
        _ => unreachable!("RenderTarget::File always yields RenderOutput::Written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, InputSource, MediaType};
    use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
    use iascii::render::ColorDepth;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "maramura-image-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    /// Writes a small deterministic RGB checkerboard PNG to a temp path and
    /// returns it.
    fn write_test_png(width: u32, height: u32) -> PathBuf {
        let path = temp_path("input.png");
        let img = ::image::RgbImage::from_fn(width, height, |x, y| {
            if (x + y) % 2 == 0 {
                ::image::Rgb([255, 255, 255])
            } else {
                ::image::Rgb([0, 0, 0])
            }
        });
        img.save(&path).expect("failed to write test fixture PNG");
        path
    }

    fn config_for(path: &Path) -> Config {
        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(8))
            .build()
            .unwrap();
        Config {
            media_type: MediaType::Image,
            source: InputSource::File(path.to_path_buf()),
            output: None,
            num_threads: 2,
            color_depth: ColorDepth::TrueColor,
            ascii,
        }
    }

    #[test]
    fn render_image_file_to_string_target_succeeds() {
        let path = write_test_png(16, 16);
        let config = config_for(&path);

        let out = render_image_file(&path, &config, &RenderTarget::String).unwrap();
        match out {
            RenderOutput::Text(t) => assert!(!t.is_empty()),
            other => panic!("expected Text, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn render_image_file_missing_file_errors() {
        let path = PathBuf::from("/definitely/not/a/real/image.png");
        let config = config_for(&path);
        let err = render_image_file(&path, &config, &RenderTarget::String).unwrap_err();
        assert!(matches!(err, MaramuraError::Image(_)));
    }

    #[test]
    fn image_to_string_matches_render_image_file() {
        let path = write_test_png(10, 10);
        let config = config_for(&path);

        let s1 = image_to_string(&path, &config).unwrap();
        match render_image_file(&path, &config, &RenderTarget::String).unwrap() {
            RenderOutput::Text(s2) => assert_eq!(s1, s2),
            _ => unreachable!(),
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn image_to_lines_returns_one_line_per_row() {
        let path = write_test_png(10, 10);
        let config = config_for(&path);

        let lines = image_to_lines(&path, &config).unwrap();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(!line.is_empty());
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn image_to_file_writes_output_and_returns_its_path() {
        let path = write_test_png(10, 10);
        let out_path = temp_path("output.txt");
        let config = config_for(&path);

        let written = image_to_file(&path, &config, Some(&out_path)).unwrap();
        assert_eq!(written, out_path);
        assert!(!std::fs::read_to_string(&out_path).unwrap().is_empty());

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&out_path).ok();
    }

    #[test]
    fn image_to_file_errors_when_no_output_path_available() {
        let path = write_test_png(4, 4);
        let config = config_for(&path); // config.output is None

        let err = image_to_file(&path, &config, None).unwrap_err();
        assert!(matches!(err, MaramuraError::Render(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn render_batch_parallel_processes_every_matching_file() {
        let dir = temp_path("batch_dir");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.png", "b.png", "c.png"] {
            let img = ::image::RgbImage::from_pixel(4, 4, ::image::Rgb([10, 20, 30]));
            img.save(dir.join(name)).unwrap();
        }
        std::fs::write(dir.join("ignore.txt"), b"not an image").unwrap();

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(4))
            .build()
            .unwrap();
        let config = Config {
            media_type: MediaType::Image,
            source: InputSource::Directory {
                dir: dir.clone(),
                pattern: regex::Regex::new(r"\.png$").unwrap(),
            },
            output: None,
            num_threads: 2,
            color_depth: ColorDepth::TrueColor,
            ascii,
        };

        let results = render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
        assert_eq!(results.len(), 3);
        for (_path, outcome) in &results {
            assert!(outcome.is_ok());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn render_batch_async_processes_every_matching_file() {
        let dir = temp_path("batch_dir_async");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.png", "b.png"] {
            let img = ::image::RgbImage::from_pixel(4, 4, ::image::Rgb([1, 2, 3]));
            img.save(dir.join(name)).unwrap();
        }

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(4))
            .build()
            .unwrap();
        let config = Arc::new(Config {
            media_type: MediaType::Image,
            source: InputSource::Directory {
                dir: dir.clone(),
                pattern: regex::Regex::new(r"\.png$").unwrap(),
            },
            output: None,
            num_threads: 2,
            color_depth: ColorDepth::TrueColor,
            ascii,
        });

        let results = render_batch_async(config, |_| RenderTarget::String)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        for (_path, outcome) in &results {
            assert!(outcome.is_ok());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_dynamic_image_matches_render_image_file_for_the_same_source() {
        let path = write_test_png(12, 12);
        let config = config_for(&path);

        let from_file = image_to_string(&path, &config).unwrap();

        let img = ::image::open(&path).unwrap();
        let from_memory = match render_dynamic_image(&img, &config, &RenderTarget::String).unwrap()
        {
            RenderOutput::Text(t) => t,
            other => panic!("expected Text, got {other:?}"),
        };

        assert_eq!(from_file, from_memory);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn render_dynamic_image_respects_explicit_output_dimensions() {
        let path = write_test_png(4, 4);
        let img = ::image::open(&path).unwrap();

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::Explicit {
                width: 3,
                height: 2,
            })
            .build()
            .unwrap();
        let config = Config {
            media_type: MediaType::Image,
            source: InputSource::File(path.clone()),
            output: None,
            num_threads: 1,
            color_depth: ColorDepth::TrueColor,
            ascii,
        };

        let lines = match render_dynamic_image(&img, &config, &RenderTarget::Lines).unwrap() {
            RenderOutput::Lines(lines) => lines,
            other => panic!("expected Lines, got {other:?}"),
        };

        assert_eq!(lines.len(), 2, "explicit height should be honored");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn image_to_file_prefers_explicit_output_over_config_output() {
        let path = write_test_png(8, 8);
        let mut config = config_for(&path);
        config.output = Some(temp_path("config-default-output.txt"));
        let explicit_out = temp_path("explicit-output.txt");

        let written = image_to_file(&path, &config, Some(&explicit_out)).unwrap();

        assert_eq!(written, explicit_out);
        assert!(explicit_out.exists());
        assert!(
            !config.output.as_ref().unwrap().exists(),
            "config.output must not be written to when an explicit path is given"
        );

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&explicit_out).ok();
    }

    #[test]
    fn image_to_file_falls_back_to_config_output_when_none_given() {
        let path = write_test_png(8, 8);
        let mut config = config_for(&path);
        let fallback_out = temp_path("fallback-output.txt");
        config.output = Some(fallback_out.clone());

        let written = image_to_file(&path, &config, None).unwrap();

        assert_eq!(written, fallback_out);
        assert!(fallback_out.exists());

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&fallback_out).ok();
    }

    #[test]
    fn render_batch_parallel_with_single_thread_still_processes_all_files() {
        let dir = temp_path("single_thread_batch");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["x.png", "y.png"] {
            let img = ::image::RgbImage::from_pixel(4, 4, ::image::Rgb([1, 1, 1]));
            img.save(dir.join(name)).unwrap();
        }

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(4))
            .build()
            .unwrap();
        let config = Config {
            media_type: MediaType::Image,
            source: InputSource::Directory {
                dir: dir.clone(),
                pattern: regex::Regex::new(r"\.png$").unwrap(),
            },
            output: None,
            num_threads: 1,
            color_depth: ColorDepth::TrueColor,
            ascii,
        };

        let results = render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|(_, outcome)| outcome.is_ok()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_batch_parallel_propagates_per_file_errors_without_failing_whole_batch() {
        // Simulate one bad file among good ones by pointing the batch's
        // resolved file list at a mix of a real image and a corrupt one.
        // `render_batch_parallel` resolves files via `config.files()`, so we
        // drive this through a real directory containing one non-image file
        // that nonetheless matches the pattern.
        let dir = temp_path("mixed_batch");
        std::fs::create_dir_all(&dir).unwrap();
        let good = ::image::RgbImage::from_pixel(4, 4, ::image::Rgb([5, 5, 5]));
        good.save(dir.join("good.png")).unwrap();
        std::fs::write(dir.join("bad.png"), b"not a real png").unwrap();

        let ascii = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::MaxWidth(4))
            .build()
            .unwrap();
        let config = Config {
            media_type: MediaType::Image,
            source: InputSource::Directory {
                dir: dir.clone(),
                pattern: regex::Regex::new(r"\.png$").unwrap(),
            },
            output: None,
            num_threads: 2,
            color_depth: ColorDepth::TrueColor,
            ascii,
        };

        let results = render_batch_parallel(&config, |_| RenderTarget::String).unwrap();
        assert_eq!(results.len(), 2);

        let mut saw_ok = false;
        let mut saw_err = false;
        for (path, outcome) in &results {
            if path.ends_with("good.png") {
                assert!(outcome.is_ok());
                saw_ok = true;
            } else if path.ends_with("bad.png") {
                assert!(matches!(outcome, Err(MaramuraError::Image(_))));
                saw_err = true;
            }
        }
        assert!(saw_ok && saw_err, "expected one success and one failure");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn render_image_file_async_matches_sync_version() {
        let path = write_test_png(6, 6);
        let config = Arc::new(config_for(&path));

        let sync_out = image_to_string(&path, &config).unwrap();
        let async_out =
            render_image_file_async(path.clone(), Arc::clone(&config), RenderTarget::String)
                .await
                .unwrap();
        match async_out {
            RenderOutput::Text(t) => assert_eq!(t, sync_out),
            other => panic!("expected Text, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
