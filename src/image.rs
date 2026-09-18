//! Image → ASCII rendering, with single-file, Rayon-parallel batch, and
//! Tokio-async entry points.
//!
//! Two independent kinds of Rayon parallelism are in play here, and it's
//! worth being explicit about which is which:
//! - **Across files/frames** ([`render_batch_parallel`],
//!   [`crate::video::stream_video_frames`]): always on, doesn't need the
//!   `parallel` feature — this is plain `rayon::prelude::*` (`par_iter`)
//!   applied to a `Vec` of paths/frames this crate already owns.
//! - **Within one conversion** ([`convert`], gated behind the `parallel`
//!   feature and forwarded to `iascii/parallel`'s `convert_with_pool`):
//!   splits a single image/frame's *rows* across the pool. Every
//!   conversion call site in this crate goes through [`convert`], so
//!   whether a given call gets this row-level split is entirely
//!   `iascii::config::Config::parallel_threshold`'s call, not something
//!   decided here.
//!
//! Note: this module refers to the external `image` crate via a leading
//! `::image::...` path throughout, since the module itself is named `image`.

use crate::config::Config;
use crate::error::{EgerError, Result};
use crate::render::{self, RenderOutput, RenderTarget};
use ::image::AnimationDecoder;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
#[cfg(feature = "video")]
use std::sync::Arc;
use std::time::Duration;

/// Converts one RGB8 buffer to an ASCII/ANSI [`iascii::grid::Grid`], the
/// shared entry point every image/frame conversion in this crate goes
/// through — so there is exactly one place that decides *how* a single
/// buffer gets converted, even though several call sites reuse it under
/// different outer contexts (a lone image, a batch item, a video/GIF
/// frame).
///
/// With the `parallel` feature (on by default — see `Cargo.toml`), this
/// routes through [`iascii::convert_with_pool`] using `config`'s cached
/// Rayon pool, giving row-level parallelism *within* one conversion.
/// iascii applies `config.ascii.parallel_threshold` internally, so a small
/// grid transparently still takes the sequential path — this never adds
/// pool overhead for a 40-row thumbnail, only for a genuinely large single
/// conversion.
///
/// Without `parallel`, this is exactly [`iascii::convert_image`] — no pool
/// is built or touched at all for pure sequential/single-image use, which
/// matters for e.g. `image_to_string` callers who never wanted a thread
/// pool in the first place.
///
/// Note on nested use: [`render_batch_parallel`] and
/// [`crate::video::stream_video_frames`] already parallelize *across*
/// files/frames on this same cached pool before calling down into this
/// function. Calling `pool.install` again from inside a job already
/// running on that pool (as `convert_with_pool` does) is an explicitly
/// supported, non-deadlocking Rayon pattern — it just executes inline
/// rather than blocking on an idle pool — so nesting is safe, and
/// self-limiting in practice: most individual files/frames in a batch
/// don't clear `parallel_threshold` on their own, so the inner path only
/// actually engages for an outsized single item.
#[cfg(feature = "parallel")]
pub(crate) fn convert(
    width: u32,
    height: u32,
    buf: &[u8],
    config: &Config,
) -> Result<iascii::grid::Grid> {
    let pool = config.thread_pool()?;
    Ok(iascii::convert_with_pool(
        width,
        height,
        buf,
        &config.ascii,
        Some(pool.as_ref()),
    )?)
}

#[cfg(not(feature = "parallel"))]
pub(crate) fn convert(
    width: u32,
    height: u32,
    buf: &[u8],
    config: &Config,
) -> Result<iascii::grid::Grid> {
    Ok(iascii::convert_image(width, height, buf, &config.ascii)?)
}

/// Renders a single image file to `target` using `config`.
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
    let grid = convert(width, height, rgb.as_raw(), config)?;
    render::dispatch(target, &grid, Some(config.color_depth))
}

/// Async wrapper around [`render_image_file`].
#[cfg(feature = "video")]
pub async fn render_image_file_async(
    path: PathBuf,
    config: Arc<Config>,
    target: RenderTarget,
) -> Result<RenderOutput> {
    let raw_bytes = tokio::fs::read(&path).await?;

    tokio::task::spawn_blocking(move || {
        let img = ::image::load_from_memory(&raw_bytes)?;
        let rgb = img.to_rgb8();
        let width = rgb.width();
        let height = rgb.height();
        let grid = convert(width, height, rgb.as_raw(), &config)?;

        render::dispatch(&target, &grid, Some(config.color_depth))
    })
    .await?
}

/// Renders every file resolved by `config.files()` in parallel on a
/// dedicated Rayon thread pool sized by `config.num_threads`.
pub fn render_batch_parallel(
    config: &Config,
    target_for: impl Fn(&Path) -> RenderTarget + Sync,
) -> Result<Vec<(PathBuf, Result<RenderOutput>)>> {
    let files = config.files()?;
    let pool = config.thread_pool()?;

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

/// Async batch entrypoint.
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

/// Convenience: render one image straight to `Vec<String>`.
pub fn image_to_lines(path: &Path, config: &Config) -> Result<Vec<String>> {
    match render_image_file(path, config, &RenderTarget::Lines)? {
        RenderOutput::Lines(lines) => Ok(lines),
        _ => unreachable!("RenderTarget::Lines always yields RenderOutput::Lines"),
    }
}

/// Decodes every frame of an animated GIF at `path` and renders each one to
/// ASCII lines, alongside that frame's display delay.
///
/// Reuses the same [`Config`]/`iascii` conversion pipeline as still images
/// (`render_dynamic_image`), so all the usual sizing/ramp/luminance/color
/// settings apply per-frame. This decodes and converts every frame eagerly
/// and holds them all in memory at once — fine for typical GIFs, but for a
/// very large one prefer streaming frame-by-frame the way
/// [`crate::video::stream_video_frames`] does for video (not implemented
/// here, since `image`'s GIF decoder doesn't expose an easy async/streaming
/// frame source the way `ffmpeg`'s piped raw video does).
pub fn gif_to_lines(path: &Path, config: &Config) -> Result<Vec<(Vec<String>, Duration)>> {
    let file = std::fs::File::open(path)?;
    let decoder = ::image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))
        .map_err(EgerError::Image)?;

    decoder
        .into_frames()
        .map(|frame| {
            let frame = frame.map_err(EgerError::Image)?;
            let (numer, denom) = frame.delay().numer_denom_ms();
            let delay = Duration::from_millis(numer as u64 / (denom.max(1)) as u64);

            let buffer = frame.into_buffer();
            let dynamic = ::image::DynamicImage::ImageRgba8(buffer);
            let lines = match render_dynamic_image(&dynamic, config, &RenderTarget::Lines)? {
                RenderOutput::Lines(lines) => lines,
                _ => unreachable!("RenderTarget::Lines always yields RenderOutput::Lines"),
            };
            Ok((lines, delay))
        })
        .collect()
}

/// Convenience: render one image straight to a file.
pub fn image_to_file(path: &Path, config: &Config, output: Option<&Path>) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .or_else(|| config.output.clone())
        .ok_or_else(|| EgerError::Render("no output path provided or configured".into()))?;

    match render_image_file(path, config, &RenderTarget::File(output))? {
        RenderOutput::Written(path) => Ok(path),
        _ => unreachable!("RenderTarget::File always yields RenderOutput::Written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MediaType;
    use ::image::codecs::gif::{GifEncoder, Repeat};
    use ::image::{Delay, Frame, RgbaImage};

    fn write_test_gif(path: &Path) {
        let mut file = std::fs::File::create(path).unwrap();
        let mut encoder = GifEncoder::new(&mut file);
        encoder.set_repeat(Repeat::Infinite).unwrap();

        // Two 2x2 frames: solid black, then solid white, each with a
        // distinct delay so we can assert both frame count and timing.
        let black = RgbaImage::from_pixel(2, 2, ::image::Rgba([0, 0, 0, 255]));
        let white = RgbaImage::from_pixel(2, 2, ::image::Rgba([255, 255, 255, 255]));
        encoder
            .encode_frame(Frame::from_parts(
                black,
                0,
                0,
                Delay::from_numer_denom_ms(50, 1),
            ))
            .unwrap();
        encoder
            .encode_frame(Frame::from_parts(
                white,
                0,
                0,
                Delay::from_numer_denom_ms(120, 1),
            ))
            .unwrap();
    }

    fn tempfile(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("eger-image-test-{}-{name}", std::process::id()));
        path
    }

    #[test]
    fn convert_produces_a_grid_with_the_requested_dimensions() {
        let path = tempfile("convert-dummy.png");
        std::fs::write(&path, b"").unwrap();
        let config = Config::builder(MediaType::Image)
            .from_file(&path)
            .explicit_dimensions(3, 3)
            .build()
            .unwrap();

        // 3x3 solid-red RGB8 buffer.
        let buf = vec![255u8, 0, 0].repeat(9);
        let grid = convert(3, 3, &buf, &config).unwrap();
        assert_eq!(grid.width(), 3);
        assert_eq!(grid.height(), 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn convert_handles_a_grid_large_enough_to_cross_the_parallel_threshold() {
        // iascii's default `parallel_threshold` is 100 rows; ask for well
        // past that so the `parallel`-feature path actually exercises
        // `convert_with_pool`'s pool-engaged branch rather than always
        // silently falling back to sequential.
        let path = tempfile("convert-large-dummy.png");
        std::fs::write(&path, b"").unwrap();
        let config = Config::builder(MediaType::Image)
            .from_file(&path)
            .explicit_dimensions(40, 150)
            .build()
            .unwrap();

        let buf = vec![128u8; 10 * 10 * 3]; // tiny 10x10 source, upsampled to 40x150
        let grid = convert(10, 10, &buf, &config).unwrap();
        assert_eq!(grid.height(), 150);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn render_batch_parallel_with_an_oversized_item_does_not_deadlock() {
        // Regression test for the nested-parallelism claim in `convert`'s
        // doc comment: render_batch_parallel already runs each file's
        // conversion inside `pool.install(|| files.into_par_iter()...)`;
        // this asserts that a batch item whose own grid is large enough to
        // additionally trigger `convert`'s inner `convert_with_pool` path
        // still completes (rather than hanging) when both levels share the
        // same underlying pool.
        let dir =
            std::env::temp_dir().join(format!("eger-nested-parallel-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.png", "b.png"] {
            let img = ::image::RgbImage::from_pixel(4, 4, ::image::Rgb([200, 50, 50]));
            img.save(dir.join(name)).unwrap();
        }

        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .explicit_dimensions(30, 120) // clears the default 100-row threshold
            .num_threads(2)
            .build()
            .unwrap();

        let results = render_batch_parallel(&config, |_| RenderTarget::Lines).unwrap();
        assert_eq!(results.len(), 2);
        for (_, outcome) in results {
            let output = outcome.unwrap();
            match output {
                RenderOutput::Lines(lines) => assert_eq!(lines.len(), 120),
                other => panic!("expected Lines, got {other:?}"),
            }
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gif_to_lines_decodes_every_frame_with_its_own_delay() {
        let path = tempfile("two-frame.gif");
        write_test_gif(&path);

        let config = Config::builder(MediaType::Image)
            .from_file(&path)
            .explicit_dimensions(2, 2)
            .build()
            .unwrap();

        let frames = gif_to_lines(&path, &config).unwrap();
        assert_eq!(frames.len(), 2, "should decode both encoded frames");
        assert_eq!(frames[0].1, Duration::from_millis(50));
        assert_eq!(frames[1].1, Duration::from_millis(120));
        for (lines, _) in &frames {
            assert_eq!(
                lines.len(),
                2,
                "explicit_dimensions height should be honored"
            );
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn gif_to_lines_errors_on_a_missing_file() {
        let config = Config::builder(MediaType::Image)
            .from_file(tempfile("does-not-exist.gif"))
            .build();
        // The file doesn't exist, so Config::build itself should already
        // reject it before we ever get to gif_to_lines.
        assert!(config.is_err());
    }
}
