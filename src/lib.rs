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
//! - [`render`] — output targets: stdout, file, `String`, or `Vec<String>`.
//! - [`error`] — unified [`EgerError`] / [`Result`].
//!
//! # Feature flags
//!
//! - **`video`** *(off by default)* — pulls in `tokio` and enables the
//!   `video` module (which additionally requires `ffmpeg`/`ffprobe` on
//!   `PATH` at runtime), plus the async (`_async`) variants of the image
//!   API and `render::dispatch_async`/`render::play_terminal`, since
//!   they share the same Tokio runtime dependency.
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

pub mod config;
pub mod error;
pub mod image;
pub mod render;
#[cfg(feature = "video")]
pub mod video;

pub use crate::config::{Config, ConfigBuilder, MediaType};
pub use crate::error::{EgerError, Result};
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

/// Everything needed to configure and drive `eger` in one `use`.
pub mod prelude {
    pub use crate::config::{Config, ConfigBuilder, InputSource, MediaType};
    pub use crate::error::{ConfigError, EgerError, Result};
    pub use crate::render::{detect_render_depth, RenderOutput, RenderTarget};
    #[cfg(feature = "video")]
    pub use crate::video::VideoInfo;

    pub use iascii::config::{LuminanceMethod, OutputSizing};
    pub use iascii::ramp::{Ramp, RampType};
    pub use iascii::render::ColorDepth;
}
