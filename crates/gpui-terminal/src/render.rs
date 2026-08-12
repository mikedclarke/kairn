//! Terminal rendering module.
//!
//! This module provides [`TerminalRenderer`], which handles efficient rendering of
//! terminal content using GPUI's text and drawing systems.
//!
//! # Rendering Pipeline
//!
//! The renderer processes the terminal grid in several stages:
//!
//! ```text
//! Terminal Grid → Layout Phase → Paint Phase
//!                      │              │
//!                      ├─ Collect backgrounds
//!                      ├─ Batch text runs
//!                      │              │
//!                      │              ├─ Paint default background
//!                      │              ├─ Paint non-default backgrounds
//!                      │              ├─ Paint text characters
//!                      │              └─ Paint cursor
//! ```
//!
//! # Optimizations
//!
//! The renderer includes several optimizations to minimize draw calls:
//!
//! 1. **Background Merging**: Adjacent cells with the same background color are
//!    merged into single rectangles, reducing the number of quads to paint.
//!
//! 2. **Text Batching**: Adjacent cells with identical styling (color, bold, italic)
//!    are grouped into [`BatchedTextRun`]s for efficient text shaping.
//!
//! 3. **Default Background Skip**: Cells with the default background color don't
//!    generate separate background rectangles.
//!
//! 4. **Cell Measurement**: Font metrics are measured once using the '│' (BOX DRAWINGS
//!    LIGHT VERTICAL) character and cached for consistent cell dimensions.
//!
//! # Cell Dimensions
//!
//! Cell size is calculated from actual font metrics using the '│' character,
//! which spans the full cell height in properly designed terminal fonts:
//!
//! - **Width**: Measured from shaped '│' character
//! - **Height**: `(ascent + descent) × line_height_multiplier`
//!
//! The `line_height_multiplier` (default 1.0) can be adjusted to add extra
//! vertical space if needed for specific fonts.
//!
//! # Example
//!
//! ```ignore
//! use gpui::px;
//! use gpui_terminal::{ColorPalette, TerminalRenderer};
//!
//! let renderer = TerminalRenderer::new(
//!     "JetBrains Mono".to_string(),
//!     px(14.0),
//!     1.0,  // line height multiplier
//!     ColorPalette::default(),
//! );
//! ```

use crate::box_drawing;
use crate::colors::ColorPalette;
use crate::event::GpuiEventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape};
use gpui::{
    App, Bounds, Edges, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Point,
    ShapedLine, SharedString, Size, TextRun, UnderlineStyle, Window, px, quad,
    transparent_black,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Cache of shaped text runs keyed by content and style. Shaping is the
/// dominant per-frame cost of painting a terminal, and a TUI frame repeats
/// almost all of its runs from the previous frame, so caching turns steady-
/// state repaints into pure draw calls.
struct ShapeCache {
    /// Font the cached lines were shaped with; changing it clears the cache.
    font_sig: (String, i32),
    map: HashMap<u64, ShapedLine>,
    /// Measured cell metrics (width, unmultiplied line height) for the same
    /// font, so the probe glyph is shaped once per font instead of per frame.
    cell: Option<(Pixels, Pixels)>,
}

impl ShapeCache {
    /// Runs are small; this bound only guards against pathological output
    /// (e.g. streaming random styled text) growing the map without limit.
    const CAPACITY: usize = 8192;

    fn new() -> Self {
        Self {
            font_sig: (String::new(), 0),
            map: HashMap::new(),
            cell: None,
        }
    }
}

/// A row's paint-ready layout: geometry-independent (positions are derived
/// from row/column indices at paint time), so it survives pane moves and
/// stays valid until the grid itself changes.
struct RowLayout {
    backgrounds: Vec<BackgroundRect>,
    /// Horizontal box-drawing spans: (start_col, end_col inclusive, weight, color).
    h_spans: Vec<(usize, usize, box_drawing::LineWeight, Hsla)>,
    /// Box-drawing cells: (col, char, color, horizontal part already drawn by a span).
    box_cells: Vec<(usize, char, Hsla, bool)>,
    /// Shaped text runs: (start_col, shaped line).
    runs: Vec<(usize, ShapedLine)>,
}

/// The whole visible frame laid out and shaped, cached against the terminal's
/// mutation generation. The window repaints far more often than the grid
/// changes (every notify repaints everything in gpui, and the Linux backend
/// repaints continuously); replaying this cache turns those repaints into
/// plain draw calls with no grid walk, no batching, and no shaping.
struct FrameLayout {
    generation: u64,
    font_sig: (String, i32),
    cols: usize,
    default_bg: Hsla,
    /// Cursor as (visible row, col), if on screen and not hidden.
    cursor: Option<(usize, usize)>,
    cursor_color: Hsla,
    /// Shape requested by the application (DECSCUSR), drawn at paint time.
    cursor_shape: CursorShape,
    rows: Vec<RowLayout>,
}

/// A batched run of text with consistent styling.
///
/// This struct groups adjacent terminal cells with identical visual attributes
/// to reduce the number of text rendering calls.
#[derive(Debug, Clone)]
pub struct BatchedTextRun {
    /// The text content to render
    pub text: String,

    /// Starting column position
    pub start_col: usize,

    /// Row position
    pub row: usize,

    /// Foreground color
    pub fg_color: Hsla,

    /// Background color
    pub bg_color: Hsla,

    /// Bold flag
    pub bold: bool,

    /// Italic flag
    pub italic: bool,

    /// Underline flag
    pub underline: bool,
}

/// Background rectangle to paint.
///
/// Represents a rectangular region with a solid color background.
#[derive(Debug, Clone)]
pub struct BackgroundRect {
    /// Starting column position
    pub start_col: usize,

    /// Ending column position (exclusive)
    pub end_col: usize,

    /// Row position
    pub row: usize,

    /// Background color
    pub color: Hsla,
}

impl BackgroundRect {
    /// Check if this rectangle can be merged with another.
    ///
    /// Two rectangles can be merged if they:
    /// - Are on the same row
    /// - Have the same color
    /// - Are horizontally adjacent
    fn can_merge_with(&self, other: &Self) -> bool {
        self.row == other.row && self.color == other.color && self.end_col == other.start_col
    }
}

/// Terminal renderer with font settings and cell dimensions.
///
/// This struct manages the rendering of terminal content, including text,
/// backgrounds, and cursor. It maintains font metrics and provides the
/// [`paint`](Self::paint) method for drawing the terminal grid.
///
/// # Font Metrics
///
/// Cell dimensions are calculated from actual font measurements via
/// [`measure_cell`](Self::measure_cell). This ensures accurate character
/// positioning regardless of the font used.
///
/// # Usage
///
/// The renderer is typically used internally by [`TerminalView`](crate::TerminalView),
/// but can also be used directly for custom rendering:
///
/// ```ignore
/// // Measure cell dimensions (call once per font change)
/// renderer.measure_cell(window);
///
/// // Paint the terminal grid
/// renderer.paint(bounds, padding, &term, window, cx);
/// ```
///
/// # Performance
///
/// For optimal performance:
/// - Call `measure_cell` only when font settings change
/// - The `paint` method is designed to be called every frame
/// - Background and text batching minimize GPU draw calls
#[derive(Clone)]
pub struct TerminalRenderer {
    /// Font family name (e.g., "Fira Code", "Menlo")
    pub font_family: String,

    /// Font size in pixels
    pub font_size: Pixels,

    /// Width of a single character cell
    pub cell_width: Pixels,

    /// Height of a single character cell (line height)
    pub cell_height: Pixels,

    /// Multiplier for line height to accommodate tall glyphs
    pub line_height_multiplier: f32,

    /// Color palette for resolving terminal colors
    pub palette: ColorPalette,

    /// Shaped-run cache shared across the clones made per frame
    shaped_cache: Arc<parking_lot::Mutex<ShapeCache>>,

    /// Laid-out frame reused until the grid's generation moves on
    frame_cache: Arc<parking_lot::Mutex<Option<FrameLayout>>>,
}

impl TerminalRenderer {
    /// Creates a new terminal renderer with the given font settings and color palette.
    ///
    /// # Arguments
    ///
    /// * `font_family` - The name of the font family to use
    /// * `font_size` - The font size in pixels
    /// * `line_height_multiplier` - Multiplier for line height (e.g., 1.2 for 20% extra)
    /// * `palette` - The color palette to use for terminal colors
    ///
    /// # Returns
    ///
    /// A new `TerminalRenderer` instance with default cell dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use gpui::px;
    /// use gpui_terminal::render::TerminalRenderer;
    /// use gpui_terminal::ColorPalette;
    ///
    /// let renderer = TerminalRenderer::new("Fira Code".to_string(), px(14.0), 1.0, ColorPalette::default());
    /// ```
    pub fn new(
        font_family: String,
        font_size: Pixels,
        line_height_multiplier: f32,
        palette: ColorPalette,
    ) -> Self {
        // Default cell dimensions - will be measured on first paint
        // Using 0.6 as approximate em-width ratio for monospace fonts
        let cell_width = font_size * 0.6;
        let cell_height = font_size * 1.4; // Line height with some spacing

        Self {
            font_family,
            font_size,
            cell_width,
            cell_height,
            line_height_multiplier,
            palette,
            shaped_cache: Arc::new(parking_lot::Mutex::new(ShapeCache::new())),
            frame_cache: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Drop every cached shaping and layout product. Call when anything the
    /// caches can't see changes: palette, padding, or line-height multiplier
    /// (font changes invalidate implicitly through the font signature).
    pub fn invalidate_caches(&self) {
        *self.shaped_cache.lock() = ShapeCache::new();
        *self.frame_cache.lock() = None;
    }

    fn font_sig(&self) -> (String, i32) {
        (
            self.font_family.clone(),
            (f32::from(self.font_size) * 100.0) as i32,
        )
    }

    /// The shaped line for a styled run, from cache when its content and
    /// style were shaped before.
    fn shaped_run(&self, run: &BatchedTextRun, text: &str, window: &mut Window) -> ShapedLine {
        let font_sig = self.font_sig();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        run.fg_color.h.to_bits().hash(&mut hasher);
        run.fg_color.s.to_bits().hash(&mut hasher);
        run.fg_color.l.to_bits().hash(&mut hasher);
        run.fg_color.a.to_bits().hash(&mut hasher);
        (run.bold, run.italic, run.underline).hash(&mut hasher);
        let key = hasher.finish();

        let mut cache = self.shaped_cache.lock();
        if cache.font_sig != font_sig {
            cache.map.clear();
            cache.cell = None;
            cache.font_sig = font_sig;
        }
        if let Some(shaped) = cache.map.get(&key) {
            return shaped.clone();
        }

        let font = Font {
            family: self.font_family.clone().into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: if run.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if run.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        };
        let text_run = TextRun {
            len: text.len(),
            font,
            color: run.fg_color,
            background_color: None,
            underline: if run.underline {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(run.fg_color),
                    wavy: false,
                })
            } else {
                None
            },
            strikethrough: None,
        };
        let text: SharedString = text.to_string().into();
        let shaped = window
            .text_system()
            .shape_line(text, self.font_size, &[text_run], None);
        if cache.map.len() >= ShapeCache::CAPACITY {
            cache.map.clear();
        }
        cache.map.insert(key, shaped.clone());
        shaped
    }

    /// Measure cell dimensions based on actual font metrics.
    ///
    /// This method measures the actual width and height of characters
    /// using the GPUI text system. It uses the '│' (BOX DRAWINGS LIGHT VERTICAL)
    /// character which spans the full cell height in properly designed terminal fonts.
    ///
    /// # Arguments
    ///
    /// * `window` - The GPUI window for text system access
    pub fn measure_cell(&mut self, window: &mut Window) {
        // Metrics only depend on the font; reuse the last measurement instead
        // of shaping the probe glyph on every frame.
        let font_sig = self.font_sig();
        {
            let mut cache = self.shaped_cache.lock();
            if cache.font_sig != font_sig {
                cache.map.clear();
                cache.cell = None;
                cache.font_sig = font_sig.clone();
            }
            if let Some((width, line_height)) = cache.cell {
                self.cell_width = width;
                self.cell_height = line_height * self.line_height_multiplier;
                return;
            }
        }

        // Measure using '│' (U+2502, BOX DRAWINGS LIGHT VERTICAL)
        // This character spans the full cell height in terminal fonts, making it
        // ideal for measuring exact cell dimensions used by TUIs
        let font = Font {
            family: self.font_family.clone().into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        let text_run = TextRun {
            len: "│".len(),
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // Shape the box-drawing character to get cell metrics
        let shaped = window
            .text_system()
            .shape_line("│".into(), self.font_size, &[text_run], None);

        // Get the width from the shaped line
        if shaped.width > px(0.0) {
            self.cell_width = shaped.width;
        }

        // Calculate height from ascent + descent with optional multiplier
        let line_height = (shaped.ascent + shaped.descent).ceil();
        if line_height > px(0.0) {
            self.cell_height = line_height * self.line_height_multiplier;
        }
        if shaped.width > px(0.0) && line_height > px(0.0) {
            self.shaped_cache.lock().cell = Some((shaped.width, line_height));
        }
    }

    /// Layout cells into batched text runs and background rects for a single row.
    ///
    /// This method processes a row of terminal cells and groups adjacent cells
    /// with identical styling into batched runs. It also collects background
    /// rectangles that need to be painted.
    ///
    /// # Arguments
    ///
    /// * `row` - The row number
    /// * `cells` - Iterator over (column, Cell) pairs
    /// * `colors` - Terminal color configuration
    ///
    /// # Returns
    ///
    /// A tuple of `(backgrounds, text_runs)` where:
    /// - `backgrounds` is a vector of merged background rectangles
    /// - `text_runs` is a vector of batched text runs
    pub fn layout_row(
        &self,
        row: usize,
        cells: impl Iterator<Item = (usize, Cell)>,
        colors: &Colors,
    ) -> (Vec<BackgroundRect>, Vec<BatchedTextRun>) {
        let mut backgrounds = Vec::new();
        let mut text_runs = Vec::new();

        let mut current_run: Option<BatchedTextRun> = None;
        let mut current_bg: Option<BackgroundRect> = None;

        for (col, cell) in cells {
            // Skip wide character spacers
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            // Extract cell styling
            let fg_color = self.palette.resolve(cell.fg, colors);
            let bg_color = self.palette.resolve(cell.bg, colors);
            let bold = cell.flags.contains(Flags::BOLD);
            let italic = cell.flags.contains(Flags::ITALIC);
            let underline = cell.flags.contains(Flags::UNDERLINE);

            // Get the character (or space if empty)
            let ch = if cell.c == ' ' || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };

            // Handle background rectangles
            if let Some(ref mut bg_rect) = current_bg {
                if bg_rect.color == bg_color && bg_rect.end_col == col {
                    // Extend current background
                    bg_rect.end_col = col + 1;
                } else {
                    // Save current background and start new one
                    backgrounds.push(bg_rect.clone());
                    current_bg = Some(BackgroundRect {
                        start_col: col,
                        end_col: col + 1,
                        row,
                        color: bg_color,
                    });
                }
            } else {
                // Start new background
                current_bg = Some(BackgroundRect {
                    start_col: col,
                    end_col: col + 1,
                    row,
                    color: bg_color,
                });
            }

            // Handle text runs
            if let Some(ref mut run) = current_run {
                if run.fg_color == fg_color
                    && run.bg_color == bg_color
                    && run.bold == bold
                    && run.italic == italic
                    && run.underline == underline
                {
                    // Extend current run
                    run.text.push(ch);
                } else {
                    // Save current run and start new one
                    text_runs.push(run.clone());
                    current_run = Some(BatchedTextRun {
                        text: ch.to_string(),
                        start_col: col,
                        row,
                        fg_color,
                        bg_color,
                        bold,
                        italic,
                        underline,
                    });
                }
            } else {
                // Start new run
                current_run = Some(BatchedTextRun {
                    text: ch.to_string(),
                    start_col: col,
                    row,
                    fg_color,
                    bg_color,
                    bold,
                    italic,
                    underline,
                });
            }
        }

        // Push final run and background
        if let Some(run) = current_run {
            text_runs.push(run);
        }
        if let Some(bg) = current_bg {
            backgrounds.push(bg);
        }

        // Merge adjacent backgrounds with same color
        let merged_backgrounds = self.merge_backgrounds(backgrounds);

        (merged_backgrounds, text_runs)
    }

    /// Merge adjacent background rects with same color.
    ///
    /// This optimization reduces the number of rectangles to paint by
    /// combining horizontally adjacent rectangles that share the same color.
    ///
    /// # Arguments
    ///
    /// * `rects` - Vector of background rectangles to merge
    ///
    /// # Returns
    ///
    /// A new vector with merged rectangles
    fn merge_backgrounds(&self, mut rects: Vec<BackgroundRect>) -> Vec<BackgroundRect> {
        if rects.is_empty() {
            return rects;
        }

        let mut merged = Vec::new();
        let mut current = rects.remove(0);

        for rect in rects {
            if current.can_merge_with(&rect) {
                current.end_col = rect.end_col;
            } else {
                merged.push(current);
                current = rect;
            }
        }

        merged.push(current);
        merged
    }

    /// Paint terminal content to the window.
    ///
    /// The heavy work (walking the grid, batching runs, shaping text) happens
    /// in [`layout_frame`](Self::layout_frame) and is cached against the
    /// terminal's mutation `generation`; a repaint of an unchanged grid
    /// replays the cached layout as plain draw calls.
    ///
    /// # Arguments
    ///
    /// * `bounds` - The bounding box to render within
    /// * `padding` - Padding around the terminal content
    /// * `term` - The terminal state
    /// * `generation` - The terminal's mutation counter at paint time
    /// * `window` - The GPUI window
    /// * `cx` - The application context
    pub fn paint(
        &self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        term: &Term<GpuiEventProxy>,
        generation: u64,
        window: &mut Window,
        cx: &mut App,
    ) {
        let font_sig = self.font_sig();
        let mut cache = self.frame_cache.lock();
        let valid = cache
            .as_ref()
            .is_some_and(|f| f.generation == generation && f.font_sig == font_sig);
        if !valid {
            *cache = Some(self.layout_frame(term, generation, font_sig, window));
        }
        let frame = cache.as_ref().expect("frame cache filled above");
        self.paint_frame(frame, bounds, padding, window, cx);
    }

    /// Walk the visible grid once and produce everything painting needs:
    /// merged backgrounds, box-drawing spans and cells, and shaped text runs.
    fn layout_frame(
        &self,
        term: &Term<GpuiEventProxy>,
        generation: u64,
        font_sig: (String, i32),
        window: &mut Window,
    ) -> FrameLayout {
        let grid = term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let colors = term.colors();
        // Scrolled into history: the visible window starts this many lines
        // above the live screen (grid lines go negative into scrollback).
        let display_offset = grid.display_offset() as i32;

        let default_bg = self.palette.resolve(
            Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
            colors,
        );
        let cursor_color = self.palette.resolve(
            Color::Named(alacritty_terminal::vte::ansi::NamedColor::Cursor),
            colors,
        );
        // The application picks the cursor shape via DECSCUSR (vim's insert
        // bar, Claude Code's prompt) and can hide it entirely, either with
        // the Hidden shape or by turning DECTCEM off.
        let cursor_shape = term.cursor_style().shape;
        let cursor_visible =
            term.mode().contains(TermMode::SHOW_CURSOR) && cursor_shape != CursorShape::Hidden;
        // Cursor shifts down with the content when scrolled into history and
        // disappears once it leaves the visible window.
        let cursor_row = grid.cursor.point.line.0 + display_offset;
        let cursor = (cursor_visible && cursor_row >= 0 && cursor_row < num_lines as i32)
            .then_some((cursor_row as usize, grid.cursor.point.column.0));

        let mut rows = Vec::with_capacity(num_lines);
        for line_idx in 0..num_lines {
            let line = Line(line_idx as i32 - display_offset);

            // Collect cells for this line
            let cells: Vec<(usize, Cell)> = (0..num_cols)
                .map(|col_idx| {
                    let col = Column(col_idx);
                    let point = AlacPoint::new(line, col);
                    let cell = grid[point].clone();
                    (col_idx, cell)
                })
                .collect();

            // Layout the row for backgrounds and batched text runs
            let (backgrounds, text_runs) =
                self.layout_row(line_idx, cells.iter().cloned(), colors);

            // First pass: find horizontal spans of box-drawing characters so
            // continuous lines draw across cells without gaps.
            let mut h_spans = Vec::new();
            let mut processed_horizontal: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut i = 0;
            while i < cells.len() {
                let (col_idx, ref cell) = cells[i];
                if let Some(weight) = box_drawing::get_horizontal_weight(cell.c) {
                    let fg_color = self.palette.resolve(cell.fg, colors);
                    let start_col = col_idx;
                    let mut end_col = col_idx;
                    let mut j = i + 1;
                    while j < cells.len() {
                        let (next_col, ref next_cell) = cells[j];
                        if next_col != end_col + 1 {
                            break;
                        }
                        let next_fg = self.palette.resolve(next_cell.fg, colors);
                        if box_drawing::get_horizontal_weight(next_cell.c) == Some(weight)
                            && next_fg == fg_color
                        {
                            end_col = next_col;
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    for col in start_col..=end_col {
                        processed_horizontal.insert(col);
                    }
                    h_spans.push((start_col, end_col, weight, fg_color));
                    i = j;
                    continue;
                }
                i += 1;
            }

            // Second pass: box-drawing cells, remembering whether a span
            // already covers their horizontal component.
            let mut box_cells = Vec::new();
            for (col_idx, cell) in cells.iter() {
                let ch = cell.c;
                if ch == ' ' || ch == '\0' || !box_drawing::is_box_drawing_char(ch) {
                    continue;
                }
                let fg_color = self.palette.resolve(cell.fg, colors);
                box_cells.push((*col_idx, ch, fg_color, processed_horizontal.contains(col_idx)));
            }

            // Third pass: shape the row's text as its batched styled runs,
            // through the run cache. Box-drawing characters keep their cells
            // as spaces (they paint as quads), all-blank runs drop entirely.
            let mut runs = Vec::new();
            for run in &text_runs {
                let display: String = run
                    .text
                    .chars()
                    .map(|c| {
                        if c == '\0' || box_drawing::is_box_drawing_char(c) {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect();
                if display.trim().is_empty() {
                    continue;
                }
                runs.push((run.start_col, self.shaped_run(run, &display, window)));
            }

            rows.push(RowLayout { backgrounds, h_spans, box_cells, runs });
        }

        FrameLayout {
            generation,
            font_sig,
            cols: num_cols,
            default_bg,
            cursor,
            cursor_color,
            cursor_shape,
            rows,
        }
    }

    /// Replay a laid-out frame as draw calls. Positions derive from the
    /// current bounds and cell metrics, so the same frame stays valid while
    /// the pane moves or the window scrolls around it.
    fn paint_frame(
        &self,
        frame: &FrameLayout,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let default_bg = frame.default_bg;
        let num_lines = frame.rows.len();

        // Paint default background (covers full bounds including padding)
        window.paint_quad(quad(
            bounds,
            px(0.0),
            default_bg,
            Edges::<Pixels>::default(),
            transparent_black(),
            Default::default(),
        ));

        // Content starts after padding
        let origin = Point {
            x: bounds.origin.x + padding.left,
            y: bounds.origin.y + padding.top,
        };

        let fill = |window: &mut Window, b: Bounds<Pixels>, color: Hsla| {
            window.paint_quad(quad(
                b,
                px(0.0),
                color,
                Edges::<Pixels>::default(),
                transparent_black(),
                Default::default(),
            ));
        };

        // Vertical offset centering text in the (possibly multiplied) cell
        let base_height = self.cell_height / self.line_height_multiplier;
        let vertical_offset = (self.cell_height - base_height) / 2.0;

        for (line_idx, row) in frame.rows.iter().enumerate() {
            let row_y = origin.y + self.cell_height * (line_idx as f32);

            // Paint backgrounds
            for bg_rect in &row.backgrounds {
                // Skip if it's the default background color
                if bg_rect.color == default_bg {
                    continue;
                }
                let x = origin.x + self.cell_width * (bg_rect.start_col as f32);
                let width = self.cell_width * ((bg_rect.end_col - bg_rect.start_col) as f32);
                fill(
                    window,
                    Bounds {
                        origin: Point { x, y: row_y },
                        size: Size { width, height: self.cell_height },
                    },
                    bg_rect.color,
                );
            }

            // Extend edge-cell backgrounds into the padding and the
            // partial-cell remainder, so a full-screen TUI reaches the pane
            // edges instead of sitting in a letterboxed grid.
            let num_cols = frame.cols;
            let content_right = origin.x + self.cell_width * (num_cols as f32);
            let bounds_right = bounds.origin.x + bounds.size.width;
            let bounds_bottom = bounds.origin.y + bounds.size.height;
            if let Some(first) = row.backgrounds.first()
                && first.color != default_bg
            {
                fill(
                    window,
                    Bounds {
                        origin: Point { x: bounds.origin.x, y: row_y },
                        size: Size {
                            width: origin.x - bounds.origin.x,
                            height: self.cell_height,
                        },
                    },
                    first.color,
                );
            }
            if let Some(last) = row.backgrounds.last()
                && last.color != default_bg
            {
                fill(
                    window,
                    Bounds {
                        origin: Point { x: content_right, y: row_y },
                        size: Size {
                            width: bounds_right - content_right,
                            height: self.cell_height,
                        },
                    },
                    last.color,
                );
            }
            let top_row = line_idx == 0;
            let bottom_row = line_idx + 1 == num_lines;
            if top_row || bottom_row {
                for bg_rect in &row.backgrounds {
                    if bg_rect.color == default_bg {
                        continue;
                    }
                    let mut x = origin.x + self.cell_width * (bg_rect.start_col as f32);
                    let mut right = origin.x + self.cell_width * (bg_rect.end_col as f32);
                    if bg_rect.start_col == 0 {
                        x = bounds.origin.x;
                    }
                    if bg_rect.end_col == num_cols {
                        right = bounds_right;
                    }
                    if top_row {
                        fill(
                            window,
                            Bounds {
                                origin: Point { x, y: bounds.origin.y },
                                size: Size {
                                    width: right - x,
                                    height: origin.y - bounds.origin.y,
                                },
                            },
                            bg_rect.color,
                        );
                    }
                    if bottom_row {
                        let bottom = row_y + self.cell_height;
                        fill(
                            window,
                            Bounds {
                                origin: Point { x, y: bottom },
                                size: Size {
                                    width: right - x,
                                    height: bounds_bottom - bottom,
                                },
                            },
                            bg_rect.color,
                        );
                    }
                }
            }

            // Box-drawing: horizontal spans first, then vertical components
            let cy = row_y + self.cell_height / 2.0;
            for (start_col, end_col, weight, color) in &row.h_spans {
                let start_x = origin.x + self.cell_width * (*start_col as f32);
                let end_x = origin.x + self.cell_width * ((*end_col + 1) as f32);
                box_drawing::draw_horizontal_span(
                    start_x,
                    end_x,
                    cy,
                    *weight,
                    self.cell_width,
                    *color,
                    window,
                );
            }
            for (col_idx, ch, color, horizontal_drawn) in &row.box_cells {
                let x = origin.x + self.cell_width * (*col_idx as f32);
                let cell_bounds = Bounds {
                    origin: Point { x, y: row_y },
                    size: Size {
                        width: self.cell_width,
                        height: self.cell_height,
                    },
                };
                if *horizontal_drawn {
                    box_drawing::draw_vertical_components(
                        *ch,
                        cell_bounds,
                        *color,
                        self.cell_width,
                        window,
                    );
                } else {
                    box_drawing::draw_box_character(
                        *ch,
                        cell_bounds,
                        *color,
                        self.cell_width,
                        window,
                    );
                }
            }

            // Text runs
            for (start_col, shaped) in &row.runs {
                let x = origin.x + self.cell_width * (*start_col as f32);
                let y = row_y + vertical_offset;
                shaped.paint(Point { x, y }, self.cell_height, window, cx).ok();
            }
        }

        // Cursor, in the shape the application requested (DECSCUSR). Bar and
        // underline thickness scale with the cell so they hold up across
        // font sizes and displays.
        if let Some((cursor_row, cursor_col)) = frame.cursor {
            let cell_origin = Point {
                x: origin.x + self.cell_width * (cursor_col as f32),
                y: origin.y + self.cell_height * (cursor_row as f32),
            };
            let cell_size = Size {
                width: self.cell_width,
                height: self.cell_height,
            };
            let (bounds, border) = match frame.cursor_shape {
                CursorShape::Beam => {
                    let width = (self.cell_width * 0.15).max(px(1.0));
                    let bounds = Bounds {
                        origin: cell_origin,
                        size: Size {
                            width,
                            height: self.cell_height,
                        },
                    };
                    (bounds, Edges::<Pixels>::default())
                }
                CursorShape::Underline => {
                    let height = (self.cell_height * 0.1).max(px(1.0));
                    let bounds = Bounds {
                        origin: Point {
                            x: cell_origin.x,
                            y: cell_origin.y + self.cell_height - height,
                        },
                        size: Size {
                            width: self.cell_width,
                            height,
                        },
                    };
                    (bounds, Edges::<Pixels>::default())
                }
                CursorShape::HollowBlock => {
                    let bounds = Bounds {
                        origin: cell_origin,
                        size: cell_size,
                    };
                    (bounds, Edges::all(px(1.0)))
                }
                // Hidden never reaches paint; layout drops the cursor.
                CursorShape::Block | CursorShape::Hidden => {
                    let bounds = Bounds {
                        origin: cell_origin,
                        size: cell_size,
                    };
                    (bounds, Edges::<Pixels>::default())
                }
            };
            let hollow = border != Edges::<Pixels>::default();
            window.paint_quad(quad(
                bounds,
                px(0.0),
                if hollow {
                    transparent_black()
                } else {
                    frame.cursor_color
                },
                border,
                if hollow {
                    frame.cursor_color
                } else {
                    transparent_black()
                },
                Default::default(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderer_creation() {
        let renderer = TerminalRenderer::new(
            "Fira Code".to_string(),
            px(14.0),
            1.0,
            ColorPalette::default(),
        );
        assert_eq!(renderer.font_family, "Fira Code");
        assert_eq!(renderer.font_size, px(14.0));
        assert_eq!(renderer.line_height_multiplier, 1.0);
    }

    #[test]
    fn test_background_rect_merge() {
        let black = Hsla::black();

        let rect1 = BackgroundRect {
            start_col: 0,
            end_col: 5,
            row: 0,
            color: black,
        };

        let rect2 = BackgroundRect {
            start_col: 5,
            end_col: 10,
            row: 0,
            color: black,
        };

        assert!(rect1.can_merge_with(&rect2));

        let rect3 = BackgroundRect {
            start_col: 5,
            end_col: 10,
            row: 1,
            color: black,
        };

        assert!(!rect1.can_merge_with(&rect3));
    }

    #[test]
    fn test_merge_backgrounds() {
        let renderer = TerminalRenderer::new(
            "monospace".to_string(),
            px(14.0),
            1.0,
            ColorPalette::default(),
        );
        let black = Hsla::black();

        let rects = vec![
            BackgroundRect {
                start_col: 0,
                end_col: 5,
                row: 0,
                color: black,
            },
            BackgroundRect {
                start_col: 5,
                end_col: 10,
                row: 0,
                color: black,
            },
        ];

        let merged = renderer.merge_backgrounds(rects);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_col, 0);
        assert_eq!(merged[0].end_col, 10);
    }
}
