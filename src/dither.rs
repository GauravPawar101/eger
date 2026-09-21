//! Dithering for `eger`'s color output.
//!
//! `iascii::render::render_ansi` picks one exact color per cell — nearest
//! ANSI-256/16 palette entry, or a straight 24-bit passthrough for
//! `TrueColor`. For a limited palette (`Ansi16` especially, `Ansi256` to a
//! lesser extent) that means neighboring cells with similar-but-not-equal
//! source colors often collapse onto the *same* palette entry, producing
//! flat, banded color regions where the source had a smooth gradient.
//!
//! Dithering fixes that the classic way: instead of independently rounding
//! each cell to its nearest palette color, the rounding *error* introduced
//! by one cell is carried forward and folded into its neighbors before
//! they're rounded, so the palette's limited colors average out — over a
//! few cells — to something closer to the true source gradient. This
//! module works entirely in *character-grid* space (one diffusion step per
//! `iascii::grid::Grid` cell, not per source pixel), which is the
//! resolution that actually matters for how the terminal output looks.
//!
//! Two families are provided:
//! - **Error diffusion** ([`DitherMethod::FloydSteinberg`],
//!   [`DitherMethod::Atkinson`]): each cell's quantization error is spread
//!   into specific not-yet-visited neighbors with fixed weights.
//!   Floyd–Steinberg spreads all of it (higher fidelity, more visible
//!   diagonal "worm" artifacts); Atkinson spreads only 3/4 of it (the
//!   classic original Macintosh look — lower contrast, cleaner).
//! - **Ordered dithering** ([`DitherMethod::Bayer2`]/[`DitherMethod::Bayer4`]/[`DitherMethod::Bayer8`]):
//!   a fixed threshold matrix biases each cell's color by a
//!   position-dependent (not error-dependent) amount before quantizing.
//!   No error carries between cells, so it parallelizes/tiles trivially
//!   and produces the recognizable regular crosshatch pattern instead of
//!   error diffusion's organic noise — useful for a stable look across an
//!   animated sequence, where error diffusion's pattern would otherwise
//!   shift unpredictably frame to frame.
//!
//! For [`ColorDepth::Ansi16`]/[`ColorDepth::Ansi256`], dithering is almost
//! always worth turning on — those palettes are coarse enough that banding
//! is usually visible. For [`ColorDepth::TrueColor`], there's no palette
//! limit to smooth over by default, so dithering instead *posterizes* each
//! channel down to [`DitherOptions::levels`] steps first and dithers
//! against that — a deliberate stylistic reduction (think retro/limited-
//! palette look) rather than a fix for banding.

use crate::palette::{self, Quantized};
use iascii::grid::Grid;
use iascii::render::ColorDepth;

/// Which dithering algorithm (if any) to apply. See the module docs for
/// the tradeoffs between the error-diffusion and ordered families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DitherMethod {
    /// No dithering: plain nearest-palette rounding per cell, same as
    /// `iascii::render::render_ansi`.
    #[default]
    None,
    /// Classic Floyd–Steinberg error diffusion.
    FloydSteinberg,
    /// Atkinson error diffusion (only 3/4 of the error is carried
    /// forward, giving a lower-contrast, less "noisy" result).
    Atkinson,
    /// 2x2 Bayer ordered dithering.
    Bayer2,
    /// 4x4 Bayer ordered dithering.
    Bayer4,
    /// 8x8 Bayer ordered dithering — the finest, least visually obtrusive
    /// ordered pattern of the three.
    Bayer8,
}

/// Settings for a dithered render. `#[must_use]` since, like
/// [`crate::config::ConfigBuilder`], every setter consumes and returns
/// `Self`.
#[must_use]
#[derive(Debug, Clone, Copy)]
pub struct DitherOptions {
    pub method: DitherMethod,
    /// Per-channel posterization step count, used only when rendering at
    /// [`ColorDepth::TrueColor`] (see the module docs). Clamped to at
    /// least 2. Defaults to 6.
    pub levels: u8,
}

impl Default for DitherOptions {
    fn default() -> Self {
        Self {
            method: DitherMethod::default(),
            levels: 6,
        }
    }
}

impl DitherOptions {
    pub fn new(method: DitherMethod) -> Self {
        Self {
            method,
            ..Self::default()
        }
    }

    pub fn levels(mut self, levels: u8) -> Self {
        self.levels = levels.max(2);
        self
    }
}

// Values scaled 0..(n*n - 1); normalized to a [-0.5, 0.5) bias at use time.
const BAYER2: [[u8; 2]; 2] = [[0, 2], [3, 1]];
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

fn bayer_bias(method: DitherMethod, x: u32, y: u32) -> f32 {
    match method {
        DitherMethod::Bayer2 => BAYER2[(y % 2) as usize][(x % 2) as usize] as f32 / 4.0 - 0.5,
        DitherMethod::Bayer4 => BAYER4[(y % 4) as usize][(x % 4) as usize] as f32 / 16.0 - 0.5,
        DitherMethod::Bayer8 => BAYER8[(y % 8) as usize][(x % 8) as usize] as f32 / 64.0 - 0.5,
        _ => 0.0,
    }
}

/// The bias amplitude ordered dithering nudges a channel by, scaled to
/// roughly one target quantization step so the pattern actually pushes
/// borderline colors across a palette boundary instead of being lost to
/// rounding.
fn bias_amplitude(depth: ColorDepth, levels: u8) -> f32 {
    match depth {
        ColorDepth::TrueColor => 255.0 / (levels.max(2) as f32 - 1.0).max(1.0),
        ColorDepth::Ansi256 => 40.0,
        ColorDepth::Ansi16 => 96.0,
    }
}

fn diffuse(
    err: &mut [[f32; 3]],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    e: [f32; 3],
    method: DitherMethod,
) {
    let mut add = |dx: isize, dy: isize, factor: f32| {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
            let idx = ny as usize * w + nx as usize;
            for c in 0..3 {
                err[idx][c] += e[c] * factor;
            }
        }
    };
    match method {
        DitherMethod::FloydSteinberg => {
            add(1, 0, 7.0 / 16.0);
            add(-1, 1, 3.0 / 16.0);
            add(0, 1, 5.0 / 16.0);
            add(1, 1, 1.0 / 16.0);
        }
        DitherMethod::Atkinson => {
            for (dx, dy) in [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)] {
                add(dx, dy, 1.0 / 8.0);
            }
        }
        _ => {}
    }
}

/// Renders `grid` to an ANSI/plain string like
/// [`crate::render::grid_to_string`], but with `options` applied — the
/// dithered counterpart of [`crate::render::grid_to_string`] /
/// `iascii::render::render_ansi`. Use this in place of
/// [`crate::render::dispatch`]'s plain `Grid`-based rendering whenever
/// [`crate::config::Config::dither`] is set; see [`crate::image::render_dynamic_image`]'s
/// callers for how the two compose. Works one frame/image at a time, so
/// it's equally applicable to a single still image or one frame of a
/// video/GIF sequence — see the module docs' note on [`DitherMethod::Bayer2`]
/// / [`DitherMethod::Bayer4`] / [`DitherMethod::Bayer8`] being the steadier choice across an animated
/// sequence.
pub fn render_ansi_dithered(grid: &Grid, depth: ColorDepth, options: DitherOptions) -> String {
    let w = grid.width() as usize;
    let h = grid.height() as usize;
    if w == 0 || h == 0 {
        return String::new();
    }

    let error_diffusing = matches!(
        options.method,
        DitherMethod::FloydSteinberg | DitherMethod::Atkinson
    );
    let mut err = if error_diffusing {
        vec![[0f32; 3]; w * h]
    } else {
        Vec::new()
    };

    let mut out = String::with_capacity(h * (w * 12 + 8));
    let mut current: Option<Quantized> = None;

    for y in 0..h {
        for x in 0..w {
            let Some(cell) = grid.get(x as u32, y as u32) else {
                continue;
            };

            let base = (
                cell.color.r as f32,
                cell.color.g as f32,
                cell.color.b as f32,
            );
            let biased = match options.method {
                DitherMethod::None => base,
                DitherMethod::FloydSteinberg | DitherMethod::Atkinson => {
                    let e = err[y * w + x];
                    (base.0 + e[0], base.1 + e[1], base.2 + e[2])
                }
                DitherMethod::Bayer2 | DitherMethod::Bayer4 | DitherMethod::Bayer8 => {
                    let bias = bayer_bias(options.method, x as u32, y as u32)
                        * bias_amplitude(depth, options.levels);
                    (base.0 + bias, base.1 + bias, base.2 + bias)
                }
            };
            let biased_u8 = (
                biased.0.clamp(0.0, 255.0).round() as u8,
                biased.1.clamp(0.0, 255.0).round() as u8,
                biased.2.clamp(0.0, 255.0).round() as u8,
            );

            let q = palette::quantize_for_depth(biased_u8, depth, Some(options.levels));

            if error_diffusing {
                let actual = q.rgb();
                let e = [
                    biased.0 - actual.0 as f32,
                    biased.1 - actual.1 as f32,
                    biased.2 - actual.2 as f32,
                ];
                diffuse(&mut err, w, h, x, y, e, options.method);
            }

            // Mirrors iascii::render::ansi::render_ansi's own state-diff
            // loop: setting a new SGR foreground color overwrites the
            // previous one on any real terminal, so no `\x1b[0m` reset is
            // needed between two *different* colors — only at the very
            // end of a row (below), same as iascii. This (plus not
            // special-casing space cells, also matching iascii, which
            // colors every cell including blanks) is what makes
            // `DitherMethod::None` byte-for-byte identical to
            // `iascii::render::render_ansi`'s own output — see this
            // module's tests.
            if current != Some(q) {
                out.push_str(&q.escape());
                current = Some(q);
            }
            out.push(cell.ch);
        }
        out.push_str("\x1b[0m\n");
        current = None;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
    use iascii::convert::convert_image;

    fn gradient_grid(w: u32, h: u32) -> Grid {
        let cfg = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::Explicit {
                width: w as usize,
                height: h as usize,
            })
            .build()
            .unwrap();
        let mut buf = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for _ in 0..w {
                let v = ((y as f32 / h.max(1) as f32) * 255.0) as u8;
                buf.extend_from_slice(&[v, v, v]);
            }
        }
        convert_image(w, h, &buf, &cfg).unwrap()
    }

    #[test]
    fn none_method_matches_plain_nearest_palette_rendering() {
        let grid = gradient_grid(6, 6);
        let dithered = render_ansi_dithered(&grid, ColorDepth::Ansi16, DitherOptions::default());
        let plain = iascii::render::render_ansi(&grid, ColorDepth::Ansi16);
        assert_eq!(dithered, plain);
    }

    #[test]
    fn floyd_steinberg_produces_output_without_panicking_and_keeps_shape() {
        let grid = gradient_grid(10, 8);
        let out = render_ansi_dithered(
            &grid,
            ColorDepth::Ansi16,
            DitherOptions::new(DitherMethod::FloydSteinberg),
        );
        assert_eq!(out.lines().count(), 8);
    }

    #[test]
    fn bayer_and_atkinson_do_not_panic_on_small_grids() {
        for method in [
            DitherMethod::Atkinson,
            DitherMethod::Bayer2,
            DitherMethod::Bayer4,
            DitherMethod::Bayer8,
        ] {
            let grid = gradient_grid(3, 3);
            let _ = render_ansi_dithered(&grid, ColorDepth::Ansi256, DitherOptions::new(method));
        }
    }

    #[test]
    fn truecolor_posterizes_to_the_requested_level_count() {
        let grid = gradient_grid(1, 50);
        let out = render_ansi_dithered(
            &grid,
            ColorDepth::TrueColor,
            DitherOptions::new(DitherMethod::None).levels(3),
        );
        let mut seen = std::collections::HashSet::new();
        for line in out.lines() {
            if let Some(start) = line.find("38;2;") {
                if let Some(end) = line[start..].find('m') {
                    seen.insert(line[start..start + end].to_string());
                }
            }
        }
        // 3 levels per channel on a grayscale ramp means at most 3 distinct
        // colors should appear across the whole gradient.
        assert!(
            seen.len() <= 3,
            "expected <=3 distinct colors, got {}",
            seen.len()
        );
    }

    #[test]
    fn dithering_reduces_banding_relative_to_plain_rounding() {
        // On a smooth vertical grayscale gradient rendered at Ansi16 (a
        // very coarse palette), plain per-cell rounding should collapse
        // long runs of identical color; Floyd-Steinberg should break those
        // runs up more (closer to the true gradient) by spreading error.
        let grid = gradient_grid(1, 40);
        let plain = render_ansi_dithered(&grid, ColorDepth::Ansi16, DitherOptions::default());
        let dithered = render_ansi_dithered(
            &grid,
            ColorDepth::Ansi16,
            DitherOptions::new(DitherMethod::FloydSteinberg),
        );
        assert_ne!(plain, dithered);
    }
}
