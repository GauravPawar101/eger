# eger

A high-level, parallel ASCII-art media pipeline built on top of [`iascii`].
`eger` turns images and (optionally) videos into ASCII/ANSI art, with a
fluent configuration API, Rayon-parallel batch processing, and — behind the
`video` feature — an async, `ffmpeg`-backed video pipeline with terminal
playback.

## At a glance

| Module         | Responsibility                                                                 |
| -------------- | ------------------------------------------------------------------------------- |
| [`config`]     | Fluent `Config`/`ConfigBuilder`: input source, sizing, color, parallelism.      |
| [`image`]      | Single-image and Rayon-parallel batch image → ASCII rendering. Always on.      |
| [`video`]      | `ffmpeg`-streamed video → ASCII frame sequences, plus terminal playback. `video` feature only. |
| [`render`]     | Output targets (stdout / file / `String` / `Vec<String>`) and color-depth detection. |
| [`error`]      | Unified `ConfigError` / `egerError` / `Result`.                              |

## Feature flags

- **default** — synchronous image rendering only. `image::render_batch_parallel`
  (Rayon-based parallel batch processing) works out of the box and does
  **not** require the `video` feature.
- **`video`** *(off by default)* — pulls in `tokio` and enables:
  - the `video` module itself (requires `ffmpeg`/`ffprobe` on `PATH` at
    runtime — there's no bundled/vendored ffmpeg),
  - the async (`_async`) variants of the image API
    (`render_image_file_async`, `render_batch_async`),
  - `render::dispatch_async` and `render::play_terminal`.

  These all share the same Tokio runtime dependency, which is why they're
  gated together rather than each having their own flag.

## Quick start

```toml
[dependencies]
eger = "0.1"
```

```rust
use eger::prelude::*;

fn run() -> eger::Result<()> {
    let config = Config::builder(MediaType::Image)
        .from_file("cat.png")
        .max_width(120)
        .color_depth(ColorDepth::TrueColor)
        .build()?;

    let ascii = eger::image_to_string(std::path::Path::new("cat.png"), &config)?;
    println!("{ascii}");
    Ok(())
}
```

With `features = ["video"]`:

```rust
use std::sync::Arc;
use eger::prelude::*;

async fn run() -> eger::Result<()> {
    let config = Arc::new(
        Config::builder(MediaType::Video)
            .from_file("clip.mp4")
            .max_width(100)
            .num_threads(8)
            .build()?,
    );
    eger::play_video(std::path::Path::new("clip.mp4"), config).await
}
```

## Configuration

`Config` is built once via `ConfigBuilder` and is cheap to share (wrap it in
`Arc<Config>` for the video/async APIs, which already expect that). All
setters are infallible and chainable; validation happens once, centrally, in
[`ConfigBuilder::build`]:

- the source path exists (`from_file`) or is a real directory (`from_dir`),
- the batch regex pattern compiles,
- the underlying `iascii::config::ConfigBuilder` itself validates cleanly.

Calling `from_file` and `from_dir` more than once (in either order) is not
an error — whichever was called last wins, since each just overwrites the
builder's internal path/pattern/`is_dir` state.

### Input sources

```rust
// Single file
Config::builder(MediaType::Image).from_file("frame.png");

// Every file in a directory whose name matches a regex
Config::builder(MediaType::Image).from_dir("frames/", r"\.png$");
```

`InputSource::Directory` resolution (`Config::files`) is:
- **non-recursive** — subdirectories are skipped even if their *name*
  matches the pattern (only `Path::is_file()` entries are considered),
- **case-sensitive** — pattern matching goes through `regex`, with no
  implicit `(?i)` flag,
- **sorted** — results are `Vec::sort`-ed for deterministic ordering
  regardless of `read_dir`'s OS-dependent iteration order, which matters for
  reproducible parallel batch runs,
- an **error**, not an empty `Vec`, when nothing matches
  (`ConfigError::NoMatchingFiles`).

### Output

`ConfigBuilder::output` sets a *default* output path (`Config.output`).
Every `*_to_file` convenience function (`image_to_file`, `video_to_file`)
prefers an explicitly-passed path over this default, and only falls back to
`config.output` when `None` is passed — so a single `Config` can be reused
across calls that override the destination per-file and calls that rely on
the configured default.

## Rendering pipelines

### Images

- `render_image_file` / `render_dynamic_image` — single image, sync.
- `render_image_file_async` — same, off-loaded to Tokio's blocking pool
  (`video` feature).
- `render_batch_parallel` — every file from `Config::files()`, converted in
  parallel on a dedicated Rayon pool sized by `config.num_threads`. A
  per-file failure (e.g. a corrupt image) surfaces as an `Err` entry for
  that file *without* aborting the rest of the batch — the `Vec` result
  always has one entry per input file, each independently `Ok`/`Err`.
- `render_batch_async` — one Tokio task per file so I/O overlaps, while the
  CPU-bound decode+convert step for each still runs on the blocking pool
  (`video` feature).
- Convenience wrappers: `image_to_string`, `image_to_lines`, `image_to_file`.

### Video (`video` feature)

Frames are streamed as raw RGB24 straight out of an `ffmpeg` pipe (no
temporary frame files) and converted in bounded chunks (`chunk_size =
num_threads * 4`, minimum 1) on a single, reused Rayon pool — this caps
memory to roughly one chunk of frames at a time while still overlapping the
*next* chunk's pipe read with the *current* chunk's CPU-bound conversion.

- `probe` — `ffprobe` for resolution/fps/frame count.
- `render_video_frames` — the full frame pipeline, with an optional
  `depth_override` to render the same video at a different `ColorDepth`
  without rebuilding `Config`.
- `video_to_lines` / `video_to_file` — convenience wrappers.
  `video_to_file` joins frames with `"\n\x1E\n"` (ASCII record separator)
  so they can be split back out losslessly.
- `play_video` — renders and immediately plays back in-terminal at the
  source's native fps, without touching disk.

Every video entry point returns `egerError::FfmpegNotFound` if `ffmpeg`
isn't on `PATH`, or `egerError::Ffmpeg(status, stderr)` if it runs but
exits non-zero (e.g. probing a missing/corrupt file).

## Color depth

`render::detect_render_depth()` picks a `ColorDepth` appropriate for the
current terminal, honoring:

- [`NO_COLOR`](https://no-color.org) (any value) → plain text,
- `TERM=dumb` → plain text,
- stdout not actually being a terminal (e.g. piped to a file) → plain text,
- otherwise, the richest depth the terminal advertises:
  `COLORTERM=truecolor`/`24bit` → `TrueColor`; `TERM` containing
  `256color` → `Ansi256`; otherwise `Ansi16`.

This is purely an opt-in convenience — nothing in the crate calls it
automatically. `Config::color_depth` (defaulting to `ColorDepth::TrueColor`)
is what every rendering entry point actually uses unless a caller passes an
explicit override (e.g. `render_video_frames`'s `depth_override`).

## Testing

```sh
# Synchronous image pipeline only
cargo test

# Full pipeline, including the video/ffmpeg-backed tests
cargo test --features video
```

Video tests generate their own tiny synthetic clips at runtime via
`ffmpeg -f lavfi -i testsrc=...` (no checked-in fixture files) and **skip
themselves at runtime** (printing a message rather than failing) if
`ffmpeg`/`ffprobe` aren't found on `PATH`. `image_pipeline.rs` and
`video_pipeline.rs` are black-box integration tests that only exercise the
crate's public API (`eger::prelude::*`), the way an external consumer
would.

## Known sharp edges

- **`ConfigError::MediaTypeNotFound` is currently unreachable.**
  `Config::builder(media_type)` always takes a `MediaType` up front, so
  nothing in `ConfigBuilder::build` can ever produce this variant today. If
  you're relying on `matches!(err, ConfigError::MediaTypeNotFound)`
  somewhere, it will never trigger — treat it as reserved for a future API
  change (e.g. a `Default`-derived builder) rather than a case you need to
  handle now.
- **`num_threads` isn't validated.** `ConfigBuilder::num_threads` stores
  whatever `usize` it's given, including `0`, without special-casing it.
  Downstream, `rayon::ThreadPoolBuilder::num_threads(0)` falls back to
  Rayon's own default parallelism rather than erroring, so `0` is *usable*
  but is not the same as "use the default" being explicit — omitting the
  call entirely (which defaults to `std::thread::available_parallelism()`)
  is the clearer way to express that.
- **`color_depth` is always applied, never optional, on the single-image
  convenience API.** `render_dynamic_image` calls
  `render::dispatch(target, &grid, Some(config.color_depth))` — there is no
  code path through `image_to_string`/`image_to_lines`/`image_to_file` that
  produces plain, escape-free output. To get plain text, either post-process
  with a plain-grid render function directly, or drive `render::dispatch`
  with `None` at a lower level.

[`iascii`]: https://docs.rs/iascii
[`ConfigBuilder::build`]: src/config.rs
