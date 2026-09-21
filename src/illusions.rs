//! Procedurally generated optical-illusion source art.
//!
//! Unlike the rest of this crate, [`Illusion`] doesn't convert an existing
//! image/video — it *generates* one, from plain pixel math, and then feeds
//! it through the same [`crate::image::render_dynamic_image`] pipeline
//! every photo goes through. That means an illusion generated here picks
//! up whatever [`crate::config::Config`] the caller already has: its ramp
//! (including a [`crate::script::Script`]), its
//! [`crate::config::Config::color_depth`], and — a particularly good
//! pairing, since these illusions are built from flat black/white/gray
//! regions where banding would otherwise be very visible — its
//! [`crate::config::Config::dither`] setting.
//!
//! Four illusions are provided:
//! - [`Illusion::CafeWall`] — offset rows of black/white tiles separated
//!   by thin gray "mortar" lines, whose straight rows read as slanted.
//! - [`Illusion::HermannGrid`] — a white grid on black, whose
//!   intersections show phantom gray dots in peripheral vision.
//! - [`Illusion::TwistedCord`] — a Zöllner-style set of parallel lines
//!   crossed by alternating-angle hatching, which reads as non-parallel.
//! - **Rotating rings** ([`rotating_rings_frames`]) — concentric rings built from a repeating
//!   asymmetric brightness cycle (the same building block behind Kitaoka's
//!   "rotating snakes"). This one is inherently about *motion*, so it's
//!   generated directly as an animated [`crate::colorize::PixelAnimation`]
//!   sequence via [`rotating_rings_frames`] instead of a single static
//!   image — see that function instead of [`generate`]/[`render_illusion`]
//!   for it.
//!
//! These are stylized approximations built for how they read as ASCII/ANSI
//! terminal output, not a vision-science-grade reproduction of the
//! original demonstrations.

use crate::colorize::{colorize_frames, PixelAnimation};
use crate::config::Config;
use crate::error::Result;
use crate::render::{RenderOutput, RenderTarget};
use crate::text::Rgb;
use ::image::{DynamicImage, Rgb as ImgRgb, RgbImage};

/// A static, generatable optical illusion. See the module docs for what
/// each one looks like; the rotating-rings illusion is the exception —
/// it's inherently animated, so it isn't part of this enum (use
/// [`rotating_rings_frames`] directly instead).
#[derive(Debug, Clone, Copy)]
pub enum Illusion {
    /// `tile`: the side length, in source pixels, of one checkerboard
    /// tile. `offset`: how far (in pixels) each row's tiles shift
    /// relative to the row above it.
    CafeWall { tile: u32, offset: u32 },
    /// `cell`: grid spacing in source pixels. `line_width`: thickness of
    /// each white grid line.
    HermannGrid { cell: u32, line_width: u32 },
    /// `lines`: how many horizontal hatched bands to draw down the image.
    TwistedCord { lines: u32 },
}

/// Generates `illusion` as a `width x height` RGB8 image.
pub fn generate(illusion: Illusion, width: u32, height: u32) -> RgbImage {
    match illusion {
        Illusion::CafeWall { tile, offset } => cafe_wall(width, height, tile.max(1), offset),
        Illusion::HermannGrid { cell, line_width } => {
            hermann_grid(width, height, cell.max(1), line_width.max(1))
        }
        Illusion::TwistedCord { lines } => twisted_cord(width, height, lines.max(1)),
    }
}

/// Generates `illusion` and renders it through `config`/`target`, exactly
/// like [`crate::image::render_image_file`] would for a real photo —
/// see the module docs for why that's a good pairing, especially with
/// [`crate::config::Config::dither`] set.
pub fn render_illusion(
    illusion: Illusion,
    width: u32,
    height: u32,
    config: &Config,
    target: &RenderTarget,
) -> Result<RenderOutput> {
    let img = DynamicImage::ImageRgb8(generate(illusion, width, height));
    crate::image::render_dynamic_image(&img, config, target)
}

fn cafe_wall(width: u32, height: u32, tile: u32, offset: u32) -> RgbImage {
    let mortar = (tile / 4).max(1);
    let period = tile + mortar;
    let cycle = 2 * tile;
    let mut img = RgbImage::new(width, height);
    for y in 0..height {
        let row = y / period;
        let within_row = y % period;
        for x in 0..width {
            let color = if within_row < mortar {
                ImgRgb([128, 128, 128])
            } else {
                let shift = (row * offset) % cycle;
                let xx = (x + shift) % cycle;
                if xx < tile {
                    ImgRgb([10, 10, 10])
                } else {
                    ImgRgb([245, 245, 245])
                }
            };
            img.put_pixel(x, y, color);
        }
    }
    img
}

fn hermann_grid(width: u32, height: u32, cell: u32, line_width: u32) -> RgbImage {
    let mut img = RgbImage::from_pixel(width, height, ImgRgb([15, 15, 15]));
    for y in 0..height {
        for x in 0..width {
            if x % cell < line_width || y % cell < line_width {
                img.put_pixel(x, y, ImgRgb([240, 240, 240]));
            }
        }
    }
    img
}

fn twisted_cord(width: u32, height: u32, lines: u32) -> RgbImage {
    let mut img = RgbImage::from_pixel(width, height, ImgRgb([250, 250, 250]));
    let band_h = (height / lines).max(1);
    let spacing: u32 = 6;

    for band in 0..lines {
        let y0 = band * band_h;
        let y1 = (y0 + band_h).min(height);
        if y0 >= height {
            break;
        }
        let mid = (y0 + y1) / 2;
        let angle_sign: i32 = if band % 2 == 0 { 1 } else { -1 };

        for x in 0..width {
            img.put_pixel(x, mid.min(height - 1), ImgRgb([10, 10, 10]));
        }

        let mut x = 0u32;
        while x < width {
            for dy in -3i32..=3 {
                let yy = mid as i32 + dy * angle_sign;
                let xx = x as i32 + dy;
                if yy >= 0 && (yy as u32) < height && xx >= 0 && (xx as u32) < width {
                    img.put_pixel(xx as u32, yy as u32, ImgRgb([10, 10, 10]));
                }
            }
            x += spacing;
        }
    }
    img
}

/// A [`PixelAnimation`] driving the "rotating rings" motion illusion (see
/// the module docs): rings, centered at `(center_x, center_y)` in grid-cell
/// coordinates, `ring_width` cells apart, cycle through a repeating
/// 4-phase asymmetric brightness sequence (dark, mid-dark, light,
/// mid-light) that shifts outward by `speed` rings per animation frame.
/// The vertical distance is scaled by `2.0` to roughly correct for a
/// terminal character cell being about twice as tall as it is wide, so
/// the rings read as circular rather than elliptical.
pub fn rotating_rings_animation(
    center_x: f32,
    center_y: f32,
    ring_width: f32,
    speed: f32,
) -> PixelAnimation {
    let ring_width = ring_width.max(0.5);
    PixelAnimation::custom(move |x, y, frame, _ch| {
        let dx = x as f32 - center_x;
        let dy = (y as f32 - center_y) * 2.0;
        let dist = (dx * dx + dy * dy).sqrt();
        let phase = (dist / ring_width + frame as f32 * speed).rem_euclid(4.0);
        let level: u8 = match phase as u32 {
            0 => 25,
            1 => 95,
            2 => 235,
            _ => 165,
        };
        Rgb::new(level, level, level)
    })
}

/// Convenience: generates `frame_count` frames of the "rotating rings"
/// illusion directly as ANSI text (no [`Config`]/image pipeline involved —
/// this is pure character-grid animation, via
/// [`crate::colorize::colorize_frames`]), filling a `width x height` block
/// of `fill_char` and animating its color with
/// [`rotating_rings_animation`]. `rings` sets how many concentric rings
/// fit across the shorter of `width`/`height`; `speed` is rings of
/// rotation per frame.
pub fn rotating_rings_frames(
    width: usize,
    height: usize,
    rings: u32,
    speed: f32,
    frame_count: u64,
    fill_char: char,
) -> Vec<String> {
    let lines: Vec<String> = (0..height)
        .map(|_| std::iter::repeat(fill_char).take(width).collect())
        .collect();
    let ring_width = (width.max(height) as f32 / 2.0) / (rings.max(1) as f32);
    let anim = rotating_rings_animation(width as f32 / 2.0, height as f32 / 2.0, ring_width, speed);
    colorize_frames(&lines, &anim, frame_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cafe_wall_generates_requested_dimensions() {
        let img = generate(Illusion::CafeWall { tile: 8, offset: 4 }, 32, 16);
        assert_eq!(img.width(), 32);
        assert_eq!(img.height(), 16);
    }

    #[test]
    fn hermann_grid_has_white_lines_at_expected_spacing() {
        let img = generate(
            Illusion::HermannGrid {
                cell: 10,
                line_width: 2,
            },
            20,
            20,
        );
        assert_eq!(img.get_pixel(0, 5).0, [240, 240, 240]);
        assert_eq!(img.get_pixel(5, 5).0, [15, 15, 15]);
    }

    #[test]
    fn twisted_cord_does_not_panic_on_small_images() {
        let img = generate(Illusion::TwistedCord { lines: 3 }, 12, 9);
        assert_eq!((img.width(), img.height()), (12, 9));
    }

    #[test]
    fn rotating_rings_frames_returns_requested_frame_count_and_dimensions() {
        let frames = rotating_rings_frames(6, 4, 3, 0.5, 5, '#');
        assert_eq!(frames.len(), 5);
        for frame in &frames {
            assert_eq!(frame.lines().count(), 4);
        }
    }

    #[test]
    fn rotating_rings_animation_changes_over_frames_at_a_fixed_cell() {
        let anim = rotating_rings_animation(5.0, 5.0, 2.0, 1.0);
        let lines = vec!["#".repeat(10); 10];
        let frame0 = crate::colorize::colorize_lines(&lines, &anim, 0);
        let frame1 = crate::colorize::colorize_lines(&lines, &anim, 1);
        assert_ne!(frame0, frame1);
    }
}
