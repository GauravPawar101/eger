//! Thin WASM/browser wrapper around the terminal-independent parts of
//! `eger`'s conversion pipeline: images, dithering, ANSI-16/256/TrueColor
//! selection, non-Latin character ramps ([`crate::script`]), per-cell
//! color animation ([`crate::colorize`]) and color transforms
//! ([`crate::color`]), block-letter banners ([`crate::banner`]), and
//! procedurally generated illusions ([`crate::illusions`]).
//!
//! This deliberately does **not** expose [`crate::config::Config`]/
//! [`crate::config::ConfigBuilder`] as-is: `Config::builder(..).from_file(..)`
//! /`from_dir(..)` assume a real filesystem, and [`crate::video`]'s whole
//! pipeline assumes a real `ffmpeg` process and a real terminal — none of
//! which exist in a browser. Instead, these functions take image bytes
//! already in memory (e.g. from a JS `<input type="file">` or a `fetch`
//! response) and talk to `iascii` directly.
//!
//! Playback timing is also left to JS: [`text_frame_at`]/[`banner_frame_at`]
//! render one frame at a time so the caller can drive it from
//! `requestAnimationFrame` or `setInterval`, instead of
//! [`crate::text::play_text`]'s blocking `std::thread::sleep` loop, which
//! has no equivalent on `wasm32` (there is no OS thread to block without
//! freezing the page).
//!
//! # Color depth and dithering
//!
//! [`ColorDepthKind`] stands in for `Option<iascii::render::ColorDepth>`
//! across this whole module (`Plain` maps to `None` — no ANSI escapes at
//! all — and the other three map onto `iascii::render::ColorDepth`
//! directly), so every function that renders color lets a caller pick
//! `Ansi16`/`Ansi256` for a coarser, more compatible terminal-like palette
//! or `TrueColor` for full 24-bit color, not just an on/off `bool` the way
//! the original [`image_bytes_to_string`]/[`image_bytes_to_lines`] still
//! do (kept as-is for backward compatibility — prefer the `_ex` variants
//! below in new code). [`crate::dither::DitherMethod`] is itself directly
//! `#[wasm_bindgen]`-exported (it was already a plain fieldless enum), so
//! it's passed straight through to the `_ex` functions without another
//! wrapper type.
//!
//! # Color transforms
//!
//! [`ColorTransformKind`] + a single `f32` `transform_amount` stands in
//! for [`crate::color::ColorTransform`] (`amount` is interpreted
//! per-kind — see [`build_transform`] — and ignored by kinds that don't
//! need it). Only one transform can be selected per call from JS (no
//! `Compose` here — build one composed transform in Rust and add a
//! bespoke binding if you need that); this covers the common
//! single-filter case (a "grayscale preview" toggle, a brightness slider,
//! ...) without needing a richer struct/array marshaled across the JS
//! boundary.
//!
//! Build with e.g.:
//! ```text
//! wasm-pack build --no-default-features --features wasm
//! ```

use crate::color::ColorTransform;
use crate::dither::{DitherMethod, DitherOptions};
use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
use iascii::grid::Grid;
use iascii::render::ColorDepth;
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------
// Original (bool-color) image bindings — kept for backward compatibility.
// Prefer the ColorDepthKind-based `_ex` functions below in new code.
// ---------------------------------------------------------------------

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
///
/// Only offers on/off TrueColor — see [`image_bytes_to_string_ex`] for
/// ANSI-16/256 support and optional dithering/color-transform.
#[wasm_bindgen]
pub fn image_bytes_to_string(
    bytes: &[u8],
    max_width: usize,
    color: bool,
) -> Result<String, JsError> {
    let grid = decode_and_convert(bytes, max_width, None)?;
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
    let grid = decode_and_convert(bytes, max_width, None)?;
    let depth = color.then_some(ColorDepth::TrueColor);
    Ok(strings_to_js(crate::render::grid_to_lines(&grid, depth)))
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
    let ascii_config = sized_ascii_config(max_width, None)?;

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

// ---------------------------------------------------------------------
// ColorDepthKind-aware image bindings: ANSI-16/256/TrueColor + dithering
// + an optional single color transform, all in one call.
// ---------------------------------------------------------------------

/// Stands in for `Option<iascii::render::ColorDepth>` across this module
/// — see the module docs' "Color depth and dithering" section.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepthKind {
    /// No ANSI escapes at all — plain characters only.
    Plain,
    /// The standard 16-color ANSI palette (`iascii::render::ColorDepth::Ansi16`).
    Ansi16,
    /// The xterm 256-color palette (`iascii::render::ColorDepth::Ansi256`).
    Ansi256,
    /// Full 24-bit color (`iascii::render::ColorDepth::TrueColor`).
    TrueColor,
}

impl ColorDepthKind {
    fn to_iascii(self) -> Option<ColorDepth> {
        match self {
            ColorDepthKind::Plain => None,
            ColorDepthKind::Ansi16 => Some(ColorDepth::Ansi16),
            ColorDepthKind::Ansi256 => Some(ColorDepth::Ansi256),
            ColorDepthKind::TrueColor => Some(ColorDepth::TrueColor),
        }
    }
}

/// Stands in for [`crate::color::ColorTransform`] — see the module docs'
/// "Color transforms" section for how `amount` is interpreted per kind.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTransformKind {
    /// No transform.
    None,
    Grayscale,
    Invert,
    Sepia,
    /// `amount` is a multiplier, clamped to `[0.0, 4.0]` — see
    /// [`ColorTransform::Brightness`].
    Brightness,
    /// `amount` is a multiplier, clamped to `[0.0, 4.0]` — see
    /// [`ColorTransform::Contrast`].
    Contrast,
    /// `amount` is a multiplier, clamped to `[0.0, 4.0]` — see
    /// [`ColorTransform::Saturate`].
    Saturate,
    /// `amount` is degrees of hue rotation — see
    /// [`ColorTransform::HueRotate`].
    HueRotate,
}

/// Builds a [`ColorTransform`] from a [`ColorTransformKind`] and its one
/// `amount` parameter, per the module docs' "Color transforms" section.
fn build_transform(kind: ColorTransformKind, amount: f32) -> ColorTransform {
    match kind {
        ColorTransformKind::None => ColorTransform::None,
        ColorTransformKind::Grayscale => ColorTransform::Grayscale,
        ColorTransformKind::Invert => ColorTransform::Invert,
        ColorTransformKind::Sepia => ColorTransform::Sepia,
        ColorTransformKind::Brightness => ColorTransform::Brightness(amount),
        ColorTransformKind::Contrast => ColorTransform::Contrast(amount),
        ColorTransformKind::Saturate => ColorTransform::Saturate(amount),
        ColorTransformKind::HueRotate => ColorTransform::HueRotate(amount),
    }
}

/// Renders an already-converted [`Grid`] with `depth` and, when `depth`
/// isn't [`ColorDepthKind::Plain`], `dither`/`dither_levels` and an
/// optional [`ColorTransformKind`]/`amount` — the shared depth-dispatch
/// logic behind [`image_bytes_to_string_ex`], [`illusion_to_string`], and
/// [`image_bytes_to_string_with_script`].
fn render_grid_ex(
    grid: &Grid,
    depth: ColorDepthKind,
    dither: DitherMethod,
    dither_levels: u8,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> String {
    let Some(color_depth) = depth.to_iascii() else {
        // Plain output: no color, so dithering/transforms (which only
        // ever affect *color*) have nothing to do.
        return crate::render::grid_to_string(grid, None);
    };
    let options = DitherOptions::new(dither).levels(dither_levels);
    if matches!(transform, ColorTransformKind::None) {
        crate::dither::render_ansi_dithered(grid, color_depth, options)
    } else {
        let built = build_transform(transform, transform_amount);
        crate::dither::render_ansi_dithered_transformed(grid, color_depth, options, &built)
    }
}

/// The full-featured counterpart to [`image_bytes_to_string`]: ANSI-16/256
/// (not just TrueColor-or-plain) via `depth`, optional dithering via
/// `dither`/`dither_levels` (ignored when `depth` is
/// [`ColorDepthKind::Plain`] — there's no color to dither), and an
/// optional single [`ColorTransformKind`]/`transform_amount` applied
/// before quantization (see the module docs). Pass
/// `dither: DitherMethod::None` and `transform: ColorTransformKind::None`
/// for output equivalent to [`image_bytes_to_string`]'s `color: true`.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn image_bytes_to_string_ex(
    bytes: &[u8],
    max_width: usize,
    depth: ColorDepthKind,
    dither: DitherMethod,
    dither_levels: u8,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> Result<String, JsError> {
    let grid = decode_and_convert(bytes, max_width, None)?;
    Ok(render_grid_ex(
        &grid,
        depth,
        dither,
        dither_levels,
        transform,
        transform_amount,
    ))
}

/// Line-per-`JsValue` counterpart to [`image_bytes_to_string_ex`] — see
/// [`image_bytes_to_lines`] for why a caller might prefer this over the
/// single-string form.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn image_bytes_to_lines_ex(
    bytes: &[u8],
    max_width: usize,
    depth: ColorDepthKind,
    dither: DitherMethod,
    dither_levels: u8,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> Result<Vec<JsValue>, JsError> {
    let grid = decode_and_convert(bytes, max_width, None)?;
    let text = render_grid_ex(
        &grid,
        depth,
        dither,
        dither_levels,
        transform,
        transform_amount,
    );
    Ok(strings_to_js(text.lines().map(str::to_string).collect()))
}

// ---------------------------------------------------------------------
// Script / character-ramp bindings.
// ---------------------------------------------------------------------

/// Same as [`image_bytes_to_string_ex`], but converts using `script`'s
/// character ramp (see [`crate::script::Script`]) instead of the default
/// Latin/ASCII one. `dark` selects [`crate::script::Script::ramp_type`]'s
/// `dark`/light terminal-background convention.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn image_bytes_to_string_with_script(
    bytes: &[u8],
    max_width: usize,
    script: crate::script::Script,
    dark: bool,
    depth: ColorDepthKind,
    dither: DitherMethod,
    dither_levels: u8,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> Result<String, JsError> {
    let ramp_type = script
        .ramp_type(dark)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let grid = decode_and_convert(bytes, max_width, Some(ramp_type))?;
    Ok(render_grid_ex(
        &grid,
        depth,
        dither,
        dither_levels,
        transform,
        transform_amount,
    ))
}

// ---------------------------------------------------------------------
// Colorize (PixelAnimation) bindings: recolor already-rendered plain
// text (from any of the Plain-depth functions above, banner_lines_wasm,
// or an illusion rendered with ColorDepthKind::Plain).
// ---------------------------------------------------------------------

/// Stands in for the built-in, closure-free variants of
/// [`crate::colorize::PixelAnimation`] — `Custom` can't cross the
/// `wasm-bindgen` boundary (same reasoning as [`TextAnimationKind`]
/// leaving out `TextAnimation::Custom`), so drive fully custom per-cell
/// math from JS directly against the plain text this module already
/// returns.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationKind {
    /// Uses `speed`.
    Rainbow,
    /// Uses `color1` as `base_color`.
    Wave,
    /// Uses `color1` as `base_color`.
    Pulse,
    /// Uses `color1` as `on_color`, `color2` as `off_color`.
    Blink,
    /// Uses `scale` and `speed`.
    Plasma,
    /// Uses `color1` as `base_color` and `speed`.
    Ripple,
    /// Uses `color1` as `from`, `color2` as `to`, and `horizontal`.
    Gradient,
}

/// Builds a [`crate::colorize::PixelAnimation`] from an [`AnimationKind`]
/// and the shared parameter set every `colorize_*` binding below accepts
/// — parameters not relevant to the chosen `kind` are simply ignored (see
/// each [`AnimationKind`] variant's doc comment for which ones it uses).
#[allow(clippy::too_many_arguments)]
fn build_animation(
    kind: AnimationKind,
    speed: f32,
    scale: f32,
    color1: (u8, u8, u8),
    color2: (u8, u8, u8),
    horizontal: bool,
) -> crate::colorize::PixelAnimation {
    use crate::colorize::PixelAnimation;
    use crate::text::Rgb;
    let c1 = Rgb::new(color1.0, color1.1, color1.2);
    let c2 = Rgb::new(color2.0, color2.1, color2.2);
    match kind {
        AnimationKind::Rainbow => PixelAnimation::Rainbow { speed },
        AnimationKind::Wave => PixelAnimation::Wave { base_color: c1 },
        AnimationKind::Pulse => PixelAnimation::Pulse { base_color: c1 },
        AnimationKind::Blink => PixelAnimation::Blink {
            on_color: c1,
            off_color: c2,
        },
        AnimationKind::Plasma => PixelAnimation::Plasma { scale, speed },
        AnimationKind::Ripple => PixelAnimation::Ripple {
            base_color: c1,
            speed,
        },
        AnimationKind::Gradient => PixelAnimation::Gradient {
            from: c1,
            to: c2,
            horizontal,
        },
    }
}

/// Colors one frame of already-rendered *plain* (uncolored) ASCII text —
/// e.g. from [`image_bytes_to_string_ex`] with
/// `depth: ColorDepthKind::Plain`, [`banner_lines_wasm`] joined with
/// `"\n"`, or [`illusion_to_string`] with `depth: ColorDepthKind::Plain`
/// — under a built-in [`AnimationKind`] at `frame`, optionally layering a
/// [`ColorTransformKind`] on top (via
/// [`crate::colorize::PixelAnimation::Transformed`]; pass
/// `transform: ColorTransformKind::None` to skip it).
///
/// `color1`/`color2` are `0xRRGGBB` packed 24-bit colors — which one(s)
/// (if any) a given `kind` uses, and what `speed`/`scale`/`horizontal`
/// mean for it, are documented on each [`AnimationKind`] variant.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn colorize_plain_text(
    plain_text: &str,
    kind: AnimationKind,
    frame: u64,
    speed: f32,
    scale: f32,
    color1: u32,
    color2: u32,
    horizontal: bool,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> String {
    let animation = build_animation(
        kind,
        speed,
        scale,
        unpack_rgb(color1),
        unpack_rgb(color2),
        horizontal,
    );
    let animation = if matches!(transform, ColorTransformKind::None) {
        animation
    } else {
        crate::colorize::PixelAnimation::Transformed {
            base: Box::new(animation),
            transform: build_transform(transform, transform_amount),
        }
    };
    let lines: Vec<&str> = plain_text.lines().collect();
    crate::colorize::colorize_lines(&lines, &animation, frame)
}

fn unpack_rgb(packed: u32) -> (u8, u8, u8) {
    (
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        (packed & 0xFF) as u8,
    )
}

// ---------------------------------------------------------------------
// Banner bindings.
// ---------------------------------------------------------------------

/// Wraps [`crate::banner::banner_lines`]: renders `text` as plain
/// (uncolored) block-letter banner rows, one `JsValue` string per row.
/// Feed the joined (`"\n"`-separated) result into [`colorize_plain_text`]
/// to animate it, the same way you would a converted image's plain text.
#[wasm_bindgen]
pub fn banner_lines_wasm(text: &str) -> Vec<JsValue> {
    strings_to_js(crate::banner::banner_lines(text))
}

/// Renders a single frame of [`crate::banner::banner_frames`] (built-in
/// [`TextAnimationKind`] styles only, same restriction and reasoning as
/// [`text_frame_at`]) as one `JsValue` string per banner row, for callers
/// driving their own timing loop in JS.
#[wasm_bindgen]
pub fn banner_frame_at(text: &str, animation: TextAnimationKind, frame_index: u32) -> Vec<JsValue> {
    use crate::text::TextAnimOptions;

    let anim = text_animation_kind_to_animation(animation);
    let opts = TextAnimOptions {
        frames: frame_index as u64 + 1,
        ..Default::default()
    };
    let frame = crate::banner::banner_frames(text, &anim, &opts)
        .ok()
        .and_then(|frames| frames.into_iter().last())
        .unwrap_or_default();
    strings_to_js(frame)
}

// ---------------------------------------------------------------------
// Text (single-line) bindings — unchanged from before, moved down here
// only to keep this file's sections grouped by module.
// ---------------------------------------------------------------------

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
    use crate::text::TextAnimOptions;

    let anim = text_animation_kind_to_animation(animation);
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
/// [`text_frame_at`]/[`banner_frame_at`] hardcode — a
/// `wasm-bindgen`-compatible enum, since the real
/// [`crate::text::TextAnimation`] (with its `Blink`/`Marquee` struct
/// variants and `Custom` closures/subprocesses) can't cross the JS
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

fn text_animation_kind_to_animation(kind: TextAnimationKind) -> crate::text::TextAnimation {
    use crate::text::{Rgb, TextAnimation};
    match kind {
        TextAnimationKind::Typewriter => TextAnimation::Typewriter,
        TextAnimationKind::Rainbow => TextAnimation::Rainbow,
        TextAnimationKind::Wave => TextAnimation::Wave,
        TextAnimationKind::Pulse => TextAnimation::Pulse,
        TextAnimationKind::Blink => TextAnimation::Blink {
            on_color: Rgb::WHITE,
            off_color: Rgb::new(0, 0, 0),
        },
    }
}

// ---------------------------------------------------------------------
// Illusion bindings.
// ---------------------------------------------------------------------

/// Stands in for [`crate::illusions::Illusion`] — a `wasm-bindgen`
/// fieldless enum plus the shared `param_a`/`param_b` this module's
/// illusion functions take, since `Illusion`'s own variants carry named
/// fields (`tile`/`offset`, `cell`/`line_width`, `lines`) that can't cross
/// the JS boundary directly the same way a fieldless enum can.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IllusionKind {
    /// Uses `param_a` as `tile`, `param_b` as `offset`.
    CafeWall,
    /// Uses `param_a` as `cell`, `param_b` as `line_width`.
    HermannGrid,
    /// Uses `param_a` as `lines` (`param_b` is ignored).
    TwistedCord,
}

fn build_illusion(kind: IllusionKind, param_a: u32, param_b: u32) -> crate::illusions::Illusion {
    use crate::illusions::Illusion;
    match kind {
        IllusionKind::CafeWall => Illusion::CafeWall {
            tile: param_a,
            offset: param_b,
        },
        IllusionKind::HermannGrid => Illusion::HermannGrid {
            cell: param_a,
            line_width: param_b,
        },
        IllusionKind::TwistedCord => Illusion::TwistedCord { lines: param_a },
    }
}

/// Generates `kind` (see [`IllusionKind`] for what `param_a`/`param_b`
/// mean for each one) at `src_width`x`src_height` source pixels, converts
/// it to ASCII capped at `max_width` output columns, and renders it with
/// the same `depth`/`dither`/`transform` options as
/// [`image_bytes_to_string_ex`] — no image bytes needed, since illusions
/// are generated procedurally (see [`crate::illusions::generate`]).
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn illusion_to_string(
    kind: IllusionKind,
    param_a: u32,
    param_b: u32,
    src_width: u32,
    src_height: u32,
    max_width: usize,
    depth: ColorDepthKind,
    dither: DitherMethod,
    dither_levels: u8,
    transform: ColorTransformKind,
    transform_amount: f32,
) -> Result<String, JsError> {
    let illusion = build_illusion(kind, param_a, param_b);
    let img = crate::illusions::generate(illusion, src_width, src_height);
    let ascii_config = sized_ascii_config(max_width, None)?;
    let grid =
        iascii::convert::convert_image(img.width(), img.height(), img.as_raw(), &ascii_config)
            .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(render_grid_ex(
        &grid,
        depth,
        dither,
        dither_levels,
        transform,
        transform_amount,
    ))
}

/// Wraps [`crate::illusions::rotating_rings_frames`] directly — it's
/// already pure character-grid animation with no `Config`/image pipeline
/// involved, so there's nothing else for this binding to do beyond
/// marshaling the frame strings to JS. See that function's docs for what
/// `rings`/`speed`/`fill_char` control.
#[wasm_bindgen]
pub fn rotating_rings_to_lines(
    width: usize,
    height: usize,
    rings: u32,
    speed: f32,
    frame_count: u64,
    fill_char: char,
) -> Vec<JsValue> {
    strings_to_js(crate::illusions::rotating_rings_frames(
        width,
        height,
        rings,
        speed,
        frame_count,
        fill_char,
    ))
}

// ---------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------

fn strings_to_js(lines: Vec<String>) -> Vec<JsValue> {
    lines
        .into_iter()
        .map(|line| JsValue::from_str(&line))
        .collect()
}

fn sized_ascii_config(
    max_width: usize,
    ramp: Option<iascii::ramp::RampType>,
) -> Result<iascii::config::Config, JsError> {
    let mut builder = AsciiConfigBuilder::new();
    if max_width > 0 {
        builder = builder.output_sizing(OutputSizing::MaxWidth(max_width));
    }
    if let Some(ramp) = ramp {
        builder = builder.ramp(ramp);
    }
    builder.build().map_err(|e| JsError::new(&e.to_string()))
}

fn decode_and_convert(
    bytes: &[u8],
    max_width: usize,
    ramp: Option<iascii::ramp::RampType>,
) -> Result<Grid, JsError> {
    let img = ::image::load_from_memory(bytes).map_err(|e| JsError::new(&e.to_string()))?;
    let rgb = img.to_rgb8();
    let ascii_config = sized_ascii_config(max_width, ramp)?;
    iascii::convert::convert_image(rgb.width(), rgb.height(), rgb.as_raw(), &ascii_config)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wasm_bindgen::JsError` doesn't implement `Debug` on every target
    /// (notably: not on the native target this test suite actually runs
    /// under, with the older `wasm-bindgen` this crate is pinned to for
    /// this sandbox's rustc — see `TESTS_AND_BENCHMARKS.md`), so a plain
    /// `.unwrap()` on a `Result<_, JsError>` won't compile here. This is a
    /// `Debug`-free stand-in that still panics with a useful message.
    trait UnwrapJs<T> {
        fn unwrap_js(self) -> T;
    }
    impl<T> UnwrapJs<T> for Result<T, JsError> {
        fn unwrap_js(self) -> T {
            match self {
                Ok(v) => v,
                Err(_) => panic!("expected Ok, got a JsError"),
            }
        }
    }

    fn tiny_png_bytes() -> Vec<u8> {
        let img = ::image::RgbImage::from_fn(4, 4, |x, y| {
            if (x + y) % 2 == 0 {
                ::image::Rgb([200u8, 40, 40])
            } else {
                ::image::Rgb([40u8, 40, 200])
            }
        });
        let mut buf = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut buf),
            ::image::ImageFormat::Png,
        )
        .unwrap();
        buf
    }

    #[test]
    fn image_bytes_to_string_plain_has_no_escapes() {
        let bytes = tiny_png_bytes();
        let out = image_bytes_to_string(&bytes, 4, false).unwrap_js();
        assert!(!out.contains('\x1b'));
        assert!(!out.is_empty());
    }

    #[test]
    fn image_bytes_to_string_color_has_truecolor_escapes() {
        let bytes = tiny_png_bytes();
        let out = image_bytes_to_string(&bytes, 4, true).unwrap_js();
        assert!(out.contains("\x1b[38;2;"));
    }

    #[test]
    fn image_bytes_to_string_ex_plain_ignores_dither_and_transform() {
        let bytes = tiny_png_bytes();
        let plain = image_bytes_to_string_ex(
            &bytes,
            4,
            ColorDepthKind::Plain,
            DitherMethod::FloydSteinberg,
            6,
            ColorTransformKind::Invert,
            0.0,
        )
        .unwrap_js();
        assert!(!plain.contains('\x1b'));
    }

    #[test]
    fn image_bytes_to_string_ex_covers_every_depth() {
        let bytes = tiny_png_bytes();
        for depth in [
            ColorDepthKind::Ansi16,
            ColorDepthKind::Ansi256,
            ColorDepthKind::TrueColor,
        ] {
            let out = image_bytes_to_string_ex(
                &bytes,
                4,
                depth,
                DitherMethod::None,
                6,
                ColorTransformKind::None,
                0.0,
            )
            .unwrap_js();
            assert!(
                out.contains('\x1b'),
                "{depth:?} should produce ANSI escapes"
            );
        }
    }

    #[test]
    fn image_bytes_to_string_ex_with_grayscale_transform_has_no_pure_source_red() {
        let bytes = tiny_png_bytes();
        let out = image_bytes_to_string_ex(
            &bytes,
            4,
            ColorDepthKind::TrueColor,
            DitherMethod::None,
            6,
            ColorTransformKind::Grayscale,
            0.0,
        )
        .unwrap_js();
        assert!(!out.contains("38;2;200;40;40"));
    }

    // JsValue::from_str genuinely only works on target_arch = "wasm32"
    // (confirmed: it compiles natively but panics at runtime off wasm32 --
    // see TESTS_AND_BENCHMARKS.md), so this is ignored on the native target
    // this workspace tests under and only meaningful under wasm-bindgen-test.
    #[cfg_attr(not(target_arch = "wasm32"), ignore = "JsValue only works on wasm32")]
    #[test]
    fn image_bytes_to_lines_ex_matches_line_count_of_the_string_form() {
        let bytes = tiny_png_bytes();
        let text = image_bytes_to_string_ex(
            &bytes,
            4,
            ColorDepthKind::TrueColor,
            DitherMethod::FloydSteinberg,
            6,
            ColorTransformKind::None,
            0.0,
        )
        .unwrap_js();
        let lines = image_bytes_to_lines_ex(
            &bytes,
            4,
            ColorDepthKind::TrueColor,
            DitherMethod::FloydSteinberg,
            6,
            ColorTransformKind::None,
            0.0,
        )
        .unwrap_js();
        assert_eq!(lines.len(), text.lines().count());
    }

    #[test]
    fn image_bytes_to_string_with_script_covers_every_script() {
        let bytes = tiny_png_bytes();
        for script in [
            crate::script::Script::Latin,
            crate::script::Script::Block,
            crate::script::Script::Braille,
            crate::script::Script::Cyrillic,
            crate::script::Script::Cjk,
        ] {
            let out = image_bytes_to_string_with_script(
                &bytes,
                4,
                script,
                true,
                ColorDepthKind::Plain,
                DitherMethod::None,
                6,
                ColorTransformKind::None,
                0.0,
            )
            .unwrap_js();
            assert!(!out.is_empty(), "{script:?}");
        }
    }

    // JsValue::from_str genuinely only works on target_arch = "wasm32"
    // (confirmed: it compiles natively but panics at runtime off wasm32 --
    // see TESTS_AND_BENCHMARKS.md), so this is ignored on the native target
    // this workspace tests under and only meaningful under wasm-bindgen-test.
    #[cfg_attr(not(target_arch = "wasm32"), ignore = "JsValue only works on wasm32")]
    #[test]
    fn gif_bytes_to_frame_strings_returns_one_entry_per_frame() {
        use ::image::codecs::gif::{GifEncoder, Repeat};
        use ::image::{Delay, Frame, RgbaImage};

        let mut buf = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut buf);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..3u8 {
                let level = i * 60;
                let frame_buf =
                    RgbaImage::from_pixel(4, 4, ::image::Rgba([level, level, level, 255]));
                encoder
                    .encode_frame(Frame::from_parts(
                        frame_buf,
                        0,
                        0,
                        Delay::from_numer_denom_ms(30, 1),
                    ))
                    .unwrap();
            }
        }

        let frames = gif_bytes_to_frame_strings(&buf, 4, true).unwrap_js();
        assert_eq!(frames.len(), 3);
    }

    #[test]
    fn colorize_plain_text_produces_ansi_output_for_every_animation_kind() {
        let plain = "####\n####\n####\n####";
        for kind in [
            AnimationKind::Rainbow,
            AnimationKind::Wave,
            AnimationKind::Pulse,
            AnimationKind::Blink,
            AnimationKind::Plasma,
            AnimationKind::Ripple,
            AnimationKind::Gradient,
        ] {
            let out = colorize_plain_text(
                plain,
                kind,
                2,
                5.0,
                0.3,
                0xFF0000,
                0x0000FF,
                true,
                ColorTransformKind::None,
                0.0,
            );
            assert!(out.contains("\x1b[38;2;"), "{kind:?}");
            assert_eq!(out.lines().count(), 4);
        }
    }

    #[test]
    fn colorize_plain_text_with_transform_still_colors_every_line() {
        let plain = "##\n##";
        let out = colorize_plain_text(
            plain,
            AnimationKind::Rainbow,
            0,
            5.0,
            0.0,
            0xFFFFFF,
            0x000000,
            false,
            ColorTransformKind::Sepia,
            0.0,
        );
        assert_eq!(out.lines().count(), 2);
        assert!(out.contains("\x1b[38;2;"));
    }

    #[test]
    fn unpack_rgb_splits_a_packed_color_correctly() {
        assert_eq!(unpack_rgb(0xFF8000), (255, 128, 0));
        assert_eq!(unpack_rgb(0x000000), (0, 0, 0));
        assert_eq!(unpack_rgb(0xFFFFFF), (255, 255, 255));
    }

    // JsValue::from_str genuinely only works on target_arch = "wasm32"
    // (confirmed: it compiles natively but panics at runtime off wasm32 --
    // see TESTS_AND_BENCHMARKS.md), so this is ignored on the native target
    // this workspace tests under and only meaningful under wasm-bindgen-test.
    #[cfg_attr(not(target_arch = "wasm32"), ignore = "JsValue only works on wasm32")]
    #[test]
    fn banner_lines_wasm_returns_the_expected_row_count() {
        let lines = banner_lines_wasm("HI");
        assert_eq!(lines.len(), crate::banner::banner_lines("HI").len());
    }

    // JsValue::from_str genuinely only works on target_arch = "wasm32"
    // (confirmed: it compiles natively but panics at runtime off wasm32 --
    // see TESTS_AND_BENCHMARKS.md), so this is ignored on the native target
    // this workspace tests under and only meaningful under wasm-bindgen-test.
    #[cfg_attr(not(target_arch = "wasm32"), ignore = "JsValue only works on wasm32")]
    #[test]
    fn banner_frame_at_returns_one_row_per_glyph_row() {
        let frame = banner_frame_at("HI", TextAnimationKind::Rainbow, 0);
        assert_eq!(frame.len(), crate::banner::banner_lines("HI").len());
    }

    #[test]
    fn text_frame_at_covers_every_animation_kind_without_panicking() {
        for kind in [
            TextAnimationKind::Typewriter,
            TextAnimationKind::Rainbow,
            TextAnimationKind::Wave,
            TextAnimationKind::Blink,
            TextAnimationKind::Pulse,
        ] {
            let frame = text_frame_at("hi", kind, 1);
            assert!(!frame.is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn illusion_to_string_covers_every_illusion_kind() {
        for kind in [
            IllusionKind::CafeWall,
            IllusionKind::HermannGrid,
            IllusionKind::TwistedCord,
        ] {
            let out = illusion_to_string(
                kind,
                8,
                4,
                32,
                16,
                20,
                ColorDepthKind::Ansi256,
                DitherMethod::Bayer4,
                6,
                ColorTransformKind::None,
                0.0,
            )
            .unwrap_js();
            assert!(!out.is_empty(), "{kind:?}");
            assert!(out.contains('\x1b'));
        }
    }

    #[test]
    fn illusion_to_string_plain_depth_has_no_escapes() {
        let out = illusion_to_string(
            IllusionKind::HermannGrid,
            10,
            2,
            20,
            20,
            15,
            ColorDepthKind::Plain,
            DitherMethod::None,
            6,
            ColorTransformKind::None,
            0.0,
        )
        .unwrap_js();
        assert!(!out.contains('\x1b'));
    }

    // JsValue::from_str genuinely only works on target_arch = "wasm32"
    // (confirmed: it compiles natively but panics at runtime off wasm32 --
    // see TESTS_AND_BENCHMARKS.md), so this is ignored on the native target
    // this workspace tests under and only meaningful under wasm-bindgen-test.
    #[cfg_attr(not(target_arch = "wasm32"), ignore = "JsValue only works on wasm32")]
    #[test]
    fn rotating_rings_to_lines_returns_the_requested_frame_count() {
        let frames = rotating_rings_to_lines(10, 6, 3, 0.5, 4, '#');
        assert_eq!(frames.len(), 4);
    }

    #[test]
    fn color_depth_kind_plain_maps_to_no_color_depth() {
        assert_eq!(ColorDepthKind::Plain.to_iascii(), None);
        assert_eq!(
            ColorDepthKind::TrueColor.to_iascii(),
            Some(ColorDepth::TrueColor)
        );
    }

    #[test]
    fn build_transform_maps_every_kind() {
        assert_eq!(
            build_transform(ColorTransformKind::None, 0.0),
            ColorTransform::None
        );
        assert_eq!(
            build_transform(ColorTransformKind::Grayscale, 0.0),
            ColorTransform::Grayscale
        );
        assert_eq!(
            build_transform(ColorTransformKind::Brightness, 2.0),
            ColorTransform::Brightness(2.0)
        );
        assert_eq!(
            build_transform(ColorTransformKind::HueRotate, 90.0),
            ColorTransform::HueRotate(90.0)
        );
    }
}
