# eger

**A parallel, async image/video → ASCII-art pipeline for Rust**, built on top of
[`iascii`](https://crates.io/crates/iascii). `eger` turns images, animated GIFs,
and video into colored (or plain) ASCII/ANSI text, and gives you the building
blocks to *animate* text and render big block-letter banners — everything you
need to build a TUI status screen, an ASCII game, a terminal video player, or
a WASM-powered ASCII renderer for the browser.

```rust
use eger::prelude::*;

let config = Config::builder(MediaType::Image)
    .from_file("cat.png")
    .max_width(120)
    .color_depth(ColorDepth::TrueColor)
    .build()?;

let ascii = eger::image_to_string(std::path::Path::new("cat.png"), &config)?;
println!("{ascii}");
```

## Why `eger`

- **One conversion path, several entry points.** Single image, `Vec<String>`,
  a file, a batch of files, an animated GIF, or a live video stream — they
  all funnel through the same `iascii`-backed conversion, so sizing,
  luminance, ramp, and color settings behave identically everywhere.
- **Two kinds of parallelism, used deliberately.** Rayon parallelizes
  *across* files/frames in batch and video pipelines (always on), and
  optionally *within* a single large conversion (row-level, via the
  `parallel` feature) — see [Parallelism](#parallelism) below.
- **A real animation layer, not just a converter.** `text` and `banner` give
  you `Typewriter`, `Rainbow`, `Wave`, `Blink`, `Marquee`, `Pulse`, and
  fully custom (subprocess- or closure-driven) animations, usable for
  loading indicators, TUI headers, game title screens, or animated banners.
- **Terminal-friendly by construction.** Auto-detected color depth and
  width, cursor-addressed diffed repainting (`DiffGranularity`) for
  low-flicker updates, and a real terminal-size query — the pieces a
  TUI or terminal game needs, without pulling in a full TUI framework.
- **Runs in the browser too.** The `wasm` feature exposes a
  `wasm-bindgen` surface for image/GIF → ASCII conversion and
  frame-at-a-time text animation, driven by your own
  `requestAnimationFrame`/`setInterval` loop.

## Feature flags

| Feature    | Default | Pulls in                     | Enables                                                                 |
|------------|:-------:|-------------------------------|--------------------------------------------------------------------------|
| `parallel` | ✅ on   | `iascii/parallel`             | Row-level parallel conversion for one large image/frame (see below).    |
| `video`    | off     | `tokio`                       | The `video` module, async image APIs, `render::play_terminal[_diffed]`, `render::dispatch_async`. Requires `ffmpeg`/`ffprobe` on `PATH` at runtime. |
| `wasm`     | off     | `wasm-bindgen`                | The `wasm` module — browser-facing image/GIF/text-animation bindings. `target_arch = "wasm32"` only; not meant to be combined with `video`. |

```toml
# Default (parallel image conversion, no video/wasm):
eger = "1"

# With video streaming/playback:
eger = { version = "1", features = ["video"] }

# For a WASM build:
eger = { version = "1", default-features = false, features = ["wasm"] }
```

## What you can build with it

- **Terminal UIs and dashboards** — render a static image or a live-updating
  ASCII visualization as part of a larger TUI; `render::diff_frame` and
  `DiffGranularity` give you flicker-free partial repaints instead of
  clear-and-redraw.
- **Terminal ASCII games** — `banner` for title screens and score displays,
  `text`'s animation styles for HUD elements and transitions, and
  `render::terminal_size`/`auto_width` to adapt to whatever terminal the
  player is running.
- **Video and GIF playback in the terminal** — `video::play_video` and
  `video::play_gif` stream frames directly to stdout, timed to the source
  frame rate/delay, with bounded memory via `stream_video_frames` rather
  than decoding the whole file up front.
- **Batch ASCII art pipelines** — `render_batch_parallel`/`render_batch_async`
  convert a whole directory of images in parallel and write results out
  per-file.
- **Browser ASCII art** — compile with `--features wasm --no-default-features`
  and call the exported functions directly from JS.

## Usage

### Rendering a single image

```rust
use eger::prelude::*;
use std::path::Path;

let config = Config::builder(MediaType::Image)
    .from_file("photo.jpg")
    .auto_width(100)               // fits the current terminal, falls back to 100
    .color_depth(ColorDepth::TrueColor)
    .build()?;

// To a String, to Vec<String>, or straight to stdout/a file:
let s = eger::image_to_string(Path::new("photo.jpg"), &config)?;
let lines = eger::image_to_lines(Path::new("photo.jpg"), &config)?;
eger::image::image_to_file(Path::new("photo.jpg"), &config, Some(Path::new("out.txt")))?;
```

### Batch-converting a directory (Rayon, always on)

```rust
use eger::prelude::*;
use eger::render::RenderTarget;

let config = Config::builder(MediaType::Image)
    .from_dir("frames/", r"\.png$")
    .num_threads(8)
    .build()?;

let results = eger::render_batch_parallel(&config, |path| {
    RenderTarget::File(path.with_extension("txt"))
})?;

for (path, outcome) in results {
    if let Err(e) = outcome {
        eprintln!("{path:?} failed: {e}");
    }
}
```

### Animated GIFs

```rust
let frames = eger::gif_to_lines(std::path::Path::new("dance.gif"), &config)?;
for (lines, delay) in frames {
    // `lines.len()` rows per frame, `delay` is that frame's display time
}
```

### Video (requires the `video` feature and `ffmpeg`/`ffprobe` on `PATH`)

```rust
use eger::prelude::*;
use std::sync::Arc;

let config = Arc::new(
    Config::builder(MediaType::Video)
        .from_file("clip.mp4")
        .max_width(120)
        .build()?,
);

eger::play_video(std::path::Path::new("clip.mp4"), config).await?;
```

For long or high-resolution video, prefer `stream_video_frames` (bounded
channel, chunked Rayon conversion) over `render_video_frames`, which holds
every frame in memory.

### Text animation

```rust
use eger::prelude::*;

let opts = TextAnimOptions {
    frames: 60,
    fps: 24.0,
    base_color: Rgb::new(0, 200, 255),
    ..Default::default()
};

eger::play_text("LOADING...", &TextAnimation::Rainbow, &opts)?;
```

### Block-letter banners

```rust
use eger::prelude::*;

// Plain, uncolored 7-row block text:
for line in eger::banner_lines("HELLO") {
    println!("{line}");
}

// Animated, colored banner frames:
let opts = TextAnimOptions { frames: 30, ..Default::default() };
let frames = eger::banner_frames("GAME OVER", &TextAnimation::Pulse, &opts)?;
```

### Low-flicker terminal repainting

```rust
use eger::render::{diff_frame, DiffGranularity};

let patch = diff_frame(&previous_frame, &next_frame, DiffGranularity::Line);
print!("{patch}"); // only the changed rows are redrawn
```

### WASM (requires the `wasm` feature)

```rust
// compiled with: wasm-pack build --no-default-features --features wasm
use eger::wasm::*;

let ascii = image_bytes_to_string(&png_bytes, 100, true)?;
```

```js
import init, { image_bytes_to_string, text_frame_at, TextAnimationKind } from "./pkg/eger.js";
await init();
const art = image_bytes_to_string(bytes, 100, true);
document.querySelector("pre").textContent = art;
```

## Parallelism

`eger` deliberately separates *where* parallel work happens:

- **Across files/frames** (`render_batch_parallel`,
  `video::stream_video_frames`) — always on, plain `rayon::par_iter` over a
  `Vec` this crate already owns. No feature flag needed.
- **Within one conversion** (`image::convert`, used by every single-image
  and per-frame conversion call site) — gated behind the `parallel` feature
  (on by default), forwarded to `iascii`'s `convert_with_pool`, splitting
  one image/frame's *rows* across the pool. `iascii`'s own
  `parallel_threshold` decides per-call whether that's worth it, so a small
  thumbnail transparently stays sequential.

Both levels share a single Rayon `ThreadPool` per `Config` (built lazily on
first use via `Config::thread_pool`, cached behind a `OnceLock`), so a batch
job or a video stream doesn't spin up a new pool — and a fresh set of OS
threads — per file or per frame. Nesting (a batch item large enough to also
trigger the inner row-level split) is a supported, non-deadlocking Rayon
pattern, and is covered by regression tests in `image.rs` and `video.rs`.

`num_threads(0)` (or leaving it unset when the machine can't report
`available_parallelism`) defers to Rayon's own default sizing.

## Resource cleanup

- The Rayon thread pool cached on `Config` is torn down normally when the
  `Config` (and every `Arc` clone of it) is dropped — no manual shutdown
  needed.
- `video`'s `ffmpeg` child process is spawned with `kill_on_drop(true)` and
  is killed explicitly the moment its output channel's receiver is dropped,
  rather than being left to write into a pipe nobody is draining — which
  would otherwise block `ffmpeg` indefinitely and hang the corresponding
  `child.wait()`.

## Benchmarks

Criterion benchmarks live in `benches/` and cover image conversion at
several sizes/color depths, banner/text frame generation, and (with
`--features video`) the video frame-conversion hot path:

```bash
cargo bench                         # default features
cargo bench --features video        # include video benches (needs ffmpeg)
```

See [`benches/README.md`](benches/README.md) for what each group measures.

## MSRV

Rust **1.80**, as declared in `Cargo.toml`'s `rust-version`.

## License

Dual-licensed under MIT or Apache-2.0, at your option.
