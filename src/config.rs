//! Fluent configuration for `maramura`'s image/video → ASCII pipelines.
//!
//! [`ConfigBuilder`] setters are infallible and chainable; every check (path
//! existence, regex compilation, `iascii` settings) happens centrally in
//! [`ConfigBuilder::build`], mirroring how `iascii::config::ConfigBuilder`
//! itself validates.

use crate::error::ConfigError;
use iascii::config::{
    Config as AsciiConfig, ConfigBuilder as AsciiConfigBuilder, LuminanceMethod, OutputSizing,
};
use iascii::ramp::RampType;
use iascii::render::ColorDepth;
use regex::Regex;
use std::path::PathBuf;

/// Whether a [`Config`] describes an image or a video pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
}

/// Where input files come from: a single file, or every file in a directory
/// whose name matches a regular expression.
#[derive(Debug, Clone)]
pub enum InputSource {
    File(PathBuf),
    Directory { dir: PathBuf, pattern: Regex },
}

/// A fully validated, immutable configuration produced by [`ConfigBuilder::build`].
///
/// Cheap to share across threads/tasks behind an `Arc<Config>` — build it
/// once and reuse it for every file in a batch or every frame of a video.
#[derive(Debug)]
pub struct Config {
    pub media_type: MediaType,
    pub source: InputSource,
    pub output: Option<PathBuf>,
    pub num_threads: usize,
    pub color_depth: ColorDepth,
    pub ascii: AsciiConfig,
}

impl Config {
    /// Starts building a new configuration for the given media type.
    pub fn builder(media_type: MediaType) -> ConfigBuilder {
        ConfigBuilder::new(media_type)
    }

    /// Resolves [`InputSource`] into the concrete list of files to process,
    /// sorted for deterministic ordering across parallel batch runs.
    pub fn files(&self) -> Result<Vec<PathBuf>, ConfigError> {
        match &self.source {
            InputSource::File(path) => Ok(vec![path.clone()]),
            InputSource::Directory { dir, pattern } => {
                let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
                    .map_err(|source| ConfigError::Io {
                        path: dir.clone(),
                        source,
                    })?
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| pattern.is_match(name))
                    })
                    .collect();

                if matches.is_empty() {
                    return Err(ConfigError::NoMatchingFiles {
                        dir: dir.clone(),
                        pattern: pattern.as_str().to_owned(),
                    });
                }

                matches.sort();
                Ok(matches)
            }
        }
    }
}

/// Builder for [`Config`].
pub struct ConfigBuilder {
    media_type: MediaType,
    path: Option<PathBuf>,
    pattern: Option<String>,
    is_dir: bool,
    output: Option<PathBuf>,
    num_threads: Option<usize>,
    color_depth: ColorDepth,
    ascii: AsciiConfigBuilder,
}

impl ConfigBuilder {
    pub fn new(media_type: MediaType) -> Self {
        Self {
            media_type,
            path: None,
            pattern: None,
            is_dir: false,
            output: None,
            num_threads: None,
            color_depth: ColorDepth::TrueColor,
            ascii: AsciiConfigBuilder::new(),
        }
    }

    /// Use a single file as input.
    pub fn from_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self.is_dir = false;
        self
    }

    /// Use every file in `dir` whose name matches `pattern` as input.
    pub fn from_dir(mut self, dir: impl Into<PathBuf>, pattern: impl Into<String>) -> Self {
        self.path = Some(dir.into());
        self.pattern = Some(pattern.into());
        self.is_dir = true;
        self
    }

    /// Sets the output file (single-file runs) or output directory
    /// (batch/video runs). Optional — leave unset when rendering straight to
    /// a `String`, `Vec<String>`, or stdout.
    pub fn output(mut self, path: impl Into<PathBuf>) -> Self {
        self.output = Some(path.into());
        self
    }

    /// Caps the number of worker threads used by Rayon for CPU-bound
    /// conversion work. Defaults to the machine's available parallelism.
    pub fn num_threads(mut self, num_threads: usize) -> Self {
        self.num_threads = Some(num_threads);
        self
    }

    /// Sets the terminal color capability used when rendering ANSI output.
    pub fn color_depth(mut self, depth: ColorDepth) -> Self {
        self.color_depth = depth;
        self
    }

    /// Caps output width, deriving height from the image/frame aspect ratio.
    pub fn max_width(mut self, max_width: usize) -> Self {
        self.ascii = self.ascii.output_sizing(OutputSizing::MaxWidth(max_width));
        self
    }

    /// Fixes output width and height explicitly.
    pub fn explicit_dimensions(mut self, width: usize, height: usize) -> Self {
        self.ascii = self
            .ascii
            .output_sizing(OutputSizing::Explicit { width, height });
        self
    }

    /// Selects the character ramp used to map luminance to glyphs.
    pub fn ramp(mut self, ramp: RampType) -> Self {
        self.ascii = self.ascii.ramp(ramp);
        self
    }

    /// Selects the luminance formula used to derive grayscale from RGB.
    pub fn luminance_method(mut self, method: LuminanceMethod) -> Self {
        self.ascii = self.ascii.luminance_method(method);
        self
    }

    /// Corrects for the non-square aspect ratio of terminal character cells.
    pub fn aspect_ratio_correction(mut self, factor: f32) -> Self {
        self.ascii = self.ascii.aspect_ratio_correction(factor);
        self
    }

    /// Minimum row count before `iascii` parallelizes conversion internally
    /// (requires `iascii`'s `parallel` feature).
    pub fn parallel_threshold(mut self, min_rows: usize) -> Self {
        self.ascii = self.ascii.parallel_threshold(min_rows);
        self
    }

    /// Validates all settings and produces an immutable [`Config`].
    pub fn build(self) -> Result<Config, ConfigError> {
        let path = self.path.ok_or(ConfigError::MissingSource)?;

        let source = if self.is_dir {
            if !path.is_dir() {
                return Err(ConfigError::FolderNotFound(path));
            }
            let pattern_str = self.pattern.unwrap_or_else(|| ".*".to_owned());
            let pattern = Regex::new(&pattern_str)
                .map_err(|source| ConfigError::InvalidRegexPattern(pattern_str, source))?;
            InputSource::Directory { dir: path, pattern }
        } else {
            if !path.is_file() {
                return Err(ConfigError::FileNotFound(path));
            }
            InputSource::File(path)
        };

        let ascii = self.ascii.build().map_err(ConfigError::Ascii)?;

        Ok(Config {
            media_type: self.media_type,
            source,
            output: self.output,
            num_threads: self.num_threads.unwrap_or_else(default_parallelism),
            color_depth: self.color_depth,
            ascii,
        })
    }
}

fn default_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use iascii::{ramp::SMOOTH_GRADIENT, Ramp};

    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "maramura-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn build_without_source_fails() {
        let err = Config::builder(MediaType::Image).build().unwrap_err();
        assert!(matches!(err, ConfigError::MissingSource));
    }

    #[test]
    fn build_with_missing_file_fails() {
        let err = Config::builder(MediaType::Image)
            .from_file("/definitely/does/not/exist.png")
            .build()
            .unwrap_err();
        assert!(matches!(err, ConfigError::FileNotFound(_)));
    }

    #[test]
    fn build_with_missing_dir_fails() {
        let err = Config::builder(MediaType::Video)
            .from_dir("/definitely/does/not/exist/dir", ".*")
            .build()
            .unwrap_err();
        assert!(matches!(err, ConfigError::FolderNotFound(_)));
    }

    #[test]
    fn build_with_invalid_regex_fails() {
        let dir = tempdir();
        let err = Config::builder(MediaType::Video)
            .from_dir(&dir, "[unterminated")
            .build()
            .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidRegexPattern(_, _)));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_from_file_succeeds_and_applies_defaults() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"not a real png, just needs to exist").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .build()
            .expect("build should succeed for an existing file");

        assert_eq!(config.media_type, MediaType::Image);
        assert!(matches!(config.source, InputSource::File(p) if p == file));
        assert_eq!(config.color_depth, ColorDepth::TrueColor);
        assert!(config.num_threads >= 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_settings_are_preserved() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let config = Config::builder(MediaType::Video)
            .from_file(&file)
            .num_threads(3)
            .color_depth(ColorDepth::Ansi16)
            .max_width(80)
            .build()
            .unwrap();

        assert_eq!(config.num_threads, 3);
        assert_eq!(config.color_depth, ColorDepth::Ansi16);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_for_single_file_source_returns_that_file() {
        let dir = tempdir();
        let file = dir.join("a.png");
        fs::write(&file, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .build()
            .unwrap();

        assert_eq!(config.files().unwrap(), vec![file]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_for_directory_source_filters_sorts_and_dedupes_by_pattern() {
        let dir = tempdir();
        let a = dir.join("b_frame.png");
        let b = dir.join("a_frame.png");
        let skip = dir.join("frame.txt");
        fs::write(&a, b"stub").unwrap();
        fs::write(&b, b"stub").unwrap();
        fs::write(&skip, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .build()
            .unwrap();

        let files = config.files().unwrap();
        assert_eq!(files, vec![b.clone(), a.clone()], "results must be sorted");
        assert!(!files.contains(&skip));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_for_directory_source_errors_when_nothing_matches() {
        let dir = tempdir();
        fs::write(dir.join("irrelevant.txt"), b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .build()
            .unwrap();

        let err = config.files().unwrap_err();
        assert!(matches!(err, ConfigError::NoMatchingFiles { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_dir_defaults_to_match_all_pattern() {
        let dir = tempdir();
        fs::write(dir.join("one"), b"stub").unwrap();
        fs::write(dir.join("two"), b"stub").unwrap();

        // No pattern given via `from_dir`'s 2-arg form is not directly
        // exercised by the public builder (it always takes a pattern), but
        // an empty/catch-all pattern should still match everything.
        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, ".*")
            .build()
            .unwrap();

        assert_eq!(config.files().unwrap().len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    // --- output() ------------------------------------------------------

    #[test]
    fn output_setter_is_preserved_on_the_built_config() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();
        let out = dir.join("out.txt");

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .output(&out)
            .build()
            .unwrap();

        assert_eq!(config.output, Some(out));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn output_defaults_to_none_when_unset() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .build()
            .unwrap();

        assert_eq!(config.output, None);
        fs::remove_dir_all(&dir).ok();
    }

    // --- from_file / from_dir mutual override ---------------------------

    #[test]
    fn calling_from_file_after_from_dir_switches_to_a_single_file_source() {
        let dir = tempdir();
        let file = dir.join("only.png");
        fs::write(&file, b"stub").unwrap();

        // Start down the directory path, then override with `from_file` —
        // the builder should honor whichever was called last, not merge or
        // error on the conflicting calls.
        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .from_file(&file)
            .build()
            .unwrap();

        assert!(matches!(config.source, InputSource::File(p) if p == file));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn calling_from_dir_after_from_file_switches_to_a_directory_source() {
        let dir = tempdir();
        let file = dir.join("solo.png");
        fs::write(&file, b"stub").unwrap();
        fs::write(dir.join("other.png"), b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .from_dir(&dir, r"\.png$")
            .build()
            .unwrap();

        assert!(matches!(config.source, InputSource::Directory { .. }));
        assert_eq!(config.files().unwrap().len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    // --- num_threads ------------------------------------------------------

    #[test]
    fn num_threads_zero_is_preserved_as_given() {
        // `ConfigBuilder` does not interpret `0` specially — it stores
        // exactly what the caller passed. Any special meaning (e.g. "let
        // Rayon pick") is left up to the consuming thread-pool builder.
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .num_threads(0)
            .build()
            .unwrap();

        assert_eq!(config.num_threads, 0);
        fs::remove_dir_all(&dir).ok();
    }

    // --- ascii-forwarding builder methods ---------------------------------
    //
    // `ramp`, `luminance_method`, `aspect_ratio_correction`, and
    // `parallel_threshold` all just forward into the wrapped
    // `iascii::config::ConfigBuilder`. `iascii::config::Config` doesn't
    // expose these back out as public fields, so these tests only confirm
    // that chaining each one still produces a successfully built `Config`
    // (i.e. no setter accidentally invalidates the builder) rather than
    // inspecting the resulting `iascii` internals directly.

    #[test]
    fn ramp_luminance_aspect_and_parallel_threshold_setters_all_chain_and_build() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let result = Config::builder(MediaType::Image)
            .from_file(&file)
            .ramp(RampType::Dark(Ramp::SmoothGradient(SMOOTH_GRADIENT)))
            .luminance_method(LuminanceMethod::Bt601)
            .aspect_ratio_correction(2.0)
            .parallel_threshold(64)
            .build();

        assert!(
            result.is_ok(),
            "chaining ascii-forwarding setters should not break build(): {:?}",
            result.err()
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_dimensions_setter_does_not_error_on_build() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_file(&file)
            .explicit_dimensions(10, 5)
            .build()
            .unwrap();

        assert_eq!(config.media_type, MediaType::Image);
        fs::remove_dir_all(&dir).ok();
    }

    // --- MediaType ---------------------------------------------------------

    #[test]
    fn media_type_is_preserved_through_build() {
        let dir = tempdir();
        let file = dir.join("input.png");
        fs::write(&file, b"stub").unwrap();

        let image_config = Config::builder(MediaType::Image)
            .from_file(&file)
            .build()
            .unwrap();
        let video_config = Config::builder(MediaType::Video)
            .from_file(&file)
            .build()
            .unwrap();

        assert_eq!(image_config.media_type, MediaType::Image);
        assert_eq!(video_config.media_type, MediaType::Video);
        fs::remove_dir_all(&dir).ok();
    }

    // --- regex edge cases ---------------------------------------------------

    #[test]
    fn directory_pattern_matching_is_case_sensitive() {
        let dir = tempdir();
        fs::write(dir.join("Frame.PNG"), b"stub").unwrap();
        fs::write(dir.join("frame.png"), b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .build()
            .unwrap();

        let files = config.files().unwrap();
        assert_eq!(files, vec![dir.join("frame.png")]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn directory_source_ignores_subdirectories_even_if_name_matches() {
        let dir = tempdir();
        let nested = dir.join("frame.png"); // a directory, not a file
        fs::create_dir_all(&nested).unwrap();
        let real_file = dir.join("real.png");
        fs::write(&real_file, b"stub").unwrap();

        let config = Config::builder(MediaType::Image)
            .from_dir(&dir, r"\.png$")
            .build()
            .unwrap();

        assert_eq!(config.files().unwrap(), vec![real_file]);
        fs::remove_dir_all(&dir).ok();
    }
}
