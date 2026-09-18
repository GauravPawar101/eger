# eger

**A parallel, async image/video → ASCII-art pipeline for Rust**, built on top of
[`iascii`](https://crates.io/crates/iascii). `eger` turns images, animated GIFs,
and video into colored (or plain) ASCII/ANSI text, gives you the building
blocks to *animate* text and render big block-letter banners, and lets you
drive the color of **any** of that ASCII art — a single image, a banner, a
GIF, or a video — from your own math instead of (or on top of) `iascii`'s
own pixel coloring.

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
  *across* files/frames in batch, video, and colorizing pipelines (always
  on), and optionally *within* a single large conversion (row-level, via the
  `parallel` feature) — see [Parallelism](#parallelism) below.
- **A real animation layer, not just a converter.** `text` and `banner` give
  you `Typewriter`, `Rainbow`, `Wave`, `Blink`, `Marquee`, `Pulse`, and fully
  custom (subprocess- or closure-driven) animations; `colorize` gives you
  the same idea at the *pixel* level, for art whose own text content isn't
  animated — a converted image, a banner, a GIF, or a video.
- **Color driven by math, not just presets.** `colorize::PixelAnimation`
  ships useful built-ins (`Rainbow`, `Wave`, `Pulse`, `Blink`, `Plasma`,
  `Ripple`), but `PixelAnimation::Custom` hands you a plain
  `(x, y, frame, ch) -> Rgb` function — write whatever math you want
  (gradients, noise, distance fields, cellular automata) and it colors
  correctly and efficiently, with no support needed from the crate beyond
  the coordinates it's called with.
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

`colorize` needs no feature flag — it's always available, built on the
`rayon` dependency the crate already pulls in unconditionally for its
always-on across-file/frame parallelism.

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
  `text`'s animation styles for HUD elements and transitions, `colorize` for
  a rainbow/plasma/ripple title screen or a custom color effect, and
  `render::terminal_size`/`auto_width` to adapt to whatever terminal the
  player is running.
- **Video and GIF playback in the terminal** — `video::play_video` and
  `video::play_gif` stream frames directly to stdout, timed to the source
  frame rate/delay, with bounded memory via `stream_video_frames` (or its
  colorized counterpart, `stream_colorized_video_frames`) rather than
  decoding the whole file up front.
- **Batch ASCII art pipelines** — `render_batch_parallel`/`render_batch_async`
  convert a whole directory of images in parallel and write results out
  per-file.
- **Stylized, non-photorealistic ASCII art** — take any converted image,
  banner, GIF, or video and recolor it with `colorize::PixelAnimation`
  instead of (or as well as) its actual pixel colors: a plasma-shaded video,
  a rainbow-swept banner, a custom radial gradient over a logo.
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

### Pixel-level color animation (`colorize`)

`text`/`banner` color one *character* (or one banner glyph) at a time.
`colorize` colors one *grid cell* at a time, so it works the same way over
a converted image, a banner, a GIF frame, or a video frame — anything
that's ultimately rows of characters.

```rust
use eger::prelude::*;

// Animate a static image's color over 30 frames instead of re-converting it:
let img = image::open("logo.png")?.to_rgb8();
let grid = eger::image::convert(img.width(), img.height(), img.as_raw(), &config)?;
let frames = eger::colorize_grid_frames(
    &grid,
    &PixelAnimation::Plasma { scale: 0.15, speed: 0.2 },
    30,
);

// Recolor a banner with a diagonal rainbow sweep:
let lines = eger::banner_lines("GAME OVER");
let frame = eger::colorize_lines(&lines, &PixelAnimation::Rainbow { speed: 6.0 }, 12);

// Fully custom math — a radial gradient, ignoring frame/char entirely:
let art = eger::colorize_lines(
    &lines,
    &PixelAnimation::custom(|x, y, _frame, _ch| {
        let d = ((x as f32).powi(2) + (y as f32).powi(2)).sqrt();
        Rgb::new(255, (d as u8).wrapping_mul(3), 128)
    }),
    0,
);
```

For content that's *already* animating frame to frame (a GIF or a video),
recolor the existing sequence in place — each frame's position in the
sequence drives the color animation's time input, so the two stay in sync:

```rust
// GIF: gif_to_lines already gives you plain (uncolored) per-frame lines.
let gif_frames = eger::gif_to_lines(std::path::Path::new("dance.gif"), &config)?;
let lines_only: Vec<Vec<String>> = gif_frames.into_iter().map(|(lines, _)| lines).collect();
let colored = eger::colorize_sequence(&lines_only, &PixelAnimation::Ripple {
    base_color: Rgb::new(0, 200, 255),
    speed: 0.3,
});
```

```rust
// Video (requires the `video` feature): stream, don't collect, for long files.
use eger::prelude::*;
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel(4);
let animation = PixelAnimation::Plasma { scale: 0.12, speed: 0.25 };
tokio::spawn(eger::stream_colorized_video_frames(
    "clip.mp4".into(),
    config.clone(),
    animation,
    tx,
));

while let Some(chunk) = rx.recv().await {
    for frame in chunk? {
        println!("{frame}");
    }
}
```

`render_colorized_video_frames` is the same idea without the channel, for
short clips where holding every frame in memory is fine (same tradeoff as
`render_video_frames` vs. `stream_video_frames`).

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

- **Across files/frames** (`render_batch_parallel`, `video::stream_video_frames`,
  and `colorize`'s multi-frame functions — `colorize_frames`,
  `colorize_grid_frames`, `colorize_sequence`, `colorize_joined_sequence`,
  and `video::stream_colorized_video_frames`'s per-chunk recoloring) —
  always on, plain `rayon::par_iter`/`into_par_iter` over independent units
  of work this crate already owns. No feature flag needed.
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

`colorize`'s frame-level parallelism uses Rayon's default global pool
(`rayon::prelude::*`'s `par_iter`/`into_par_iter`) rather than a `Config`'s
cached pool, since coloring doesn't need `Config` at all — it operates on
plain text, not on anything `iascii` produces from a `Config`.

`num_threads(0)` (or leaving it unset when the machine can't report
`available_parallelism`) defers to Rayon's own default sizing.

## Coloring performance (`colorize`)

- **Runs, not per-cell escapes.** Adjacent same-color cells — including
  across a row boundary, since ANSI color state isn't reset by a bare `\n`
  on any real terminal — are merged into a single `escape .. reset` pair.
  A uniform-color frame (`Pulse`, `Blink`, a `Custom` closure that ignores
  position) costs one escape pair for the whole frame, not one per
  character.
- **Escape strings are cached per frame.** Each distinct `Rgb` value's
  ANSI escape string is formatted once and reused for every later
  occurrence in that frame, so animations with many repeating-but-not-
  adjacent colors (`Rainbow`, `Plasma`) don't reformat the same string
  over and over.
- **No forced cloning.** `colorize_lines`/`colorize_frames` are generic
  over `S: AsRef<str>`, so callers already holding borrowed `&str` rows
  (like `colorize_joined_sequence`, splitting one joined video frame on
  `\n`) pass them straight through instead of allocating owned `String`s
  first.
- **Dimensions computed once per call**, not once per generated frame —
  `colorize_frames`/`colorize_grid_frames` measure `lines`' width/height a
  single time and reuse it across every frame, since the source art's
  shape doesn't change frame to frame.
- **Bounded memory for video.** `stream_colorized_video_frames` colors
  video frames as they're decoded, chunk by chunk, instead of requiring
  the whole video's frames in memory first — the same bounded-memory
  design `stream_video_frames` uses for plain video rendering.

## Resource cleanup

- The Rayon thread pool cached on `Config` is torn down normally when the
  `Config` (and every `Arc` clone of it) is dropped — no manual shutdown
  needed.
- `video`'s `ffmpeg` child process is spawned with `kill_on_drop(true)` and
  is killed explicitly the moment its output channel's receiver is dropped,
  rather than being left to write into a pipe nobody is draining — which
  would otherwise block `ffmpeg` indefinitely and hang the corresponding
  `child.wait()`. `stream_colorized_video_frames` inherits this: if its own
  output channel's receiver is dropped, it stops reading from the
  underlying plain-frame stream, which propagates the same way.

## Benchmarks

Criterion benchmarks live in `benches/` and cover image conversion at
several sizes/color depths, banner/text/colorize frame generation, and
(with `--features video`) the video frame-conversion hot path:

```bash
cargo bench                         # default features
cargo bench --features video        # include video benches (needs ffmpeg)
```

See [`benches/README.md`](benches/README.md) for what each group measures.

## MSRV

Rust **1.80**, as declared in `Cargo.toml`'s `rust-version`.

## License

Dual-licensed under MIT or Apache-2.0, at your option.
