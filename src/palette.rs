//! Shared ANSI color-palette quantization helpers.
//!
//! Both [`crate::dither`] (error-diffusion / ordered dithering down to a
//! target [`ColorDepth`]) and [`crate::segment`] (matching a styled
//! segment's *unstyled* neighbor cells to the same palette
//! `iascii::render::render_ansi` would have picked, so a partially-styled
//! frame looks visually consistent) need to answer the same question:
//! "given a 24-bit color and a [`ColorDepth`], what's the closest
//! displayable color, and what escape sequence draws it?".
//!
//! `iascii::render::ansi` answers that question too, but its palette
//! tables and nearest-color search are private to that crate. This module
//! reimplements the same standard formulas (the fixed 16-color ANSI
//! palette; the xterm 6x6x6 color cube plus 24-step grayscale ramp for
//! 256-color) independently, so results match what plain (unstyled)
//! `iascii` output would show at the same depth.

use iascii::render::ColorDepth;

pub(crate) type Rgb8 = (u8, u8, u8);

/// Standard 16-color ANSI palette, in the same order as the SGR 30-37 /
/// 90-97 foreground codes.
pub(crate) static ANSI16_PALETTE: [Rgb8; 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn dist2(a: Rgb8, b: Rgb8) -> u32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Nearest of the 16 standard ANSI colors to `c`.
pub(crate) fn nearest_ansi16(c: Rgb8) -> u8 {
    ANSI16_PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, &p)| dist2(c, p))
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}

/// The actual displayed color for ANSI-16 index `idx` (0..=15).
pub(crate) fn ansi16_rgb(idx: u8) -> Rgb8 {
    ANSI16_PALETTE[(idx as usize).min(15)]
}

/// Nearest xterm 256-color index (the 16 base colors are left out of the
/// search, matching `iascii`'s own approach of only choosing between the
/// 6x6x6 cube (16..=231) and the grayscale ramp (232..=255)).
pub(crate) fn nearest_ansi256(c: Rgb8) -> u8 {
    let closest_level = |v: u8| -> (usize, u8) {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, &l)| (v as i32 - l as i32).unsigned_abs())
            .map(|(i, &l)| (i, l))
            .unwrap_or((0, 0))
    };
    let (ri, rv) = closest_level(c.0);
    let (gi, gv) = closest_level(c.1);
    let (bi, bv) = closest_level(c.2);
    let cube_dist = dist2(c, (rv, gv, bv));

    let gray_avg = ((c.0 as u32 + c.1 as u32 + c.2 as u32) / 3) as u8;
    let gray_idx: u8 = if gray_avg < 8 {
        0
    } else if gray_avg > 238 {
        23
    } else {
        ((gray_avg - 8) as f32 / 10.0).round() as u8
    };
    let gray_val = 8 + gray_idx * 10;
    let gray_dist = dist2(c, (gray_val, gray_val, gray_val));

    if gray_dist < cube_dist {
        232 + gray_idx
    } else {
        16 + (36 * ri + 6 * gi + bi) as u8
    }
}

/// The actual displayed color for xterm 256-color index `idx`.
pub(crate) fn ansi256_rgb(idx: u8) -> Rgb8 {
    if idx >= 232 {
        let v = 8 + (idx - 232) * 10;
        (v, v, v)
    } else if idx >= 16 {
        let i = idx - 16;
        let r = CUBE_LEVELS[(i / 36) as usize];
        let g = CUBE_LEVELS[((i / 6) % 6) as usize];
        let b = CUBE_LEVELS[(i % 6) as usize];
        (r, g, b)
    } else {
        ansi16_rgb(idx)
    }
}

/// Posterizes one 8-bit channel to `levels` evenly spaced steps
/// (`levels >= 2`).
pub(crate) fn posterize_channel(v: u8, levels: u8) -> u8 {
    let levels = levels.max(2) as f32;
    let step = 255.0 / (levels - 1.0);
    ((v as f32 / step).round() * step).clamp(0.0, 255.0) as u8
}

/// A color resolved against a specific [`ColorDepth`]'s displayable
/// palette, alongside enough information to both emit its ANSI escape and
/// (for error-diffusion dithering) know exactly what color will actually
/// appear on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quantized {
    TrueColor(Rgb8),
    Ansi256(u8),
    Ansi16(u8),
}

impl Quantized {
    pub(crate) fn rgb(&self) -> Rgb8 {
        match *self {
            Quantized::TrueColor(c) => c,
            Quantized::Ansi256(i) => ansi256_rgb(i),
            Quantized::Ansi16(i) => ansi16_rgb(i),
        }
    }

    pub(crate) fn escape(&self) -> String {
        match *self {
            Quantized::TrueColor((r, g, b)) => format!("\x1b[38;2;{r};{g};{b}m"),
            Quantized::Ansi256(i) => format!("\x1b[38;5;{i}m"),
            Quantized::Ansi16(i) => {
                let code = if i < 8 { 30 + i } else { 90 + (i - 8) };
                format!("\x1b[{code}m")
            }
        }
    }
}

/// Resolves `c` against `depth`'s palette. `levels`, when `Some`, additionally
/// posterizes each channel before the palette lookup — meaningful only for
/// [`ColorDepth::TrueColor`] (used by [`crate::dither`] to posterize before
/// diffusing error; `Ansi256`/`Ansi16` are already coarse discrete palettes,
/// so `levels` is ignored for them).
pub(crate) fn quantize_for_depth(c: Rgb8, depth: ColorDepth, levels: Option<u8>) -> Quantized {
    match depth {
        ColorDepth::TrueColor => match levels {
            Some(l) => Quantized::TrueColor((
                posterize_channel(c.0, l),
                posterize_channel(c.1, l),
                posterize_channel(c.2, l),
            )),
            None => Quantized::TrueColor(c),
        },
        ColorDepth::Ansi256 => Quantized::Ansi256(nearest_ansi256(c)),
        ColorDepth::Ansi16 => Quantized::Ansi16(nearest_ansi16(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_ansi16_matches_exact_palette_entries() {
        for (i, &c) in ANSI16_PALETTE.iter().enumerate() {
            assert_eq!(nearest_ansi16(c), i as u8);
        }
    }

    #[test]
    fn ansi256_roundtrips_cube_corners() {
        let idx = nearest_ansi256((255, 0, 0));
        let rgb = ansi256_rgb(idx);
        assert!(rgb.0 > 200 && rgb.1 < 50 && rgb.2 < 50);
    }

    #[test]
    fn posterize_channel_clamps_to_levels() {
        assert_eq!(posterize_channel(0, 2), 0);
        assert_eq!(posterize_channel(255, 2), 255);
        assert_eq!(posterize_channel(130, 2), 255);
        assert_eq!(posterize_channel(120, 2), 0);
    }

    #[test]
    fn quantize_truecolor_without_levels_is_identity() {
        let q = quantize_for_depth((10, 20, 30), ColorDepth::TrueColor, None);
        assert_eq!(q.rgb(), (10, 20, 30));
    }
}
