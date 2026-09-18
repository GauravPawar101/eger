//! `eger` — a high-level, parallel ASCII-art media pipeline built on top
//! of [`iascii`].
//!
//! - [`config`] — fluent [`Config`]/[`ConfigBuilder`] describing input,
//!   output, sizing, and parallelism.
//! - [`image`] — single-image and Rayon-parallel batch image → ASCII
//!   rendering. Always available.
//! - `video` *(requires the `video` feature)* — video → ASCII
//!   frame-sequence rendering, streamed through `ffmpeg` with chunked
//!   Rayon-parallel conversion, plus terminal playback.
//! - [`render`] — output targets: stdout, file, `String`, or `Vec<String>`,
//!   plus terminal-size detection ([`render::terminal_size`]) and
//!   cursor-addressed diffed playback ([`render::diff_frame`],
//!   [`render::play_terminal_diffed`]) as a lower-flicker alternative to
//!   clear-and-redraw.
//! - [`text`] — text → ANSI coloring/animation: built-in styles
//!   (`Typewriter`, `Rainbow`, `Wave`, `Blink`, `Marquee`, `Pulse`) plus
//!   custom animations driven by an external program or a Rust closure.
//!   Always available (no `video` feature needed, except for the async
//!   `play_text_async`/`stream_text_frames` variants).
//! - [`banner`] — multi-line "big text" banners: the same animation styles
//!   as [`text`], rendered as block letters instead of a single line.
//! - [`colorize`] — per-*cell* (as opposed to [`text`]/[`banner`]'s
//!   per-character) color animation for *any* block of plain ASCII text —
//!   a converted image, a decoded GIF/video frame, banner text, or
//!   anything else — including [`colorize::PixelAnimation::Custom`] for
//!   fully custom, mathematically-defined coloring functions.
//! - [`wasm`] *(requires the `wasm` feature, `target_arch = "wasm32"` only)*
//!   — a thin `wasm-bindgen` wrapper around the terminal-independent parts
//!   of the pipeline, for rendering ASCII art in a browser.
//! - [`error`] — unified [`EgerError`] / [`Result`].
//!
//! `video` also gains GIF playback ([`video::play_gif`]), and [`image`]
//! gains synchronous GIF decoding ([`image::gif_to_lines`]) available
//! without the `video` feature.
//!
//! # Feature flags
//!
//! - **`video`** *(off by default)* — pulls in `tokio` and enables the
//!   `video` module (which additionally requires `ffmpeg`/`ffprobe` on
//!   `PATH` at runtime), plus the async (`_async`) variants of the image
//!   API and `render::dispatch_async`/`render::play_terminal`, since
//!   they share the same Tokio runtime dependency.
//! - **`wasm`** *(off by default)* — pulls in `wasm-bindgen` and enables
//!   the [`wasm`] module. Only meaningful when targeting `wasm32`; mutually
//!   exclusive with `video` in practice, since `tokio`/`ffmpeg`/real
//!   terminals don't exist in a browser.
//!
//! # Example
//!
//! ```rust,no_run
//! use eger::prelude::*;
//!
//! # fn run() -> eger::Result<()> {
//! let config = Config::builder(MediaType::Image)
//!     .from_file("cat.png")
//!     .max_width(120)
//!     .color_depth(ColorDepth::TrueColor)
//!     .build()?;
//! let ascii = eger::image::image_to_string(std::path::Path::new("cat.png"), &config)?;
//! println!("{ascii}");
//! # Ok(())
//! # }
//! ```

pub mod banner;
pub mod colorize;
pub mod config;
pub mod error;
pub mod image;
pub mod render;
pub mod text;
#[cfg(feature = "video")]
pub mod video;
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
pub mod wasm;

pub use crate::banner::{banner_frames, banner_lines};
pub use crate::colorize::{
    colorize_frames, colorize_grid, colorize_grid_frames, colorize_joined_sequence, colorize_lines,
    colorize_sequence, ColorFn, PixelAnimation,
};
pub use crate::config::{Config, ConfigBuilder, MediaType};
pub use crate::error::{EgerError, Result};
pub use crate::image::{
    gif_to_lines, image_to_file, image_to_lines, image_to_string, render_batch_parallel,
    render_dynamic_image, render_image_file,
};
#[cfg(feature = "video")]
pub use crate::image::{render_batch_async, render_image_file_async};
#[cfg(feature = "video")]
pub use crate::render::play_terminal_diffed;
pub use crate::render::{
    detect_render_depth, terminal_size, DiffGranularity, RenderOutput, RenderTarget,
};
pub use crate::text::{
    play_text, text_frames, CustomAnimation, Rgb, TextAnimOptions, TextAnimation,
};
#[cfg(feature = "video")]
pub use crate::text::{play_text_async, stream_text_frames};
#[cfg(feature = "video")]
pub use crate::video::{
    play_gif, play_video, probe, render_colorized_video_frames, render_video_frames,
    stream_colorized_video_frames, stream_video_frames, video_to_file, video_to_lines,
    video_to_lines_plain, ColorMode, VideoInfo,
};

/// Everything needed to configure and drive `eger` in one `use`.
pub mod prelude {
    pub use crate::banner::{banner_frames, banner_lines};
    pub use crate::colorize::{
        colorize_frames, colorize_grid, colorize_grid_frames, colorize_joined_sequence,
        colorize_lines, colorize_sequence, ColorFn, PixelAnimation,
    };
    pub use crate::config::{Config, ConfigBuilder, InputSource, MediaType};
    pub use crate::error::{ConfigError, EgerError, Result};
    #[cfg(feature = "video")]
    pub use crate::render::play_terminal_diffed;
    pub use crate::render::{
        detect_render_depth, terminal_size, DiffGranularity, RenderOutput, RenderTarget,
    };
    pub use crate::text::{
        play_text, text_frames, CustomAnimation, Rgb, TextAnimOptions, TextAnimation,
    };
    #[cfg(feature = "video")]
    pub use crate::text::{play_text_async, stream_text_frames};
    #[cfg(feature = "video")]
    pub use crate::video::{
        play_gif, render_colorized_video_frames, stream_colorized_video_frames,
        video_to_lines_plain, ColorMode, VideoInfo,
    };

    pub use iascii::config::{LuminanceMethod, OutputSizing};
    pub use iascii::ramp::{Ramp, RampType};
    pub use iascii::render::ColorDepth;
}
