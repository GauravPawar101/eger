//! Reusable color transforms, shared by [`crate::dither`] (applied to a
//! cell's color before quantization/diffusion — see
//! [`crate::dither::render_ansi_dithered_transformed`]),
//! [`crate::colorize`] ([`crate::colorize::PixelAnimation::Transformed`]
//! wraps any other animation and applies one of these on top of it), and
//! [`crate::wasm`] (exposed directly for browser callers that just want to
//! post-process already-rendered art, e.g. a "grayscale preview" toggle).
//!
//! Every transform is a pure `Rgb -> Rgb` function — see
//! [`ColorTransform::apply`] — so they compose trivially
//! ([`ColorTransform::Compose`]) and are cheap enough to run per-cell
//! without any lookup tables or precomputation.
//!
//! [`ColorTransform::apply_rgb8_buffer`] additionally exists for the case
//! where you want to transform a *source image* before it's even converted
//! to ASCII (so the transform also influences which characters `iascii`'s
//! luminance ramp picks, not just the color drawn) — see
//! [`crate::wasm::image_bytes_to_string_ex`] for how the wasm bindings use
//! that.

use crate::text::Rgb;

/// A named or composed color transform: a pure function from one [`Rgb`]
/// to another. See the module docs for where these plug in.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorTransform {
    /// No-op — included so callers can hold an `Option<ColorTransform>`-free
    /// "maybe transform" value uniformly (used by the `wasm` bindings,
    /// which take a transform kind as a plain enum with no `Option`).
    None,
    /// Desaturates to the [ITU-R BT.601](https://en.wikipedia.org/wiki/Rec._601)
    /// luma-weighted gray (`0.299R + 0.587G + 0.114B`) — the same weights
    /// most "black & white photo" filters use, rather than a flat average.
    Grayscale,
    /// Inverts each channel (`255 - c`) — a photographic negative.
    Invert,
    /// Classic sepia-tone matrix (the same transform Instagram-style photo
    /// filters use), tinting the image toward [`Rgb::SEPIA`].
    Sepia,
    /// Multiplies every channel by `factor` (clamped to `[0.0, 4.0]` so a
    /// mistyped huge value can't silently do nothing via saturation at
    /// `255` for every pixel). `1.0` is a no-op, `< 1.0` darkens, `> 1.0`
    /// brightens.
    Brightness(f32),
    /// Scales each channel's distance from mid-gray (128) by `factor`
    /// (clamped to `[0.0, 4.0]`). `1.0` is a no-op, `0.0` flattens to flat
    /// gray, `> 1.0` increases contrast.
    Contrast(f32),
    /// Scales the color's distance from its own grayscale value by
    /// `factor` (clamped to `[0.0, 4.0]`) in HSL space. `1.0` is a no-op,
    /// `0.0` is equivalent to [`ColorTransform::Grayscale`] (with equal
    /// weighting per channel rather than [`ColorTransform::Grayscale`]'s
    /// luma weights — the two intentionally aren't bit-identical at
    /// `factor: 0.0`), `> 1.0` boosts saturation.
    Saturate(f32),
    /// Rotates hue by `degrees` in HSL space, leaving saturation and
    /// lightness unchanged. Wraps around 360°, so any `f32` is valid.
    HueRotate(f32),
    /// Blends `amount` (clamped to `[0.0, 1.0]`) of the way from the input
    /// color toward `color` — `0.0` is a no-op, `1.0` replaces the input
    /// entirely with `color`.
    Tint { color: Rgb, amount: f32 },
    /// Applies every transform in order, each seeing the previous one's
    /// output.
    Compose(Vec<ColorTransform>),
}

impl ColorTransform {
    /// Applies this transform to a single color.
    pub fn apply(&self, c: Rgb) -> Rgb {
        match self {
            ColorTransform::None => c,
            ColorTransform::Grayscale => {
                let v = (0.299 * f32::from(c.r())
                    + 0.587 * f32::from(c.g())
                    + 0.114 * f32::from(c.b()))
                .round()
                .clamp(0.0, 255.0) as u8;
                Rgb::new(v, v, v)
            }
            ColorTransform::Invert => Rgb::new(255 - c.r(), 255 - c.g(), 255 - c.b()),
            ColorTransform::Sepia => {
                let (r, g, b) = (f32::from(c.r()), f32::from(c.g()), f32::from(c.b()));
                let tr = (0.393 * r + 0.769 * g + 0.189 * b)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let tg = (0.349 * r + 0.686 * g + 0.168 * b)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let tb = (0.272 * r + 0.534 * g + 0.131 * b)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                Rgb::new(tr, tg, tb)
            }
            ColorTransform::Brightness(factor) => {
                let factor = factor.clamp(0.0, 4.0);
                let ch = |v: u8| (f32::from(v) * factor).round().clamp(0.0, 255.0) as u8;
                Rgb::new(ch(c.r()), ch(c.g()), ch(c.b()))
            }
            ColorTransform::Contrast(factor) => {
                let factor = factor.clamp(0.0, 4.0);
                let ch = |v: u8| {
                    (((f32::from(v) - 128.0) * factor) + 128.0)
                        .round()
                        .clamp(0.0, 255.0) as u8
                };
                Rgb::new(ch(c.r()), ch(c.g()), ch(c.b()))
            }
            ColorTransform::Saturate(factor) => {
                let factor = factor.clamp(0.0, 4.0);
                let (h, s, l) = rgb_to_hsl(c);
                hsl_to_rgb(h, (s * factor).clamp(0.0, 1.0), l)
            }
            ColorTransform::HueRotate(degrees) => {
                let (h, s, l) = rgb_to_hsl(c);
                hsl_to_rgb((h + degrees).rem_euclid(360.0), s, l)
            }
            ColorTransform::Tint { color, amount } => c.mix(*color, *amount),
            ColorTransform::Compose(steps) => steps.iter().fold(c, |acc, step| step.apply(acc)),
        }
    }

    /// Applies this transform in place to a flat `RGB8` pixel buffer (3
    /// bytes per pixel, row-major — the layout `image::RgbImage::as_raw`
    /// and [`crate::illusions::generate`]'s output use), for transforming
    /// a *source* image before ASCII conversion rather than transforming
    /// already-rendered cell colors. A no-op ([`ColorTransform::None`])
    /// still walks the buffer; callers on a hot path should skip calling
    /// this at all when they know the transform is `None`.
    pub fn apply_rgb8_buffer(&self, buf: &mut [u8]) {
        for pixel in buf.chunks_exact_mut(3) {
            let out = self.apply(Rgb::new(pixel[0], pixel[1], pixel[2]));
            pixel[0] = out.r();
            pixel[1] = out.g();
            pixel[2] = out.b();
        }
    }
}

/// Converts an 8-bit-per-channel [`Rgb`] to `(hue_degrees, saturation,
/// lightness)`, each of `saturation`/`lightness` in `[0.0, 1.0]` and `hue`
/// in `[0.0, 360.0)`. Used internally by [`ColorTransform::Saturate`] and
/// [`ColorTransform::HueRotate`]; exposed since it's independently useful
/// to anything else in this crate that wants HSL math (unlike
/// [`crate::text::hsv_to_rgb`]'s HSV, which the built-in animations use
/// for their "full brightness sweep" effects — HSL is the more natural
/// space for "keep the *lightness* fixed and only touch hue/saturation"
/// transforms like these).
pub(crate) fn rgb_to_hsl(c: Rgb) -> (f32, f32, f32) {
    let r = f32::from(c.r()) / 255.0;
    let g = f32::from(c.g()) / 255.0;
    let b = f32::from(c.b()) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h.rem_euclid(360.0), s, l)
}

/// Inverse of [`rgb_to_hsl`].
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    if s <= f32::EPSILON {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return Rgb::new(v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Rgb::new(
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_produces_equal_channels() {
        let out = ColorTransform::Grayscale.apply(Rgb::new(10, 200, 50));
        assert_eq!(out.r(), out.g());
        assert_eq!(out.g(), out.b());
    }

    #[test]
    fn grayscale_is_a_fixed_point_for_gray_input() {
        let gray = Rgb::new(100, 100, 100);
        assert_eq!(ColorTransform::Grayscale.apply(gray), gray);
    }

    #[test]
    fn invert_is_its_own_inverse() {
        let c = Rgb::new(10, 200, 50);
        let twice = ColorTransform::Invert.apply(ColorTransform::Invert.apply(c));
        assert_eq!(twice, c);
    }

    #[test]
    fn invert_black_is_white_and_vice_versa() {
        assert_eq!(ColorTransform::Invert.apply(Rgb::BLACK), Rgb::WHITE);
        assert_eq!(ColorTransform::Invert.apply(Rgb::WHITE), Rgb::BLACK);
    }

    #[test]
    fn sepia_removes_blue_dominance() {
        // A cool blue should come out warmer (more red/green than blue)
        // after a sepia tint, since that's the whole point of the filter.
        let out = ColorTransform::Sepia.apply(Rgb::new(20, 20, 200));
        assert!(out.r() > out.b());
    }

    #[test]
    fn brightness_one_is_identity() {
        let c = Rgb::new(30, 128, 240);
        assert_eq!(ColorTransform::Brightness(1.0).apply(c), c);
    }

    #[test]
    fn brightness_zero_is_black() {
        let c = Rgb::new(30, 128, 240);
        assert_eq!(ColorTransform::Brightness(0.0).apply(c), Rgb::BLACK);
    }

    #[test]
    fn brightness_clamps_channels_at_255() {
        let out = ColorTransform::Brightness(10.0).apply(Rgb::new(200, 200, 200));
        assert_eq!(out, Rgb::new(255, 255, 255));
    }

    #[test]
    fn contrast_one_is_identity() {
        let c = Rgb::new(10, 128, 240);
        assert_eq!(ColorTransform::Contrast(1.0).apply(c), c);
    }

    #[test]
    fn contrast_zero_collapses_to_mid_gray() {
        let out = ColorTransform::Contrast(0.0).apply(Rgb::new(10, 250, 128));
        assert_eq!(out, Rgb::new(128, 128, 128));
    }

    #[test]
    fn saturate_zero_matches_grayscale_for_a_neutral_input() {
        // For an already-gray pixel, desaturating fully must still be gray
        // (this doesn't assert bit-identity with ColorTransform::Grayscale
        // in general -- HSL vs. luma weighting differ -- just that gray
        // stays gray).
        let gray = Rgb::new(150, 150, 150);
        let out = ColorTransform::Saturate(0.0).apply(gray);
        assert_eq!(out.r(), out.g());
        assert_eq!(out.g(), out.b());
    }

    #[test]
    fn saturate_one_is_close_to_identity() {
        let c = Rgb::new(200, 40, 40);
        let out = ColorTransform::Saturate(1.0).apply(c);
        // HSL round-trip isn't bit-exact due to float rounding, but should
        // be within a couple of levels per channel.
        assert!((i16::from(out.r()) - i16::from(c.r())).abs() <= 2);
        assert!((i16::from(out.g()) - i16::from(c.g())).abs() <= 2);
        assert!((i16::from(out.b()) - i16::from(c.b())).abs() <= 2);
    }

    #[test]
    fn hue_rotate_by_360_is_close_to_identity() {
        let c = Rgb::new(200, 40, 40);
        let out = ColorTransform::HueRotate(360.0).apply(c);
        assert!((i16::from(out.r()) - i16::from(c.r())).abs() <= 2);
        assert!((i16::from(out.g()) - i16::from(c.g())).abs() <= 2);
        assert!((i16::from(out.b()) - i16::from(c.b())).abs() <= 2);
    }

    #[test]
    fn hue_rotate_by_180_swaps_a_primary_red_toward_cyan() {
        let out = ColorTransform::HueRotate(180.0).apply(Rgb::new(255, 0, 0));
        // Red (hue 0) rotated 180 degrees lands on cyan (hue 180).
        assert!(out.r() < 20);
        assert!(out.g() > 200);
        assert!(out.b() > 200);
    }

    #[test]
    fn tint_zero_is_identity_and_one_is_the_target_color() {
        let c = Rgb::new(10, 20, 30);
        assert_eq!(
            ColorTransform::Tint {
                color: Rgb::RED,
                amount: 0.0
            }
            .apply(c),
            c
        );
        assert_eq!(
            ColorTransform::Tint {
                color: Rgb::RED,
                amount: 1.0
            }
            .apply(c),
            Rgb::RED
        );
    }

    #[test]
    fn compose_applies_each_step_in_order() {
        let c = Rgb::new(10, 20, 30);
        let composed = ColorTransform::Compose(vec![
            ColorTransform::Invert,
            ColorTransform::Invert,
            ColorTransform::Grayscale,
        ])
        .apply(c);
        // Two inverts cancel out, leaving a plain grayscale of the input.
        assert_eq!(composed, ColorTransform::Grayscale.apply(c));
    }

    #[test]
    fn none_is_identity() {
        let c = Rgb::new(1, 2, 3);
        assert_eq!(ColorTransform::None.apply(c), c);
    }

    #[test]
    fn apply_rgb8_buffer_transforms_every_pixel() {
        let mut buf = vec![10u8, 20, 30, 200, 100, 50];
        ColorTransform::Invert.apply_rgb8_buffer(&mut buf);
        assert_eq!(buf, vec![245, 235, 225, 55, 155, 205]);
    }

    #[test]
    fn hsl_round_trip_preserves_grayscale() {
        let (h, s, l) = rgb_to_hsl(Rgb::new(128, 128, 128));
        assert_eq!(s, 0.0);
        let back = hsl_to_rgb(h, s, l);
        assert_eq!(back, Rgb::new(128, 128, 128));
    }

    #[test]
    fn hsl_round_trip_preserves_primary_hues() {
        for c in [Rgb::RED, Rgb::GREEN, Rgb::BLUE] {
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            assert_eq!(back, c, "round trip failed for {c:?}");
        }
    }
}
