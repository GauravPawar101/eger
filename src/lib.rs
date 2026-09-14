//! `maramura` — a high-level, parallel ASCII-art media pipeline built on top
//! of [`iascii`].
//!
//! - [`config`] — fluent [`Config`]/[`ConfigBuilder`] describing input,
//!   output, sizing, and parallelism.
//! - [`image`] — single-image and Rayon-parallel batch image → ASCII
//!   rendering. Always available.
//! - `video` *(requires the `video` feature)* — video → ASCII
//!   frame-sequence rendering, streamed through `ffmpeg` with chunked
//!   Rayon-parallel conversion, plus terminal playback.
//! - [`render`] — output targets: stdout, file, `String`, or `Vec<String>`.
//! - [`error`] — unified [`MaramuraError`] / [`Result`].
//!
//! # Feature flags
//!
//! - **`video`** *(off by default)* — pulls in `tokio` and enables the
//!   `video` module (which additionally requires `ffmpeg`/`ffprobe` on
//!   `PATH` at runtime), plus the async (`_async`) variants of the image
//!   API and `render::dispatch_async`/`render::play_terminal`, since
//!   they share the same Tokio runtime dependency. Leave this off if you
//!   only need synchronous image → ASCII rendering — Rayon-based parallel
//!   batch processing (see [`image::render_batch_parallel`]) works out of
//!   the box either way and does **not** require this feature.
//!
//! # Example
//!
//! ```rust,no_run
//! use maramura::prelude::*;
//!
//! # fn run() -> maramura::Result<()> {
//! // Image -> String (no features required)
//! let config = Config::builder(MediaType::Image)
//!     .from_file("cat.png")
//!     .max_width(120)
//!     .color_depth(ColorDepth::TrueColor)
//!     .build()?;
//! let ascii = maramura::image::image_to_string(std::path::Path::new("cat.png"), &config)?;
//! println!("{ascii}");
//! # Ok(())
//! # }
//! ```
//!
//! With `features = ["video"]` enabled:
//!
//! ```rust,no_run,ignore
//! use std::sync::Arc;
//! use maramura::prelude::*;
//!
//! # async fn run() -> maramura::Result<()> {
//! let video_config = Arc::new(
//!     Config::builder(MediaType::Video)
//!         .from_file("clip.mp4")
//!         .max_width(100)
//!         .num_threads(8)
//!         .build()?,
//! );
//! maramura::video::play_video(std::path::Path::new("clip.mp4"), video_config).await?;
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod error;
pub mod image;
pub mod render;
#[cfg(feature = "video")]
pub mod video;

pub use crate::config::{Config, ConfigBuilder, MediaType};
pub use crate::error::{MaramuraError, Result};
pub use crate::image::{
    image_to_file, image_to_lines, image_to_string, render_batch_parallel, render_dynamic_image,
    render_image_file,
};
#[cfg(feature = "video")]
pub use crate::image::{render_batch_async, render_image_file_async};
pub use crate::render::{detect_render_depth, RenderOutput, RenderTarget};
#[cfg(feature = "video")]
pub use crate::video::{
    play_video, probe, render_video_frames, video_to_file, video_to_lines, VideoInfo,
};

/// Everything needed to configure and drive `maramura` in one `use`, including
/// the underlying `iascii` types (ramps, luminance formulas, sizing, color
/// depth) so consumers don't need to depend on `iascii` directly.
pub mod prelude {
    pub use crate::config::{Config, ConfigBuilder, InputSource, MediaType};
    pub use crate::error::{ConfigError, MaramuraError, Result};
    pub use crate::render::{detect_render_depth, RenderOutput, RenderTarget};
    #[cfg(feature = "video")]
    pub use crate::video::VideoInfo;

    pub use iascii::config::{LuminanceMethod, OutputSizing};
    pub use iascii::ramp::{Ramp, RampType};
    pub use iascii::render::ColorDepth;
}
