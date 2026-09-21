//! Named/numbered structure extraction and per-structure styling.
//!
//! Every other rendering path in this crate treats a converted image,
//! video frame, or banner as one flat block of cells to style uniformly
//! (or animate uniformly, via [`crate::colorize::PixelAnimation`]).
//! [`SegmentMap`] instead partitions a single [`Grid`] into **segments** — contiguous regions numbered `1, 2, 3, ...` in scan
//! order (top-to-bottom, left-to-right) and optionally given a name — so a
//! particular structure in the art (a face, a logo, a patch of sky) can be
//! bolded, colored, or animated on its own while the rest of the frame
//! renders exactly as it normally would.
//!
//! Two ways to get a [`SegmentMap`]:
//! - [`SegmentMap::detect`] — automatic connected-component labeling over
//!   the grid's own per-cell pixel colors (flood fill, 4-connected,
//!   merging cells within [`SegmentOptions::color_tolerance`] of a running
//!   region average). Good for photos/renders where a "structure" is
//!   visually a patch of similar color.
//! - [`SegmentMap::manual`] — caller-supplied rectangles (in grid-cell
//!   coordinates), numbered in the order given. Good when you already
//!   know where the structure is (a logo always in the top-left, a
//!   banner's third word, ...).
//!
//! [`SegmentMap::overlay_ids`] renders a quick numbered preview so a
//! caller — human or automated — can see which id corresponds to which
//! region before committing to a [`SegmentStyles`] map.
//!
//! For a *sequence* of frames (a GIF, a video, or a batch of images) where
//! you want to say "on frame 5, animate segment 2 with `Plasma`, but leave
//! segment 2 of every other frame alone", detect a [`SegmentMap`] per
//! frame independently (segments aren't tracked/matched across frames —
//! that would need optical-flow-style correspondence this crate doesn't
//! attempt) and key a style lookup by `(frame_index, segment_id)`; see
//! [`SequenceStyles`].

use crate::colorize::{pixel_color, ColorFn, PixelAnimation};
use crate::palette::{self, Quantized};
use crate::text::Rgb as TextRgb;
use iascii::grid::Grid;
use iascii::render::ColorDepth;
use std::collections::HashMap;
use std::sync::Arc;

/// A rectangular region in grid-cell coordinates (columns/rows, not source
/// image pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[inline]
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// One numbered (and optionally named) structure within a [`SegmentMap`].
#[derive(Debug, Clone)]
pub struct Segment {
    /// Stable, 1-based identifier in scan order — what a [`SegmentStyles`]
    /// map keys off, and what [`SegmentMap::overlay_ids`] prints inside
    /// each of this segment's cells (mod 10, since a cell holds one
    /// character).
    pub id: usize,
    pub name: Option<String>,
    /// The tight bounding box of every cell belonging to this segment —
    /// used as the local coordinate origin for [`SegmentStyle::Animation`]
    /// (see its docs), not a claim that every cell inside the box belongs
    /// to the segment (an irregular detected region generally doesn't
    /// fill its own bounding box).
    pub bounds: Rect,
    pub cell_count: usize,
}

/// Tuning for [`SegmentMap::detect`].
#[derive(Debug, Clone, Copy)]
pub struct SegmentOptions {
    /// Maximum per-channel-ish color distance (squared Euclidean distance
    /// in 0..=255 RGB space, *not* a plain per-channel delta — see
    /// [`SegmentMap::detect`]'s implementation) a cell may have from its
    /// region's running average color and still be folded into that
    /// region. Lower = more, smaller segments; higher = fewer, larger
    /// ones. Defaults to a squared distance of `40*40*3`, i.e. roughly "40
    /// per channel" on average.
    pub color_tolerance: u32,
    /// Regions smaller than this many cells are dropped (their cells
    /// become unsegmented) rather than kept as noise. Defaults to 3.
    pub min_cells: usize,
    /// Caps how many segments `detect` keeps, retaining the largest by
    /// cell count and dropping (unsegmenting) the rest. Defaults to 64 —
    /// generous for a deliberate "recolor a handful of structures" use,
    /// while still bounding worst-case output size on a highly detailed
    /// image. `0` means unlimited.
    pub max_segments: usize,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            color_tolerance: 40 * 40 * 3,
            min_cells: 3,
            max_segments: 64,
        }
    }
}

/// A partition of one [`Grid`]'s cells into numbered/named [`Segment`]s.
#[derive(Debug, Clone)]
pub struct SegmentMap {
    width: u32,
    height: u32,
    segments: Vec<Segment>,
    /// Row-major, `width * height` long; `owner[y*width+x]` is the id of
    /// the segment cell `(x, y)` belongs to, or `None` if it belongs to no
    /// segment (background, or filtered out by `min_cells`/`max_segments`).
    owner: Vec<Option<usize>>,
}

impl SegmentMap {
    /// Automatically partitions `grid` into segments via 4-connected
    /// flood fill over per-cell pixel color, merging a cell into its
    /// north/south/east/west neighbor's region when the cell's color is
    /// within `options.color_tolerance` (squared distance) of that
    /// region's running average color.
    ///
    /// Segments are numbered in the order their *first* cell (scanning
    /// top-to-bottom, left-to-right) is visited, starting at `1`.
    pub fn detect(grid: &Grid, options: SegmentOptions) -> Self {
        let width = grid.width();
        let height = grid.height();
        let w = width as usize;
        let h = height as usize;

        let mut owner: Vec<Option<usize>> = vec![None; w * h];
        let mut visited = vec![false; w * h];
        let mut raw_segments: Vec<(Vec<(u32, u32)>, [f64; 3])> = Vec::new();

        for start_y in 0..height {
            for start_x in 0..width {
                let idx = (start_y as usize) * w + start_x as usize;
                if visited[idx] {
                    continue;
                }
                let Some(cell) = grid.get(start_x, start_y) else {
                    continue;
                };
                if cell.ch == ' ' {
                    visited[idx] = true;
                    continue;
                }

                // BFS flood fill for this region.
                let mut members: Vec<(u32, u32)> = Vec::new();
                let mut sum = [0f64; 3];
                let mut stack = vec![(start_x, start_y)];
                visited[idx] = true;

                while let Some((x, y)) = stack.pop() {
                    let cur_idx = (y as usize) * w + x as usize;
                    let Some(c) = grid.get(x, y) else { continue };
                    let n = members.len().max(1) as f64;
                    let avg = [sum[0] / n, sum[1] / n, sum[2] / n];
                    if !members.is_empty()
                        && sq_dist(avg, rgb_f64(c.color)) > options.color_tolerance as f64
                    {
                        // Belongs to a new region instead; undo the
                        // speculative visit so a later scan can seed it.
                        visited[cur_idx] = false;
                        continue;
                    }

                    members.push((x, y));
                    sum[0] += c.color.r as f64;
                    sum[1] += c.color.g as f64;
                    sum[2] += c.color.b as f64;

                    for (nx, ny) in neighbors(x, y, width, height) {
                        let nidx = (ny as usize) * w + nx as usize;
                        if visited[nidx] {
                            continue;
                        }
                        let Some(nc) = grid.get(nx, ny) else { continue };
                        if nc.ch == ' ' {
                            visited[nidx] = true;
                            continue;
                        }
                        visited[nidx] = true;
                        stack.push((nx, ny));
                    }
                }

                if !members.is_empty() {
                    raw_segments.push((members, sum));
                }
            }
        }

        raw_segments.retain(|(members, _)| members.len() >= options.min_cells);
        if options.max_segments > 0 && raw_segments.len() > options.max_segments {
            raw_segments.sort_by_key(|(members, _)| std::cmp::Reverse(members.len()));
            raw_segments.truncate(options.max_segments);
            // Re-sort back into scan order (by each region's first/top-left
            // member) so ids still increase in a stable, predictable order.
            raw_segments.sort_by_key(|(members, _)| members[0]);
        }

        let mut segments = Vec::with_capacity(raw_segments.len());
        for (i, (members, _)) in raw_segments.into_iter().enumerate() {
            let id = i + 1;
            let (min_x, min_y, max_x, max_y) = members.iter().fold(
                (u32::MAX, u32::MAX, 0u32, 0u32),
                |(minx, miny, maxx, maxy), &(x, y)| {
                    (minx.min(x), miny.min(y), maxx.max(x), maxy.max(y))
                },
            );
            for &(x, y) in &members {
                owner[(y as usize) * w + x as usize] = Some(id);
            }
            segments.push(Segment {
                id,
                name: None,
                bounds: Rect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1),
                cell_count: members.len(),
            });
        }

        Self {
            width,
            height,
            segments,
            owner,
        }
    }

    /// Builds a [`SegmentMap`] from caller-supplied rectangles instead of
    /// automatic detection, numbered `1, 2, 3, ...` in the order given.
    /// Later rectangles take ownership of any cells they overlap with
    /// earlier ones (last-wins), matching how a caller would naturally
    /// read a top-to-bottom list of "region: rect" pairs.
    pub fn manual(width: u32, height: u32, regions: Vec<(Option<String>, Rect)>) -> Self {
        let w = width as usize;
        let h = height as usize;
        let mut owner: Vec<Option<usize>> = vec![None; w * h];
        let mut segments = Vec::with_capacity(regions.len());

        for (i, (name, bounds)) in regions.into_iter().enumerate() {
            let id = i + 1;
            let clamped = Rect::new(
                bounds.x.min(width),
                bounds.y.min(height),
                bounds.width.min(width.saturating_sub(bounds.x)),
                bounds.height.min(height.saturating_sub(bounds.y)),
            );
            let mut cell_count = 0;
            for y in clamped.y..clamped.y + clamped.height {
                for x in clamped.x..clamped.x + clamped.width {
                    owner[(y as usize) * w + x as usize] = Some(id);
                    cell_count += 1;
                }
            }
            segments.push(Segment {
                id,
                name,
                bounds: clamped,
                cell_count,
            });
        }

        Self {
            width,
            height,
            segments,
            owner,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn get(&self, id: usize) -> Option<&Segment> {
        self.segments.iter().find(|s| s.id == id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Segment> {
        self.segments
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
    }

    /// The id of the segment cell `(x, y)` belongs to, if any.
    pub fn segment_id_at(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.owner[(y as usize) * (self.width as usize) + x as usize]
    }

    /// Assigns (or renames) a segment's name after the fact — useful after
    /// [`SegmentMap::detect`], where segments start out unnamed, once
    /// you've looked at [`SegmentMap::overlay_ids`] and know which number
    /// is, say, "face" or "logo".
    pub fn set_name(&mut self, id: usize, name: impl Into<String>) -> bool {
        match self.segments.iter_mut().find(|s| s.id == id) {
            Some(seg) => {
                seg.name = Some(name.into());
                true
            }
            None => false,
        }
    }

    /// Renders `grid` with each segmented cell replaced by its id (last
    /// digit only, since a cell holds one character) instead of its usual
    /// glyph, and unsegmented cells left as a `.`, so a caller can see at
    /// a glance which number to use in a [`SegmentStyles`] map before
    /// styling anything. Uncolored — this is a diagnostic view, not a
    /// final render.
    pub fn overlay_ids(&self) -> String {
        let mut out = String::with_capacity((self.height as usize) * (self.width as usize + 1));
        for y in 0..self.height {
            for x in 0..self.width {
                match self.segment_id_at(x, y) {
                    Some(id) => out.push(char::from_digit((id % 10) as u32, 10).unwrap_or('?')),
                    None => out.push('.'),
                }
            }
            out.push('\n');
        }
        out
    }
}

fn rgb_f64(c: iascii::grid::Rgb) -> [f64; 3] {
    [c.r as f64, c.g as f64, c.b as f64]
}

fn sq_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

fn neighbors(x: u32, y: u32, width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut out = Vec::with_capacity(4);
    if x + 1 < width {
        out.push((x + 1, y));
    }
    if x > 0 {
        out.push((x - 1, y));
    }
    if y + 1 < height {
        out.push((x, y + 1));
    }
    if y > 0 {
        out.push((x, y - 1));
    }
    out.into_iter()
}

/// How a single segment should be drawn, layered on top of the frame's
/// otherwise-normal rendering.
#[derive(Clone)]
pub enum SegmentStyle {
    /// Bold, in the cell's normal (depth-quantized) color.
    Bold,
    /// A flat, explicit color, normal weight.
    Color(TextRgb),
    /// A flat, explicit color, bold.
    BoldColor(TextRgb),
    /// A [`PixelAnimation`], evaluated in coordinates local to this
    /// segment's bounding box (`x - bounds.x, y - bounds.y`) rather than
    /// the whole frame — so e.g. a `Ripple` animation applied to a small
    /// structure ripples from *that structure's* center, not the image's.
    Animation(PixelAnimation),
}

impl SegmentStyle {
    /// Convenience: wraps a raw per-cell function as an
    /// [`SegmentStyle::Animation`], mirroring [`PixelAnimation::custom`].
    pub fn custom_animation(
        f: impl Fn(usize, usize, u64, char) -> TextRgb + Send + Sync + 'static,
    ) -> Self {
        let cf: ColorFn = Arc::new(f);
        Self::Animation(PixelAnimation::Custom(cf))
    }
}

/// A `segment id -> style` lookup for styling a single frame's
/// [`SegmentMap`]. See [`SequenceStyles`] for styling a whole sequence of
/// frames, where the same segment id means different things frame to
/// frame.
#[derive(Default, Clone)]
pub struct SegmentStyles(HashMap<usize, SegmentStyle>);

impl SegmentStyles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, id: usize, style: SegmentStyle) -> Self {
        self.0.insert(id, style);
        self
    }

    /// Like [`SegmentStyles::set`], but resolves `name` against `map`
    /// first. Returns `self` unchanged (a no-op) if no segment has that
    /// name, so this can still be chained fluently; check
    /// [`SegmentMap::find_by_name`] yourself first if that should be an
    /// error instead.
    pub fn set_named(mut self, map: &SegmentMap, name: &str, style: SegmentStyle) -> Self {
        if let Some(seg) = map.find_by_name(name) {
            self.0.insert(seg.id, style);
        }
        self
    }

    pub fn get(&self, id: usize) -> Option<&SegmentStyle> {
        self.0.get(&id)
    }
}

/// A `(frame_index, segment id) -> style` lookup for styling an entire
/// sequence of independently `SegmentMap::detect`-ed frames — see the
/// module docs for why frame index is part of the key.
#[derive(Default, Clone)]
pub struct SequenceStyles(HashMap<(usize, usize), SegmentStyle>);

impl SequenceStyles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, frame_index: usize, segment_id: usize, style: SegmentStyle) -> Self {
        self.0.insert((frame_index, segment_id), style);
        self
    }

    pub fn get(&self, frame_index: usize, segment_id: usize) -> Option<&SegmentStyle> {
        self.0.get(&(frame_index, segment_id))
    }
}

fn cell_style_escape(color: Option<Quantized>, bold: bool) -> Option<String> {
    if color.is_none() && !bold {
        return None;
    }
    let mut s = String::new();
    if bold {
        s.push_str("\x1b[1m");
    }
    if let Some(c) = color {
        s.push_str(&c.escape());
    }
    Some(s)
}

/// Renders one [`Grid`]/[`SegmentMap`] pair to an ANSI string: every cell
/// renders exactly as [`crate::render::grid_to_string`] would at `depth`
/// *unless* it belongs to a segment with an entry in `styles`, in which
/// case that style takes over. Pass `depth: None` for a plain (uncolored)
/// render — segment [`SegmentStyle::Bold`]/[`SegmentStyle::Color`]/
/// [`SegmentStyle::BoldColor`]/[`SegmentStyle::Animation`] colors are
/// dropped too in that case (only bolding, where requested, survives),
/// since there would otherwise be no consistent base to layer a color
/// change on top of.
pub fn render_segments(
    grid: &Grid,
    map: &SegmentMap,
    styles: &SegmentStyles,
    depth: Option<ColorDepth>,
    frame: u64,
) -> String {
    let w = grid.width() as usize;
    let h = grid.height() as usize;
    let mut out = String::with_capacity(h * (w * 12 + 8));
    let mut current: Option<String> = None;

    for y in 0..h {
        for x in 0..w {
            let Some(cell) = grid.get(x as u32, y as u32) else {
                continue;
            };
            if cell.ch == ' ' {
                if current.take().is_some() {
                    out.push_str("\x1b[0m");
                }
                out.push(' ');
                continue;
            }

            let seg_id = map.segment_id_at(x as u32, y as u32);
            let style = seg_id.and_then(|id| styles.get(id));
            let seg_bounds = seg_id.and_then(|id| map.get(id)).map(|s| s.bounds);

            let (bold, color): (bool, Option<Quantized>) = match (style, depth) {
                (None, Some(d)) => (
                    false,
                    Some(palette::quantize_for_depth(rgb_tuple(cell.color), d, None)),
                ),
                (None, None) => (false, None),
                (Some(SegmentStyle::Bold), Some(d)) => (
                    true,
                    Some(palette::quantize_for_depth(rgb_tuple(cell.color), d, None)),
                ),
                (Some(SegmentStyle::Bold), None) => (true, None),
                (Some(SegmentStyle::Color(c)), Some(d)) => (
                    false,
                    Some(palette::quantize_for_depth((c.0, c.1, c.2), d, None)),
                ),
                (Some(SegmentStyle::Color(_)), None) => (false, None),
                (Some(SegmentStyle::BoldColor(c)), Some(d)) => (
                    true,
                    Some(palette::quantize_for_depth((c.0, c.1, c.2), d, None)),
                ),
                (Some(SegmentStyle::BoldColor(_)), None) => (true, None),
                (Some(SegmentStyle::Animation(anim)), Some(d)) => {
                    let bounds = seg_bounds.unwrap_or(Rect::new(0, 0, w as u32, h as u32));
                    let lx = (x as u32).saturating_sub(bounds.x) as usize;
                    let ly = (y as u32).saturating_sub(bounds.y) as usize;
                    let rgb = pixel_color(
                        anim,
                        lx,
                        ly,
                        frame,
                        cell.ch,
                        bounds.width.max(1) as usize,
                        bounds.height.max(1) as usize,
                    );
                    (
                        false,
                        Some(palette::quantize_for_depth((rgb.0, rgb.1, rgb.2), d, None)),
                    )
                }
                (Some(SegmentStyle::Animation(_)), None) => (false, None),
            };

            let escape = cell_style_escape(color, bold);
            if escape != current {
                out.push_str("\x1b[0m");
                if let Some(e) = &escape {
                    out.push_str(e);
                }
                current = escape;
            }
            out.push(cell.ch);
        }
        out.push_str("\x1b[0m\n");
        current = None;
    }

    out
}

fn rgb_tuple(c: iascii::grid::Rgb) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}

/// [`render_segments`] for a whole sequence of frames, each with its own
/// already-detected [`SegmentMap`] (see [`SegmentMap::detect`] called once
/// per frame) and styled via [`SequenceStyles`] instead of
/// [`SegmentStyles`] — so segment 2 of frame 0 and segment 2 of frame 5
/// can be styled completely independently. `grids.len()` must equal
/// `maps.len()`; frames beyond the shorter of the two are ignored.
pub fn render_segment_sequence(
    grids: &[Grid],
    maps: &[SegmentMap],
    styles: &SequenceStyles,
    depth: Option<ColorDepth>,
) -> Vec<String> {
    grids
        .iter()
        .zip(maps.iter())
        .enumerate()
        .map(|(frame_index, (grid, map))| {
            // Adapt SequenceStyles -> SegmentStyles for this one frame by
            // cloning just the entries that apply to it; segment count per
            // frame is small enough (bounded by SegmentOptions::max_segments)
            // that this is cheap relative to the render itself.
            let mut per_frame = SegmentStyles::new();
            for seg in map.segments() {
                if let Some(style) = styles.get(frame_index, seg.id) {
                    per_frame = per_frame.set(seg.id, style.clone());
                }
            }
            render_segments(grid, map, &per_frame, depth, frame_index as u64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iascii::config::{ConfigBuilder as AsciiConfigBuilder, OutputSizing};
    use iascii::convert::convert_image;

    fn two_block_grid() -> Grid {
        // 4x2 grid: left half solid red, right half solid blue.
        let cfg = AsciiConfigBuilder::new()
            .output_sizing(OutputSizing::Explicit {
                width: 4,
                height: 2,
            })
            .build()
            .unwrap();
        #[rustfmt::skip]
        let buf: [u8; 4 * 2 * 3] = [
            255,0,0, 255,0,0, 0,0,255, 0,0,255,
            255,0,0, 255,0,0, 0,0,255, 0,0,255,
        ];
        convert_image(4, 2, &buf, &cfg).unwrap()
    }

    #[test]
    fn detect_splits_two_distinct_color_blocks_into_two_segments() {
        let grid = two_block_grid();
        let map = SegmentMap::detect(&grid, SegmentOptions::default());
        assert_eq!(map.segments().len(), 2);
        assert_eq!(map.segment_id_at(0, 0), map.segment_id_at(1, 1));
        assert_ne!(map.segment_id_at(0, 0), map.segment_id_at(3, 0));
    }

    #[test]
    fn manual_segments_are_numbered_in_given_order_and_last_wins_on_overlap() {
        let map = SegmentMap::manual(
            10,
            10,
            vec![
                (Some("a".into()), Rect::new(0, 0, 5, 5)),
                (Some("b".into()), Rect::new(3, 3, 5, 5)),
            ],
        );
        assert_eq!(map.find_by_name("a").unwrap().id, 1);
        assert_eq!(map.find_by_name("b").unwrap().id, 2);
        // (3,3) is inside both rects; "b" was added second, so it wins.
        assert_eq!(map.segment_id_at(3, 3), Some(2));
        // (0,0) is only inside "a".
        assert_eq!(map.segment_id_at(0, 0), Some(1));
    }

    #[test]
    fn overlay_ids_marks_unsegmented_cells_with_a_dot() {
        let map = SegmentMap::manual(3, 1, vec![(None, Rect::new(0, 0, 1, 1))]);
        let overlay = map.overlay_ids();
        assert_eq!(overlay.trim_end(), "1..");
    }

    #[test]
    fn render_segments_only_recolors_styled_segments() {
        let grid = two_block_grid();
        let map = SegmentMap::detect(&grid, SegmentOptions::default());
        let red_id = map.segment_id_at(0, 0).unwrap();
        let styles = SegmentStyles::new().set(red_id, SegmentStyle::Color(TextRgb::new(9, 9, 9)));
        let out = render_segments(&grid, &map, &styles, Some(ColorDepth::TrueColor), 0);
        assert!(out.contains("38;2;9;9;9"));
        // The unstyled (blue) segment should still show its own original
        // color, not the override.
        assert!(out.contains("38;2;0;0;255"));
    }

    #[test]
    fn min_cells_drops_tiny_noise_regions() {
        let grid = two_block_grid();
        let opts = SegmentOptions {
            min_cells: 100,
            ..SegmentOptions::default()
        };
        let map = SegmentMap::detect(&grid, opts);
        assert!(map.segments().is_empty());
        assert_eq!(map.segment_id_at(0, 0), None);
    }
}
