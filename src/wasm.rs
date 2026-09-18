//! Thin WASM/browser wrapper around the terminal-independent parts of
//! `eger`'s conversion pipeline.
//!
//! This deliberately does **not** expose [`crate::config::Config`]/
//! [`crate::config::ConfigBuilder`] as-is: `Config::builder(..).from_file(..)`
//! /`from_dir(..)` assume a real filesystem, and [`crate::video`]'s whole
//! pipeline assumes a real `ffmpeg` process and a real terminal — none of
//! which exist in a browser. Instead, these functions take image bytes
//! already in memory (e.g. from a JS `<input type="file">` or a `fetch`
//! response) and talk to `iascii` directly.
//!
//! Playback timing is also left to JS: [`text_frame_at`] renders one frame
//! at a time so the caller can drive it from `requestAnimationFrame` or
//! `setInterval`, instead of [`crate::text::play_text`]'s blocking
//! `std::thread::sleep` loop, which has no equivalent on `wasm32` (there is
//! no OS thread to block without freezing the page).
//!
//! Build with e.g.:
//! ```text
//! wasm-pack build --no-default-features --features wasm
//! ```

use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::render::ColorDepth;
use wasm_bindgen::prelude::*;

/// Converts raw image bytes (any format the `image` crate can decode —
/// PNG, JPEG, GIF (first frame only; see [`gif_bytes_to_frame_strings`] for
/// all frames), WebP, BMP, ...) into a single ANSI/plain-text string, ready
/// to drop into a `<pre>` tag.
///
/// `max_width` caps the output width, deriving height from the source
/// image's aspect ratio; pass `0` to use `iascii`'s own default sizing.
/// `color` selects ANSI-colored (truecolor) output; pass `false` for plain
/// characters with no escape codes, which is both safer to assign to
/// `textContent` and cheaper for the browser to diff than `innerHTML`.
#[wasm_bindgen]
pub fn image_bytes_to_string(
    bytes: &[u8],
    max_width: usize,
    color: bool,
) -> Result<String, JsError> {
    let grid = decode_and_convert(bytes, max_width)?;
    let depth = color.then_some(ColorDepth::TrueColor);
    Ok(crate::render::grid_to_string(&grid, depth))
}

/// Same as [`image_bytes_to_string`], but returns one `String` per output
/// row instead of a single newline-joined blob — convenient for rendering
/// one DOM node per line instead of reflowing an entire `<pre>` on update.
#[wasm_bindgen]
pub fn image_bytes_to_lines(
    bytes: &[u8],
    max_width: usize,
    color: bool,
) -> Result<Vec<JsValue>, JsError> {
    let grid = decode_and_convert(bytes, max_width)?;
    let depth = color.then_some(ColorDepth::TrueColor);
    Ok(crate::render::grid_to_lines(&grid, depth)
        .into_iter()
        .map(|line| JsValue::from_str(&line))
        .collect())
}

/// Decodes every frame of an animated GIF's bytes and renders each one to
/// a single ANSI/plain-text string, alongside its delay in milliseconds —
/// the browser-friendly equivalent of [`crate::image::gif_to_lines`] (which
/// takes a filesystem path and isn't available on `wasm32`).
///
/// The caller is expected to drive playback itself (e.g. with
/// `setInterval`/`requestAnimationFrame`), using the returned delays.
#[wasm_bindgen]
pub fn gif_bytes_to_frame_strings(
    bytes: &[u8],
    max_width: usize,
    color: bool,
) -> Result<Vec<JsValue>, JsError> {
    use ::image::{codecs::gif::GifDecoder, AnimationDecoder};

    let decoder =
        GifDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| JsError::new(&e.to_string()))?;
    let depth = color.then_some(ColorDepth::TrueColor);
    // Built once and reused for every frame — `sized_ascii_config` is a
    // pure function of `max_width` alone, so there's no reason to pay for
    // rebuilding an identical `iascii` config on every single GIF frame.
    let ascii_config = sized_ascii_config(max_width)?;

    let mut out = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| JsError::new(&e.to_string()))?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = numer as u64 / denom.max(1) as u64;

        let dynamic = ::image::DynamicImage::ImageRgba8(frame.into_buffer());
        let rgb = dynamic.to_rgb8();
        let grid =
            iascii::convert::convert_image(rgb.width(), rgb.height(), rgb.as_raw(), &ascii_config)
                .map_err(|e| JsError::new(&e.to_string()))?;
        let text = crate::render::grid_to_string(&grid, depth);

        out.push(JsValue::from_str(&format!("{delay_ms}\u{1e}{text}")));
    }
    Ok(out)
}

/// Renders a single frame of a built-in [`crate::text::TextAnimation`],
/// for callers driving their own timing loop in JS instead of using
/// [`crate::text::play_text`]'s blocking playback (which isn't usable on
/// `wasm32`). Frame indices are 0-based; pass whatever `TextAnimOptions`
/// you'd otherwise use except `frames`, which is derived from
/// `frame_index` so only that one frame is generated.
///
/// Custom (`Program`/`Function`) animations aren't exposed here: `Program`
/// spawns a native subprocess, which has no `wasm32` equivalent, and a
/// Rust-closure `Function` can't cross the `wasm-bindgen` boundary from JS.
/// Drive those from JS directly instead.
#[wasm_bindgen]
pub fn text_frame_at(text: &str, animation: TextAnimationKind, frame_index: u32) -> String {
    use crate::text::{Rgb, TextAnimOptions, TextAnimation};

    let anim = match animation {
        TextAnimationKind::Typewriter => TextAnimation::Typewriter,
        TextAnimationKind::Rainbow => TextAnimation::Rainbow,
        TextAnimationKind::Wave => TextAnimation::Wave,
        TextAnimationKind::Pulse => TextAnimation::Pulse,
        TextAnimationKind::Blink => TextAnimation::Blink {
            on_color: Rgb::WHITE,
            off_color: Rgb::new(0, 0, 0),
        },
    };
    let opts = TextAnimOptions {
        frames: frame_index as u64 + 1,
        ..Default::default()
    };
    crate::text::text_frames(text, &anim, &opts)
        .ok()
        .and_then(|frames| frames.into_iter().last())
        .unwrap_or_default()
}

/// The built-in [`crate::text::TextAnimation`] styles that don't need extra
/// per-call configuration (colors, widths, ...) beyond what
/// [`text_frame_at`] hardcodes — a `wasm-bindgen`-compatible enum, since
/// the real [`crate::text::TextAnimation`] (with its `Blink`/`Marquee`
/// struct variants and `Custom` closures/subprocesses) can't cross the JS
/// boundary directly.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum TextAnimationKind {
    Typewriter,
    Rainbow,
    Wave,
    Blink,
    Pulse,
}

fn sized_ascii_config(max_width: usize) -> Result<iascii::config::Config, JsError> {
    let mut builder = AsciiConfigBuilder::new();
    if max_width > 0 {
        builder = builder.output_sizing(OutputSizing::MaxWidth(max_width));
    }
    builder.build().map_err(|e| JsError::new(&e.to_string()))
}

fn decode_and_convert(bytes: &[u8], max_width: usize) -> Result<iascii::grid::Grid, JsError> {
    let img = ::image::load_from_memory(bytes).map_err(|e| JsError::new(&e.to_string()))?;
    let rgb = img.to_rgb8();
    let ascii_config = sized_ascii_config(max_width)?;
    iascii::convert::convert_image(rgb.width(), rgb.height(), rgb.as_raw(), &ascii_config)
        .map_err(|e| JsError::new(&e.to_string()))
}
