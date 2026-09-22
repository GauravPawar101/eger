# Tests & benchmarks added to `eger`

> **Update:** this package now also includes new crate functionality —
> color transforms, expanded dithering/colorize/wasm support — not just
> tests and benchmarks for the original code. See "New crate features"
> below for what changed and why, before the original tests/benchmarks
> writeup.

## New crate features

- **`src/color.rs` (new module)** — a `ColorTransform` enum (`Grayscale`,
  `Invert`, `Sepia`, `Brightness`, `Contrast`, `Saturate`, `HueRotate`,
  `Tint`, `Compose`), each a pure `Rgb -> Rgb` function via
  `ColorTransform::apply`, plus `apply_rgb8_buffer` for transforming a
  flat `RGB8` source-image buffer before ASCII conversion. Internal
  `rgb_to_hsl`/`hsl_to_rgb` helpers back `Saturate`/`HueRotate`. **20 unit
  tests.**
- **`src/text.rs`** — `Rgb` gained ~25 named color constants (`RED`,
  `BLUE`, `ORANGE`, `PURPLE`, `SEPIA`, `TURQUOISE`, ...) and a
  `mix(other, t)` lerp method used by the new `Gradient` animation and
  `Tint` transform.
- **`src/dither.rs`** — added `render_ansi_dithered_transformed(grid,
  depth, options, transform)`, which runs a `ColorTransform` on each
  cell's color *before* biasing/diffusing/quantizing, so e.g. a
  `Grayscale` transform still gets the full benefit of dithering on a
  coarse palette rather than transforming already-banded output. The
  original `render_ansi_dithered` signature is untouched (it now just
  calls the new function with no transform) — no breaking change, and
  `DitherOptions` stays `Copy` (the transform is a separate parameter
  rather than a new field, since `ColorTransform::Compose` holds a `Vec`
  and isn't `Copy`). **6 new tests** on top of the existing dither tests.
- **`src/colorize.rs`** — two new `PixelAnimation` variants:
  `Gradient { from, to, horizontal }` (a static two-color sweep, the
  simplest possible `Rgb::mix`-based effect, for the common
  "tint between two brand colors" case without a `custom` closure) and
  `Transformed { base, transform }` (wraps *any* other animation,
  including another `Transformed`, and runs a `ColorTransform` on its
  output — composes cleanly instead of needing a bespoke variant per
  combination, e.g. "desaturated rainbow"). **8 new tests.**
- **`src/script.rs` / `src/dither.rs`** — `Script` and `DitherMethod`
  (both already plain fieldless enums) are now directly
  `#[wasm_bindgen]`-annotated behind `cfg_attr(feature = "wasm", ...)`,
  so they cross the JS boundary as themselves with no duplicate mirror
  enum needed.
- **`src/wasm.rs` — substantially extended.** The original
  `image_bytes_to_string`/`image_bytes_to_lines`/`gif_bytes_to_frame_strings`/
  `text_frame_at` are kept as-is for backward compatibility. New:
  - **ANSI-16/256/TrueColor + dithering + color transforms:**
    `ColorDepthKind` (`Plain`/`Ansi16`/`Ansi256`/`TrueColor`) and
    `ColorTransformKind` (mirrors `ColorTransform`, one `f32` "amount"
    param covers every kind that needs one) drive
    `image_bytes_to_string_ex`/`image_bytes_to_lines_ex` — the
    full-featured counterparts to the original bool-color functions.
  - **Script/ramp:** `image_bytes_to_string_with_script(bytes, max_width,
    script: Script, dark, depth, dither, dither_levels, transform,
    transform_amount)`.
  - **Colorize:** `AnimationKind` (`Rainbow`/`Wave`/`Pulse`/`Blink`/
    `Plasma`/`Ripple`/`Gradient`) + `colorize_plain_text(...)` recolors
    already-rendered plain text (from any `Plain`-depth function, or
    `banner_lines_wasm` joined with `"\n"`) at a given frame, with an
    optional layered `ColorTransformKind` on top.
  - **Banner:** `banner_lines_wasm(text)` and `banner_frame_at(text,
    animation, frame_index)`.
  - **Illusions:** `IllusionKind` (`CafeWall`/`HermannGrid`/
    `TwistedCord`) + `illusion_to_string(...)` (generate → convert →
    render, no image bytes needed) and `rotating_rings_to_lines(...)`
    (thin wrapper over `illusions::rotating_rings_frames`).

  **19 new tests** (14 run and pass natively — see "Verifying the wasm
  module" below; 5 are `Vec<JsValue>`-returning and only run under real
  `wasm-bindgen-test`).
- **`benches/color_transform_bench.rs` (new)** — per-color transform cost
  for every kind plus a 4-step `Compose` chain, the `apply_rgb8_buffer`
  whole-image path, transformed-vs-untransformed dithering, and
  transformed-vs-plain colorize animation. Ran to completion, no errors.

### Verifying the wasm module

`src/wasm.rs` is gated `#[cfg(all(target_arch = "wasm32", feature =
"wasm"))]`, and this sandbox has no `wasm32-unknown-unknown` target
installed (no `rustup`), so it can't be compiled or tested in its normal
configuration here. However, I found that `#[wasm_bindgen]` on the plain
fieldless enums this module uses (`ColorDepthKind`, `ColorTransformKind`,
`AnimationKind`, `IllusionKind`, `TextAnimationKind`, plus the
now-`#[wasm_bindgen]`-annotated `Script`/`DitherMethod`) **does** expand
and typecheck on the native target — so, purely for verification, I
temporarily relaxed the module's `target_arch` restriction to
`#[cfg(feature = "wasm")]`, compiled and ran its test suite natively, then
restored the real `target_arch = "wasm32"` gate before finalizing this
package (confirmed: `cargo test --features wasm` now shows the module
absent again on native, as it should be — see below).

What that native run showed:
- **14 of 19 tests pass for real** — every function that returns a plain
  `String`/`Result<String, JsError>` (all the `_ex`/`_with_script`/
  `colorize_plain_text`/`illusion_to_string`/`text_frame_at` logic,
  i.e. essentially all of the actual rendering/color logic this module
  adds) executes correctly, not just typechecks.
- **5 tests are marked `#[cfg_attr(not(target_arch = "wasm32"), ignore =
  "...")]`** — these are the ones returning `Vec<JsValue>`
  (`image_bytes_to_lines_ex`, `gif_bytes_to_frame_strings`,
  `banner_lines_wasm`, `banner_frame_at`, `rotating_rings_to_lines`).
  `wasm_bindgen::JsValue::from_str` compiles fine natively but genuinely
  panics at runtime off `wasm32` ("function not implemented on
  non-wasm32 targets") — confirmed by running them before adding the
  `ignore` marker. This is expected: JS-value construction fundamentally
  needs the real wasm runtime. They're left in the source, correctly
  gated, ready to run for real under `wasm-bindgen-test` — but they are
  genuinely **not independently verified** by anything I could run here,
  beyond compiling. Everything each of them calls internally
  (`strings_to_js`, and the same `banner_lines`/`banner_frames`/
  `rotating_rings_frames` functions the crate's own non-wasm tests
  already exercise) is separately tested elsewhere in this package,
  which narrows the untested surface to `strings_to_js`'s one-line
  `JsValue::from_str` mapping itself.
- `cargo build --features "wasm,video"` also compiles cleanly (confirms
  no naming/type conflicts between the two optional modules, e.g. the new
  `ColorDepthKind` was deliberately named to avoid colliding with
  `video::ColorMode`).

If you want full coverage of the `Vec<JsValue>` functions, add a
`wasm32-unknown-unknown` target (`rustup target add
wasm32-unknown-unknown`) and run them with `wasm-pack test --headless
--chrome` (or `--firefox`/`--node`) using `wasm-bindgen-test` — the
existing `#[test]` functions would need converting to `#[wasm_bindgen_test]`
for that runner, which I didn't do here since it can't be executed in
this sandbox either way.

### New `Cargo.toml` pin

`wasm-bindgen` is now pinned to `=0.2.92` (down from an unpinned `"0.2"`)
for the same reason as the other dev-dependency pins: `wasm-bindgen-shared`
0.2.128+ needs rustc 1.77+, and this sandbox only has 1.75. Loosen this
along with the other pins on a modern toolchain.

---


This package contains your original 15 source files (`src/`), unmodified,
plus a new integration-test suite and a full Criterion benchmark suite.
Everything here was actually **compiled and run**, not just written —
`iascii` turned out to be a real published crate on crates.io, so this
whole project builds for real.

## What's in the box

```
eger/
├── Cargo.toml                  # dependencies + bench registration (see notes below)
├── src/                        # your original 15 files, plus new src/color.rs
│                                #   and additions to dither.rs, colorize.rs,
│                                #   text.rs, script.rs, wasm.rs — see
│                                #   "New crate features" above
├── tests/
│   └── integration_test.rs     # 15 cross-module tests (17 with --features video)
├── benches/
│   ├── dither_bench.rs         # every DitherMethod x every ColorDepth x 2 sizes,
│   │                           #   + a posterization-level sweep, + iascii baseline
│   ├── palette_bench.rs        # quantization cost in isolation, by ColorDepth
│   ├── colorize_bench.rs       # every PixelAnimation, frame-size scaling,
│   │                           #   colorize_frames/colorize_sequence parallel paths
│   ├── image_bench.rs          # single-image conversion by output size,
│   │                           #   batch-parallel rendering by thread count,
│   │                           #   dithered vs. plain render cost
│   ├── banner_text_bench.rs    # banner_lines/banner_frames/text_frames by
│   │                           #   animation style, text length, frame count
│   ├── segment_bench.rs        # SegmentMap::detect by region count and grid
│   │                           #   size, render_segments with mixed styles,
│   │                           #   SegmentMap::manual construction
│   ├── illusions_bench.rs      # each Illusion's raw generation, the full
│   │                           #   render_illusion pipeline, rotating rings
│   └── color_transform_bench.rs # every ColorTransform kind, transformed vs.
│                                #   plain dithering/colorize (new)
└── TESTS_AND_BENCHMARKS.md     # this file
```

Every module you originally uploaded (`banner`, `colorize`, `config`,
`dither`, `error`, `illusions`, `image`, `lib`, `palette`, `render`,
`script`, `segment`, `text`, `video`, `wasm`) already had solid
`#[cfg(test)]` unit tests of its own; those, and each module's body, are
untouched except where "New crate features" above says otherwise. What's
new is `src/color.rs`, the additions to `dither.rs`/`colorize.rs`/
`text.rs`/`script.rs`/`wasm.rs`, `tests/integration_test.rs` (cross-module
workflows a real caller would actually chain together), and the entire
`benches/` directory.

## Verified results

Run in this environment (Ubuntu, `apt`-installed Rust 1.75 — see the
version-pin note below):

| Suite | Result |
|---|---|
| Unit tests (`src/`), default features | **137 passed**, 0 failed (104 original + 33 from the new color/dither-transform/colorize/gradient additions) |
| Unit tests, `--features video` | **146 passed**, 0 failed (real `ffmpeg`/`ffprobe`, not mocked) |
| Unit tests, `--features wasm` (native target) | **137 passed**, 0 failed — `src/wasm.rs` correctly excluded (needs real `wasm32`); see "Verifying the wasm module" above for the separate native-only check I ran against it |
| `tests/integration_test.rs`, default features | **15 passed**, 0 failed |
| `tests/integration_test.rs`, `--features video` | **17 passed**, 0 failed |
| Doc-tests | **2 passed** |
| All 8 bench files | compile clean, **every one run to completion** with real timing output, no panics |

## What the integration tests cover

Each module's own unit tests necessarily test that module in isolation.
`tests/integration_test.rs` instead chains modules together the way a
real program would:

- `Config` → `image::render_image_file` → every `RenderTarget` (String,
  Lines, File, and a plain/no-color config), checked against real PNGs
  written with the `image` crate
- `Config::dither` → `render_dynamic_image`, across **every**
  `DitherMethod` × `ColorDepth` combination, plus a direct check that
  dithered vs. plain output actually diverges on a coarse-palette gradient
- Every `Illusion` variant generated and pushed through the full
  `Config`/dither/render pipeline
- `SegmentMap::detect` on a real two-color converted image, naming
  segments, styling one with a flat color and one with a `PixelAnimation`,
  and checking the rendered ANSI output and the `overlay_ids` diagnostic
- `banner_lines` output re-colored through every built-in `PixelAnimation`
  via `colorize_frames`, plus a check that `banner_frames`' own
  per-character coloring and `colorize_frames`' per-cell coloring are
  genuinely different (neither is an accidental no-op wrapper of the
  other)
- A real GIF (encoded with the `image` crate's `GifEncoder`) decoded via
  `gif_to_lines`, then recolored with both `colorize_sequence` and
  `colorize_joined_sequence`, checked for equivalence
- `colorize_grid` vs. `colorize_lines` equivalence over the same
  converted grid
- `render_batch_parallel` over a real directory of PNGs (plus a
  non-matching file to confirm the regex filter excludes it)
- Every `Script` ramp driving a real `Config` → conversion
- `text_frames` output fed into `render::diff_frame`
- Error propagation: a missing file surfaces as
  `EgerError::Config(ConfigError::FileNotFound(_))`
- **`--features video` only:** a full `probe` → `video_to_lines` →
  `video_to_file` pipeline against a real `ffmpeg`-generated test clip,
  with the written file's `FRAME_SEPARATOR`-delimited frame count checked;
  plus `video_to_lines_plain` → `colorize_joined_sequence` recoloring

## What the benchmarks cover

All benchmarks use [Criterion](https://github.com/bheisler/criterion.rs)
with `harness = false`. Run them with:

```sh
cargo bench                       # everything
cargo bench --bench dither_bench  # one file
cargo bench -- pattern            # filter by benchmark name substring
```

HTML reports land in `target/criterion/report/index.html` (needs
`gnuplot` for the nicer plots; falls back to a built-in plotting backend
otherwise, no missing-gnuplot warning breaks anything).

- **`dither_bench`** — `DitherMethod::{None, FloydSteinberg, Atkinson,
  Bayer2, Bayer4, Bayer8}` × `ColorDepth::{Ansi16, Ansi256, TrueColor}` at
  two grid sizes, a `DitherOptions::levels` sweep at `TrueColor`, and a
  baseline against `iascii::render::render_ansi` (useful since
  `DitherMethod::None` is defined to be byte-identical to it).
- **`palette_bench`** — isolates quantization cost (via
  `DitherMethod::None`, which does no error diffusion) across color
  depths, on a synthetic-noise grid designed to stress the nearest-color
  search's worst case, plus the same comparison against
  `iascii::render::render_ansi`.
- **`colorize_bench`** — every built-in `PixelAnimation` on a fixed frame
  size, then one animation (`Plasma`) across four frame sizes to show
  scaling, then `colorize_frames`' and `colorize_sequence`'s Rayon
  parallel-generation paths at increasing frame counts.
- **`image_bench`** — single-image conversion at four output sizes on a
  synthetic photo-like (radial gradient + noise) source; batch-parallel
  rendering across 8 real PNGs at 1/2/4 threads; dithered vs. plain
  render cost at `Ansi256`.
- **`banner_text_bench`** — `banner_lines` by text length;
  `banner_frames` and `text_frames` by animation style (including a
  custom closure animation); `banner_frames` scaling with frame count.
- **`segment_bench`** — `SegmentMap::detect` by number of distinct color
  regions and by grid size; `render_segments` with a mix of `Bold` /
  `Color` / `Animation` styles; `SegmentMap::manual` construction cost by
  region count.
- **`illusions_bench`** — raw pixel generation for `CafeWall` /
  `HermannGrid` / `TwistedCord`; the full `render_illusion` pipeline
  (generate → convert → render); `rotating_rings_frames` by frame count.

## A note on the `Cargo.toml` version pins

This sandbox only had `rustc`/`cargo` 1.75 available via `apt` (no
`rustup`), so several dependencies are pinned to versions whose MSRV is
still ≤1.75 — otherwise `rayon-core`, `half` (via criterion/ciborium), and
`clap`/`getrandom`/`fastrand` (via criterion/tempfile) all pull in
versions requiring rustc 1.80/1.81. Every pin is commented in
`Cargo.toml`. **On a normal, reasonably current Rust toolchain you can
loosen or remove all of them** — they're not needed for correctness, only
for building under this environment's old compiler.

## `wasm` feature

See "Verifying the wasm module" near the top of this document — `src/wasm.rs`
was substantially extended, and while it can't be built for real `wasm32`
in this sandbox, most of its logic (14/19 new tests) was verified for
real via a temporary native-target relaxation. The remaining gap is
documented there precisely.
