// Copyright 2025 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::layout::alignment::align;
use crate::layout::data::LayoutData;
use crate::layout::inline_edges::InlineEdges;
use crate::style::Brush;
use crate::{InlineBox, VerticalAlign};
use core::cmp::Ordering;
use core::fmt;

use crate::IndentOptions;
use crate::layout::{
    ContentWidths, Style, alignment::Alignment, alignment::AlignmentOptions, line::Line,
    line_break::BreakLines,
};

/// Text layout.
///
/// The [`Debug`] implementation prints a compact summary by default.
/// The alternate form (`{:#?}`) formats the full underlying data.
///
/// [`Debug`]: core::fmt::Debug
#[derive(Clone)]
pub struct Layout<B: Brush> {
    pub(crate) data: LayoutData<B>,
}

impl<B: Brush> fmt::Debug for Layout<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("Layout").field("data", &self.data).finish()
        } else {
            f.debug_struct("Layout")
                .field("text_len", &self.data.text_len)
                .field("width", &self.data.width)
                .field("height", &self.data.height)
                .field("lines", &self.data.lines.len())
                .field("runs", &self.data.runs.len())
                .field("styles", &self.data.styles.len())
                .finish_non_exhaustive()
        }
    }
}

impl<B: Brush> Layout<B> {
    /// Creates an empty layout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the layout while retaining capacity.
    ///
    /// This clears underlying strong references to fonts, which would otherwise
    /// prevent those fonts from being freed until dropping or reusing this layout.
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Sets whether line breaking includes the root span box on every line.
    ///
    /// Set this after building and before breaking lines. Disabling it removes
    /// only the unconditional root contribution; text still contributes its
    /// span boxes and their ancestors, and inline boxes retain their extents.
    /// Existing broken lines are not updated until line breaking runs again.
    ///
    /// This is enabled by default. Calling [`Layout::clear`] or rebuilding into
    /// this layout restores the default.
    pub fn set_include_root_strut(&mut self, include: bool) {
        self.data.include_root_strut = include;
    }

    /// Returns the scale factor provided when creating the layout.
    pub fn scale(&self) -> f32 {
        self.data.scale
    }

    /// Returns the style collection for the layout.
    pub fn styles(&self) -> &[Style<B>] {
        &self.data.styles
    }

    /// Updates vertical alignment without rebuilding or reshaping the text.
    ///
    /// Provide one entry per collected style, in layout units. `None` keeps the existing
    /// alignment. Returns `false` without changing anything if the slice length differs from
    /// [`Self::styles`]; valid updates, including unchanged values, return `true`.
    ///
    /// This reuses cached font and line-height metrics and preserves shaping boundaries,
    /// glyphs and source positions. Call [`Self::break_all_lines`] or [`Self::break_lines`]
    /// again before using positions; existing broken lines are not updated by this method.
    pub fn update_vertical_alignments(&mut self, alignments: &[Option<VerticalAlign>]) -> bool {
        if alignments.len() != self.data.styles.len()
            || self.data.style_metrics.len() != self.data.styles.len()
        {
            return false;
        }
        let mut changed = false;
        for (style, alignment) in self.data.styles.iter_mut().zip(alignments) {
            if let Some(alignment) = alignment {
                if style.vertical_align != *alignment {
                    style.vertical_align = *alignment;
                    changed = true;
                }
            }
        }
        if changed {
            let quantization = self.data.quantization();
            super::style_metrics::refresh_vertical_alignments(
                &self.data.styles,
                quantization,
                &mut self.data.style_metrics,
            );
        }
        true
    }

    /// Updates the horizontal inline box edges of styles without reshaping.
    ///
    /// `edges` has one entry per style in [`Self::styles`]; `None` leaves a style's edges
    /// unchanged. Edges apply to spans tracked by the builder (see
    /// `TreeBuilder::set_style_span_tracking`): opening edges belong to the span's first
    /// fragment and closing edges to its last fragment. Edges are attached to the shaped atoms
    /// next to the span's boundaries when the layout is built, so an edge inside a whole atom
    /// (for example a ligature) follows that atom's end.
    ///
    /// Returns `false` (and changes nothing) if `edges` has the wrong length or a value is
    /// negative or not finite. Call [`Self::break_all_lines`] or [`Self::break_lines`] again
    /// before using positions or content widths.
    pub fn update_inline_edges(&mut self, edges: &[Option<InlineEdges>]) -> bool {
        if edges.len() != self.data.styles.len() {
            return false;
        }
        let valid = |e: &InlineEdges| {
            [
                e.start_margin,
                e.start_border,
                e.start_padding,
                e.end_padding,
                e.end_border,
                e.end_margin,
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.)
        };
        if edges.iter().flatten().any(|e| !valid(e)) {
            return false;
        }
        if self.data.inline_edges.len() != self.data.styles.len() {
            self.data
                .inline_edges
                .resize(self.data.styles.len(), InlineEdges::ZERO);
        }
        let mut changed = false;
        for (current, edge) in self.data.inline_edges.iter_mut().zip(edges) {
            if let Some(edge) = edge {
                if current != edge {
                    *current = *edge;
                    changed = true;
                }
            }
        }
        if changed {
            self.data.refresh_edge_gaps();
        }
        true
    }

    /// Whether a style with nonzero horizontal inline box edges has an edge that no shaped atom
    /// carries (for example a span in a layout without text, or one enclosed by inline boxes).
    /// Such edges take no space.
    pub fn has_unplaced_inline_edges(&self) -> bool {
        self.data
            .unplaced_edge_events
            .iter()
            .any(|event| self.data.edge_event_width(*event) != 0.)
    }

    /// Whether a nonzero inline box edge advance is attached to right-to-left text.
    pub fn has_rtl_inline_edges(&self) -> bool {
        self.data.edge_gaps.iter().any(|gap| {
            let site = &self.data.edge_sites[gap.site as usize];
            self.data.shaped_text.runs()[site.run as usize]
                .bidi_level
                .is_rtl()
        })
    }

    /// The sides (logical start, end) of `style`'s span at which an out-of-flow inline box was
    /// pushed while collapsible whitespace was pending: the box shares the edge's normalized
    /// position, but is placed before that whitespace (its [`InlineBox::index`] precedes the
    /// position). Edges at such a side are not placed consistently with the box.
    ///
    /// This depends only on the built text, not on the current edge values.
    pub fn inline_edge_sides_with_pending_out_of_flow_box(&self, style: usize) -> (bool, bool) {
        use crate::InlineBoxKind;
        use crate::resolve::tree::{SpanControl, SpanEdgeKind};
        let mut sides = (false, false);
        let Ok(style) = u16::try_from(style) else {
            return sides;
        };
        for control in &self.data.span_controls {
            let SpanControl::InlineBox { index, anchor } = *control else {
                continue;
            };
            let Some(placed) = self.data.inline_boxes.get(index) else {
                continue;
            };
            if placed.inline_box.kind == InlineBoxKind::InFlow || placed.inline_box.index == anchor
            {
                continue;
            }
            for edge in &self.data.span_edges {
                if edge.style == style && edge.index == anchor {
                    match edge.kind {
                        SpanEdgeKind::Open => sides.0 = true,
                        SpanEdgeKind::Close => sides.1 = true,
                    }
                }
            }
        }
        sides
    }

    /// The current horizontal inline box edges of `style`.
    pub fn inline_edges(&self, style: usize) -> InlineEdges {
        self.data
            .inline_edges
            .get(style)
            .copied()
            .unwrap_or_default()
    }

    /// Returns the length of the source text, in bytes.
    pub fn text_len(&self) -> usize {
        self.data.text_len
    }

    /// Returns the alignment that was applied to the layout by [`Layout::align`].
    ///
    /// Returns `None` if the layout has not been aligned since it was last built.
    pub fn alignment(&self) -> Option<Alignment> {
        self.data.alignment
    }

    /// The `max_advance` that was used to line break the `Layout`
    pub fn layout_max_advance(&self) -> f32 {
        self.data.layout_max_advance
    }

    /// Returns the computed width of the layout excluding the width of hanging whitespace.
    pub fn width(&self) -> f32 {
        self.data.width
    }

    /// Returns the computed width of the layout including the width of trailing whitespace.
    pub fn full_width(&self) -> f32 {
        self.data.full_width
    }

    /// Returns the y-offset of the baseline of the first line in the layout.
    /// Returns `None` if the layout is empty.
    pub fn first_baseline(&self) -> Option<f32> {
        self.lines().next().map(|line| line.metrics().baseline)
    }

    /// Returns the y-offset of the baseline of the last line in the layout.
    /// Returns `None` if the layout is empty.
    pub fn last_baseline(&self) -> Option<f32> {
        self.lines().next_back().map(|line| line.metrics().baseline)
    }

    /// Calculates the lower and upper bounds on the width of the layout. These
    /// are recalculated every time this method is called.
    ///
    /// This method currently may not return the correct results for
    /// mixed-direction text.
    pub fn calculate_content_widths(&self) -> ContentWidths {
        self.data.calculate_content_widths()
    }

    /// Returns the height of the layout.
    pub fn height(&self) -> f32 {
        self.data.height
    }

    /// Returns the number of lines in the layout.
    pub fn len(&self) -> usize {
        self.data.lines.len()
    }

    /// Returns `true` if the layout is empty.
    pub fn is_empty(&self) -> bool {
        self.data.lines.is_empty()
    }

    /// Returns the line at the specified index.
    ///
    /// Returns `None` if the index is out of bounds, i.e. if it's
    /// not less than [`self.len()`](Self::len).
    pub fn get(&self, index: usize) -> Option<Line<'_, B>> {
        Some(Line {
            index: index as u32,
            layout: self,
            data: self.data.lines.get(index)?,
        })
    }

    /// Returns `true` if the dominant direction of the layout is right-to-left.
    pub fn is_rtl(&self) -> bool {
        self.data.base_level.is_rtl()
    }

    /// Iterator over inline boxes in text index order.
    pub fn inline_boxes(&self) -> impl ExactSizeIterator<Item = &InlineBox> + '_ {
        self.data.inline_boxes.iter().map(|b| &b.inline_box)
    }

    /// Mutable iterator over the inline boxes in text index order.
    pub fn inline_boxes_mut(&mut self) -> impl ExactSizeIterator<Item = &mut InlineBox> + '_ {
        self.data.inline_boxes.iter_mut().map(|b| &mut b.inline_box)
    }

    /// Returns an iterator over the lines in the layout.
    pub fn lines(
        &self,
    ) -> impl ExactSizeIterator<Item = Line<'_, B>> + DoubleEndedIterator + '_ + Clone {
        self.data
            .lines
            .iter()
            .enumerate()
            .map(move |(index, data)| Line {
                index: index as u32,
                layout: self,
                data,
            })
    }

    /// Sets the text-indent for the layout.
    ///
    /// The indent is applied as a margin on the start edge of indented lines, reducing the
    /// available width for line breaking and offsetting content during alignment. Negative
    /// values cause the line to protrude beyond the start edge.
    ///
    /// This must be called before [`Layout::break_all_lines`] or [`Layout::break_lines`],
    /// and before [`Layout::align`].
    pub fn set_text_indent(&mut self, amount: f32, options: IndentOptions) {
        self.data.indent_amount = amount;
        self.data.indent_options = options;
    }

    /// Returns line breaker to compute lines for the layout.
    pub fn break_lines(&mut self) -> BreakLines<'_, B> {
        BreakLines::new(self)
    }

    /// Breaks all lines with the specified maximum advance.
    pub fn break_all_lines(&mut self, max_advance: Option<f32>) {
        self.break_lines()
            .break_remaining(max_advance.unwrap_or(f32::MAX));
    }

    /// Apply alignment to the layout.
    ///
    /// You must perform line breaking prior to aligning, through [`Layout::break_lines`] or
    /// [`Layout::break_all_lines`].
    ///
    /// If a finite `max_advance` is supplied to `Layout::break_all_lines` then that width will be applied
    /// relative to that width. Otherwise alignment will be applied relative to the width of the
    /// longest line as computed by line breaking.
    ///
    /// If line-specific `offset` and `max_advance` are set using the advanced methods on the `BreakLines`
    /// struct then each line will be aligned individually within its line box.
    pub fn align(&mut self, alignment: Alignment, options: AlignmentOptions) {
        align(&mut self.data, alignment, options);
    }

    /// Returns the index and `Line` object for the line containing the
    /// given byte `index` in the source text.
    pub(crate) fn line_for_byte_index(&self, index: usize) -> Option<(usize, Line<'_, B>)> {
        let line_index = self
            .data
            .lines
            .binary_search_by(|line| {
                if index < line.text_range.start {
                    Ordering::Greater
                } else if index >= line.text_range.end {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()?;
        Some((line_index, self.get(line_index)?))
    }

    /// Returns the index and `Line` object for the line containing the
    /// given `offset`.
    ///
    /// The offset is specified in the direction orthogonal to line direction.
    /// For horizontal text, this is a vertical or y offset. If the offset is
    /// on a line boundary, it is considered to be contained by the later line.
    pub(crate) fn line_for_offset(&self, offset: f32) -> Option<(usize, Line<'_, B>)> {
        if offset < 0.0 {
            return Some((0, self.get(0)?));
        }
        let maybe_line_index = self.data.lines.binary_search_by(|line| {
            if offset < line.metrics.block_min_coord {
                Ordering::Greater
            } else if offset >= line.metrics.block_max_coord {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        let line_index = match maybe_line_index {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        Some((line_index, self.get(line_index)?))
    }
}

impl<B: Brush> Default for Layout<B> {
    fn default() -> Self {
        Self {
            data: LayoutData::default(),
        }
    }
}
