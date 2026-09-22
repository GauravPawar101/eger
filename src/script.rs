//! Multi-script character ramps.
//!
//! `iascii::ramp::Ramp` ships several Latin-script and symbolic ramps
//! ([`iascii::ramp::SMOOTH_GRADIENT`] and friends) plus
//! [`iascii::ramp::Ramp::new_custom`] for supplying your own. [`Script`]
//! is a small catalogue of ready-made ramps built from *other* Unicode
//! scripts/blocks, for ASCII art that reads as e.g. Cyrillic, CJK, or
//! Devanagari brushwork instead of the usual `@%#*+=-:. ` Latin ramp —
//! each entry's characters are ordered light-to-heavy by approximate
//! visual density/ink coverage, the same property a luminance ramp needs.
//!
//! Every ramp here is validated through [`iascii::ramp::Ramp::new_custom`]
//! exactly like a hand-written custom ramp would be, so the same rules
//! apply: build with [`Script::ramp`] and feed it straight to
//! [`crate::config::ConfigBuilder::ramp`], or use the
//! [`crate::config::ConfigBuilder::script`] shortcut.
//!
//! A couple of scripts (Arabic, Hebrew) are RTL/contextual-shaping scripts
//! whose letterforms are designed to join with their neighbors; rendered
//! one isolated codepoint per grid cell they're still legible as
//! *texture* (which is all an ASCII-art ramp needs) but won't read as
//! connected words — the same caveat `iascii::ramp`'s own module docs note
//! for multi-codepoint emoji ramps.

use iascii::error::ImageError;
use iascii::ramp::{Ramp, RampType};
#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::wasm_bindgen;

/// A ready-made character ramp drawn from a non-Latin Unicode script or
/// block, ordered light-to-heavy.
#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// The default Latin/ASCII ramp — included so callers can treat
    /// `Script` as the one place that selects a ramp, without special-
    /// casing "no script chosen".
    Latin,
    /// Unicode block/shade elements (`░▒▓█` and friends) — script-neutral,
    /// and the steadiest choice to combine with [`crate::dither`], since
    /// its cells are already solid-ink blocks rather than glyphs whose
    /// own shape carries part of the tonal signal.
    Block,
    /// Braille Patterns block (`U+2800..U+28FF`), ordered by dot count —
    /// a compact, high "resolution" ramp (256 distinct density steps).
    Braille,
    /// Cyrillic letterforms, ordered by approximate stroke density.
    Cyrillic,
    /// Greek letterforms, ordered by approximate stroke density.
    Greek,
    /// A representative set of CJK Unified Ideographs, ordered by stroke
    /// count (a coarse proxy for ink density).
    Cjk,
    /// Devanagari letterforms and matras, ordered by approximate stroke
    /// density.
    Devanagari,
    /// Hebrew letterforms, ordered by approximate stroke density. See the
    /// module docs' note on contextual/RTL scripts.
    Hebrew,
    /// Arabic letterforms (isolated forms), ordered by approximate stroke
    /// density. See the module docs' note on contextual/RTL scripts.
    Arabic,
}

impl Script {
    fn chars(self) -> &'static str {
        match self {
            // Mirrors iascii::ramp::SMOOTH_GRADIENT so `Script::Latin`
            // is a drop-in "no script selected" default.
            Script::Latin => {
                " .'`^\",:;Il!i><~+_-?][}{1)(|\\/jtfrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"
            }
            Script::Block => " ░▒▓█",
            Script::Braille => braille_ramp(),
            Script::Cyrillic => " .,\u{0301}\u{0300}ıі´гвсэоауеьъиклнптмчшщДЖЮ",
            Script::Greek => " .,'ιτ\u{03B9}ρνςολυπχθδφκγζξψβμω",
            Script::Cjk => " .一二人入八十丁力乃三口土工才寸下大丈山川已己工日月木水火田",
            Script::Devanagari => " .़्ािीुूॐअइउएकचटतनपमरलवसह",
            Script::Hebrew => " .יוזןטבכלמנסעפצקרשתךםףץ",
            Script::Arabic => " .ءابتثجحخدذرسصطعفقكلمنهوي",
        }
    }

    /// Builds an `iascii` [`Ramp`] from this script's characters.
    ///
    /// Validated exactly like [`Ramp::new_custom`] validates any
    /// hand-written ramp; the only way this can fail is if the string
    /// above accidentally contains a raw control character or a leading
    /// detached Unicode variation selector, neither of which any variant
    /// here does — but the `Result` is still surfaced (rather than
    /// unwrapped) so a future edit to the character lists above can never
    /// turn into a silent panic.
    pub fn ramp(self) -> Result<Ramp, ImageError> {
        Ramp::new_custom(self.chars())
    }

    /// Convenience: [`Script::ramp`] wrapped in [`RampType::Dark`] (dark
    /// glyphs represent low luminance, the usual "light background text
    /// terminal" convention) or [`RampType::Light`] (inverted, for a dark
    /// terminal background where heavier glyphs should mark *bright*
    /// source pixels).
    pub fn ramp_type(self, dark: bool) -> Result<RampType, ImageError> {
        let ramp = self.ramp()?;
        Ok(if dark {
            RampType::Dark(ramp)
        } else {
            RampType::Light(ramp)
        })
    }
}

/// Builds the 256-entry Braille Patterns ramp (`U+2800..=U+28FF`), ordered
/// by the number of raised dots in each cell's 2x4 dot matrix — i.e. by
/// the bit count of the codepoint's low byte, since Braille Patterns
/// assigns each of the 8 dot positions to one bit offset from the block's
/// base `U+2800`.
fn braille_ramp() -> &'static str {
    use std::sync::OnceLock;
    static RAMP: OnceLock<&'static str> = OnceLock::new();
    RAMP.get_or_init(|| {
        let mut cells: Vec<u8> = (0u8..=255).collect();
        cells.sort_by_key(|c| c.count_ones());
        let s: String = cells
            .into_iter()
            .map(|c| char::from_u32(0x2800 + c as u32).unwrap())
            .collect();
        // Leaked once into a 'static str: this crate never builds more
        // than a handful of `Config`s per process, so trading a few
        // hundred bytes of one-time allocation for a `&'static str` (what
        // every other `Script` variant already is) is simpler than
        // threading an owned `String` through `Ramp::CustomRamp(Cow::Owned(..))`
        // at every call site.
        Box::leak(s.into_boxed_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_script_builds_a_valid_ramp() {
        for script in [
            Script::Latin,
            Script::Block,
            Script::Braille,
            Script::Cyrillic,
            Script::Greek,
            Script::Cjk,
            Script::Devanagari,
            Script::Hebrew,
            Script::Arabic,
        ] {
            let ramp = script.ramp();
            assert!(ramp.is_ok(), "{script:?} ramp should validate: {ramp:?}");
        }
    }

    #[test]
    fn braille_ramp_has_256_entries_ordered_by_dot_count() {
        let s = Script::Braille.chars();
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 256);
        let counts: Vec<u32> = chars
            .iter()
            .map(|c| (*c as u32 - 0x2800).count_ones())
            .collect();
        assert!(counts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn dark_and_light_variants_wrap_correctly() {
        assert!(matches!(
            Script::Block.ramp_type(true).unwrap(),
            RampType::Dark(_)
        ));
        assert!(matches!(
            Script::Block.ramp_type(false).unwrap(),
            RampType::Light(_)
        ));
    }
}
