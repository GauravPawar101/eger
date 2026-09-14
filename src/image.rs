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
    let raw_bytes = tokio::fs::read(&path).await?;

    tokio::task::spawn_blocking(move || {
        let img = ::image::load_from_memory(&raw_bytes)?;
        let rgb = img.to_rgb8();
        let width = rgb.width();
        let height = rgb.height();

        let mut builder = rayon::ThreadPoolBuilder::new();
        if config.num_threads > 0 {
            builder = builder.num_threads(config.num_threads);
        }
        let pool = builder
            .build()
            .map_err(|e| MaramuraError::Render(e.to_string()))?;

        let grid = pool.install(|| convert_image(width, height, rgb.as_raw(), &config.ascii))?;

        render::dispatch(&target, &grid, Some(config.color_depth))
    })
    .await?
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
    // Tests retained verbatim
}
