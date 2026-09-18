# Benchmarks

Four [Criterion](https://docs.rs/criterion) benchmark binaries, one per area
of the pipeline:

| Benchmark            | Measures                                                                 | Feature needed |
|-----------------------|---------------------------------------------------------------------------|:--------------:|
| `image_conversion`    | `render_dynamic_image` across source size, output width, color depth.   | none            |
| `batch_rendering`     | `render_batch_parallel` across file count and `num_threads`.            | none            |
| `animation`           | `text_frames`/`banner_frames`/`banner_lines` across animation styles.   | none            |
| `video_streaming`     | `render_video_frames` (via `stream_video_frames`) across frame count.   | `video` + `ffmpeg`/`ffprobe` on `PATH` |

## Running

```bash
cargo bench                                  # image_conversion, batch_rendering, animation
cargo bench --bench image_conversion         # just one binary
cargo bench --features video                 # also runs video_streaming
cargo bench --no-default-features            # sequential-only image conversion (no `parallel`)
```

`video_streaming` checks for `ffmpeg`/`ffprobe` on `PATH` at the start of
its `main` and prints a message and skips its group (without failing the
build or the run) if they're missing — the same approach `video.rs`'s own
`#[tokio::test]`s use.

## Reading results

Criterion writes HTML reports to `target/criterion/<group>/<bench>/report/index.html`,
including violin plots and change-over-time tracking if you run the same
benchmark repeatedly (e.g. before/after a change to compare).

A few things worth specifically comparing:

- **`convert_by_source_size` vs `convert_by_output_width`** — source
  resolution and requested output width scale conversion cost differently
  (decode/sample cost vs. glyph-emission cost); useful for picking a
  sensible default `max_width`/`auto_width` fallback for your use case.
- **`--features video` vs `--no-default-features --features video`** —
  shows the effect of `iascii`'s row-level `parallel` conversion path on
  top of video's own always-on across-frame Rayon parallelism.
- **`render_batch_parallel_by_thread_count`** — diminishing (or negative)
  returns from more threads than you have cores, useful for sanity-checking
  a `num_threads` default on a given machine.
