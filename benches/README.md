# Tests & benchmarks added to `eger`

This package contains your original 15 source files (`src/`), unmodified,
plus a new integration-test suite and a full Criterion benchmark suite.
Everything here was actually **compiled and run**, not just written —
`iascii` turned out to be a real published crate on crates.io, so this
whole project builds for real.

## What's in the box

```
eger/
├── Cargo.toml                  # dependencies + bench registration (see notes below)
├── src/                        # your original 15 files, unchanged
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
│   └── illusions_bench.rs      # each Illusion's raw generation, the full
│                                #   render_illusion pipeline, rotating rings
└── TESTS_AND_BENCHMARKS.md     # this file
```

Every module you uploaded (`banner`, `colorize`, `config`, `dither`,
`error`, `illusions`, `image`, `lib`, `palette`, `render`, `script`,
`segment`, `text`, `video`, `wasm`) already had solid `#[cfg(test)]` unit
tests of its own — those are untouched. What's new is the
`tests/integration_test.rs` file (cross-module workflows a real caller
would actually chain together) and the entire `benches/` directory.

## Verified results

Run in this environment (Ubuntu, `apt`-installed Rust 1.75 — see the
version-pin note below):

| Suite | Result |
|---|---|
| Existing unit tests, default features | **104 passed**, 0 failed |
| Existing unit tests, `--features video` | **113 passed**, 0 failed (real `ffmpeg`/`ffprobe`, not mocked) |
| New `tests/integration_test.rs`, default features | **15 passed**, 0 failed |
| New `tests/integration_test.rs`, `--features video` | **17 passed**, 0 failed |
| Doc-tests | **2 passed** |
| All 7 bench files | compile clean, **every one run to completion** with real timing output, no panics |

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

## `wasm` feature — not verified here

`src/wasm.rs` is gated on `target_arch = "wasm32"`, and this sandbox has
no `wasm32-unknown-unknown` target installed (no `rustup` to add it), so
I could not compile or test it. Its existing structure (and public
function signatures) weren't touched. If you want `wasm-bindgen-test`
coverage for it, that's the one piece of this deliverable that's
unverified.
