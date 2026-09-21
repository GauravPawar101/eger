# eger

[![Crates.io](https://img.shields.io/crates/v/eger.svg)](https://crates.io/crates/eger)
[![Documentation](https://docs.rs/eger/badge.svg)](https://docs.rs/eger)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A high-level, parallel ASCII-art media pipeline built on top of
[`iascii`](https://crates.io/crates/iascii). Turn images, video, GIFs, and
plain text into colored (or plain) ANSI terminal art — with Floyd–Steinberg
/ Atkinson / Bayer dithering, animated text and banners, per-region
segment styling, non-Latin character ramps, and procedurally generated
optical illusions, all sharing one rendering pipeline.

```toml
[dependencies]
eger = "0.1"
```

```rust
use eger::prelude::*;

let config = Config::builder(MediaType::Image)
    .from_file("cat.png")
    .max_width(120)
    .color_depth(ColorDepth::TrueColor)
    .build()?;
let ascii = eger::image::image_to_string(std::path::Path::new("cat.png"), &config)?;
println!("{ascii}");
```

## What's inside

| Module | What it does |
|---|---|
| [`config`](src/config.rs) | Fluent `Config`/`ConfigBuilder` for input, output, sizing, and parallelism |
| [`image`](src/image.rs) | Single-image and Rayon-parallel batch image → ASCII rendering |
| `video` *(feature `video`)* | Video → ASCII frame-sequence rendering, streamed through `ffmpeg` |
| [`render`](src/render.rs) | Output targets (stdout / file / `String` / `Vec<String>`), terminal-size detection, cursor-addressed diffed playback |
| [`text`](src/text.rs) | Text → ANSI coloring/animation: `Typewriter`, `Rainbow`, `Wave`, `Blink`, `Marquee`, `Pulse`, plus custom program/closure animations |
| [`banner`](src/banner.rs) | Multi-line "big text" block-letter banners, same animation styles as `text` |
| [`colorize`](src/colorize.rs) | Per-*cell* color animation for any block of plain ASCII text (converted image, GIF/video frame, banner, ...), including fully custom coloring functions |
| [`segment`](src/segment.rs) | Partitions a converted frame into numbered/named structures so one region can be bolded, colored, or animated independently |
| [`dither`](src/dither.rs) | Floyd–Steinberg / Atkinson error diffusion and Bayer ordered dithering, for reducing banding on coarse ANSI palettes |
| [`script`](src/script.rs) | Ready-made non-Latin character ramps — Cyrillic, Greek, CJK, Devanagari, Hebrew, Arabic, Braille, block/shade |
| [`illusions`](src/illusions.rs) | Procedurally generated optical illusions (café wall, Hermann grid, twisted cord, animated rotating rings) rendered through the same pipeline |
| `wasm` *(feature `wasm`, `wasm32` only)* | Thin `wasm-bindgen` wrapper for rendering ASCII art in a browser |
| [`error`](src/error.rs) | Unified `EgerError` / `Result` |

`video` also enables GIF playback (`video::play_gif`); GIF decoding
(`image::gif_to_lines`) is available without the `video` feature.

## Feature flags

- **`video`** *(off by default)* — pulls in `tokio`, enables the `video`
  module (requires `ffmpeg`/`ffprobe` on `PATH` at runtime), plus the
  async (`_async`) variants of the image API.
- **`wasm`** *(off by default)* — pulls in `wasm-bindgen`, enables the
  `wasm` module. Only meaningful when targeting `wasm32`.

## Testing & benchmarks

See [`TESTS_AND_BENCHMARKS.md`](TESTS_AND_BENCHMARKS.md) for the full
integration-test and Criterion-benchmark suite, and what's verified where.

```sh
cargo test                    # unit + integration tests
cargo test --features video   # + video pipeline tests (needs ffmpeg)
cargo bench                   # full benchmark suite
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
