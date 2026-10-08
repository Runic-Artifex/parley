// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Context for layout.

use super::FontContext;
use super::context::LayoutContext;
use super::style::{Brush, StyleProperty, TextStyle};

use super::layout::Layout;

use alloc::{string::String, vec::Vec};
use core::ops::{Bound, Range, RangeBounds};
use parlance::BaseDirection;
use parley_engine::break_overrides::LineBreakOverrideFn;

use crate::inline_box::{InlineBox, LayoutInlineBox};
use crate::resolve::tree::EmptySpan;
use crate::resolve::{ResolvedStyle, StyleRun};
use crate::shape::ShapingBoundary;
use crate::{InlineBoxKind, QuantizationMode};

#[derive(Clone, Copy)]
pub(crate) struct BuilderOptions<'a> {
    scale: f32,
    quantize: bool,
    quantization_mode: QuantizationMode,
    base_direction: BaseDirection,
    line_break_override: Option<&'a LineBreakOverrideFn>,
}

impl BuilderOptions<'_> {
    pub(crate) fn new(scale: f32, quantize: bool) -> Self {
        Self {
            scale,
            quantize,
            quantization_mode: QuantizationMode::Device,
            base_direction: BaseDirection::Auto,
            line_break_override: None,
        }
    }
}

/// Builder for constructing a text layout with ranged attributes.
///
/// This builder does not collapse whitespace. For whitespace collapsing, use the [`TreeBuilder`]
#[must_use]
pub struct RangedBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
}

impl<'b, B: Brush> RangedBuilder<'b, B> {
    pub fn push_default<'a>(&mut self, property: impl Into<StyleProperty<'a, B>>) {
        let resolved =
            self.lcx
                .rcx
                .resolve_property(self.fcx, &property.into(), self.options.scale);
        self.lcx.ranged_style_builder.push_default(resolved);
    }

    pub fn push<'a>(
        &mut self,
        property: impl Into<StyleProperty<'a, B>>,
        range: impl RangeBounds<usize>,
    ) {
        let resolved =
            self.lcx
                .rcx
                .resolve_property(self.fcx, &property.into(), self.options.scale);
        self.lcx.ranged_style_builder.push(resolved, range);
    }

    pub fn push_inline_box(&mut self, inline_box: InlineBox) {
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index: 0,
            baseline_offset: 0.,
        });
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Sets the coordinate space used to quantize font metrics.
    pub fn set_quantization_mode(&mut self, mode: QuantizationMode) {
        self.options.quantization_mode = mode;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    pub fn build_into(self, layout: &mut Layout<B>, text: impl AsRef<str>) {
        // Apply RangedStyleBuilder styles directly to style-table/style-run state.
        self.lcx
            .ranged_style_builder
            .finish(&mut self.lcx.style_table, &mut self.lcx.style_runs);

        // Call generic layout builder method
        build_into_layout(layout, text.as_ref(), self.lcx, self.fcx, self.options, &[]);
    }

    pub fn build(self, text: impl AsRef<str>) -> Layout<B> {
        let mut layout = Layout::default();
        self.build_into(&mut layout, text);
        layout
    }
}

/// Builder for constructing a text layout from a style table and
/// indexed style runs.
///
/// This builder does not collapse whitespace. For whitespace collapsing, use the [`TreeBuilder`]
#[must_use]
pub struct StyleRunBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) len: usize,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
    pub(crate) cursor: usize,
}

impl<'b, B: Brush> StyleRunBuilder<'b, B> {
    /// Reserves additional capacity for styles and runs.
    ///
    /// This is an optional optimization for callers that know counts
    /// up front; call it before pushing styles and runs to reduce
    /// reallocations.
    pub fn reserve(&mut self, additional_styles: usize, additional_runs: usize) {
        self.lcx.style_table.reserve(additional_styles);
        self.lcx.style_runs.reserve(additional_runs);
    }

    /// Adds a fully-specified style to the shared style table and
    /// returns its index.
    pub fn push_style<'family, 'settings>(
        &mut self,
        style: TextStyle<'family, 'settings, B>,
    ) -> u16 {
        let resolved = self
            .lcx
            .rcx
            .resolve_entire_style_set(self.fcx, &style, self.options.scale);
        let style_index = self.lcx.style_table.len();
        assert!(style_index <= u16::MAX as usize, "too many styles");
        self.lcx.style_table.push(resolved);
        style_index as u16
    }

    /// Adds a style run referencing an entry from the style table.
    ///
    /// Runs must be contiguous and non-overlapping, and must cover
    /// `0..text.len()` once all runs have been added.
    pub fn push_style_run(&mut self, style_index: u16, range: impl RangeBounds<usize>) {
        let range = resolve_range(range, self.len);
        assert!(
            range.start == self.cursor,
            "StyleRunBuilder expects contiguous non-overlapping runs"
        );
        assert!(
            range.start <= range.end,
            "StyleRunBuilder expects ordered ranges"
        );
        assert!(
            (style_index as usize) < self.lcx.style_table.len(),
            "StyleRunBuilder expects style indices that were previously added via push_style"
        );
        self.lcx.style_runs.push(StyleRun {
            style_index,
            range: range.clone(),
        });
        self.cursor = range.end;
    }

    pub fn push_inline_box(&mut self, inline_box: InlineBox) {
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index: 0,
            baseline_offset: 0.,
        });
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Sets the coordinate space used to quantize font metrics.
    pub fn set_quantization_mode(&mut self, mode: QuantizationMode) {
        self.options.quantization_mode = mode;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    pub fn build_into(self, layout: &mut Layout<B>, text: impl AsRef<str>) {
        assert!(
            self.cursor == self.len,
            "StyleRunBuilder requires runs that cover the full text"
        );
        if self.lcx.style_runs.is_empty() {
            // This is reachable for empty text. In this case, the layout still needs a style and an
            // empty style run, e.g. to size a cursor.
            if self.lcx.style_table.is_empty() {
                let style = self.lcx.rcx.resolve_entire_style_set(
                    self.fcx,
                    &TextStyle::default(),
                    self.options.scale,
                );
                self.lcx.style_table.push(style);
            }
            self.lcx.style_runs.push(StyleRun {
                style_index: 0,
                range: 0..0,
            });
        }
        build_into_layout(layout, text.as_ref(), self.lcx, self.fcx, self.options, &[]);
    }

    pub fn build(self, text: impl AsRef<str>) -> Layout<B> {
        let mut layout = Layout::default();
        self.build_into(&mut layout, text);
        layout
    }
}

/// Builder for constructing a text layout with a tree of attributes.
#[must_use]
pub struct TreeBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
    pub(crate) retain_empty_spans: bool,
}

/// A normalized UTF-8 byte range attributed to an opaque caller-provided source.
///
/// Source ids are metadata only: they do not introduce style or shaping boundaries.
/// Ranges are nonempty, half-open, sorted and nonoverlapping; untagged output may
/// leave gaps between them. Zero is a valid source id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSourceRange {
    /// Half-open UTF-8 byte bounds in normalized output text.
    pub range: Range<usize>,
    /// Opaque caller-provided identity, including zero.
    pub source: u64,
}

impl<'b, B: Brush> TreeBuilder<'b, B> {
    /// Retains ordinary empty style spans as zero-advance line participants.
    ///
    /// Disabled by default. Modification scopes do not generate participants. Retained spans
    /// share the layout's font metric policy and introduce no text. Non-baseline alignment
    /// interrupts adjacent-text shaping; baseline spans preserve compatible shaping unless
    /// the caller explicitly requests a shaping boundary.
    pub fn set_empty_span_participation(&mut self, enabled: bool) {
        self.retain_empty_spans = enabled;
    }

    pub fn push_style_span(&mut self, style: TextStyle<'_, '_, B>) {
        self.push_style_span_with_shaping_boundaries(style, false, false);
    }

    /// Begins an ordinary span with optional forced itemization boundaries.
    ///
    /// `before` and `after` request a boundary at the span's normalized opening and closing
    /// positions, respectively. Requests are independent of geometric alignment and empty-span
    /// participation. They add no text, source range, layout item or inline advance. Existing
    /// engine rules for shaping context and grapheme starts still apply at these boundaries.
    /// Automatic span closing records the closing boundary as well.
    pub fn push_style_span_with_shaping_boundaries(
        &mut self,
        style: TextStyle<'_, '_, B>,
        before: bool,
        after: bool,
    ) {
        let resolved = self
            .lcx
            .rcx
            .resolve_entire_style_set(self.fcx, &style, self.options.scale);
        self.lcx
            .tree_style_builder
            .push_style_span_with_shaping_boundaries(resolved, before, after);
    }

    pub fn push_style_modification_span<'s, 'iter>(
        &mut self,
        properties: impl IntoIterator<Item = &'iter StyleProperty<'s, B>>,
    ) where
        's: 'iter,
        B: 'iter,
    {
        self.lcx
            .tree_style_builder
            .push_style_modification_span(properties.into_iter().map(|p| {
                self.lcx
                    .rcx
                    .resolve_property(self.fcx, p, self.options.scale)
            }));
    }

    pub fn pop_style_span(&mut self) {
        self.lcx.tree_style_builder.pop_style_span();
    }

    pub fn push_text(&mut self, text: &str) {
        self.lcx.tree_style_builder.push_text(text);
    }

    /// Pushes text with optional provenance retained through whitespace processing.
    ///
    /// Removed whitespace produces no range. A collapsed space belongs to the first
    /// contributing source. Unattributed text pushed with [`Self::push_text`] is omitted
    /// from the source map returned by [`Self::build_into_with_sources`].
    pub fn push_text_with_source(&mut self, text: &str, source: u64) {
        self.lcx
            .tree_style_builder
            .push_text_with_source(text, source);
    }

    pub fn push_inline_box(&mut self, mut inline_box: InlineBox) {
        self.lcx.tree_style_builder.commit_uncommitted_text();

        if inline_box.kind == InlineBoxKind::InFlow {
            self.lcx.tree_style_builder.flush_pending_whitespace();
            self.lcx.tree_style_builder.mark_current_span_content();
            self.lcx.tree_style_builder.set_last_item_is_inline_box();
        }

        // TODO: arrange type better here to factor out the index
        inline_box.index = self.lcx.tree_style_builder.committed_text_len();
        let parent_style_index = self.lcx.tree_style_builder.resolve_current_style_id();
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index,
            baseline_offset: 0.,
        });
        self.lcx.tree_style_builder.note_inline_box();
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Sets the coordinate space used to quantize font metrics.
    pub fn set_quantization_mode(&mut self, mode: QuantizationMode) {
        self.options.quantization_mode = mode;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    #[inline]
    pub fn build_into(self, layout: &mut Layout<B>) -> String {
        let mut empty_spans = Vec::new();
        // Apply TreeStyleBuilder styles to LayoutContext.
        let text = self.lcx.tree_style_builder.finish_with_empty(
            &mut self.lcx.style_table,
            &mut self.lcx.style_runs,
            &mut empty_spans,
            self.retain_empty_spans,
        );
        self.lcx.tree_style_builder.take_sources();

        // Call generic layout builder method
        let shaping_boundaries = span_shaping_boundaries(
            &empty_spans,
            &self.lcx.style_table,
            text.len(),
            self.lcx.tree_style_builder.shaping_boundaries(),
        );
        build_into_layout(
            layout,
            &text,
            self.lcx,
            self.fcx,
            self.options,
            &shaping_boundaries,
        );
        layout.data.empty_spans = empty_spans;

        text
    }

    /// Builds a layout and returns text provenance in the normalized text's byte space.
    pub fn build_into_with_sources(self, layout: &mut Layout<B>) -> (String, Vec<TextSourceRange>) {
        let mut empty_spans = Vec::new();
        let text = self.lcx.tree_style_builder.finish_with_empty(
            &mut self.lcx.style_table,
            &mut self.lcx.style_runs,
            &mut empty_spans,
            self.retain_empty_spans,
        );
        let sources = self.lcx.tree_style_builder.take_sources();
        let shaping_boundaries = span_shaping_boundaries(
            &empty_spans,
            &self.lcx.style_table,
            text.len(),
            self.lcx.tree_style_builder.shaping_boundaries(),
        );
        build_into_layout(
            layout,
            &text,
            self.lcx,
            self.fcx,
            self.options,
            &shaping_boundaries,
        );
        layout.data.empty_spans = empty_spans;
        (text, sources)
    }

    #[inline]
    pub fn build(self) -> (Layout<B>, String) {
        let mut layout = Layout::default();
        let text = self.build_into(&mut layout);
        (layout, text)
    }
}

// These itemization-only boundaries neither add layout items nor alter text or provenance.
// Shaped-run boundaries retain the engine's existing grapheme-start and context rules.
fn span_shaping_boundaries<B: Brush>(
    empty_spans: &[EmptySpan],
    styles: &[ResolvedStyle<B>],
    text_len: usize,
    mut boundaries: Vec<ShapingBoundary>,
) -> Vec<ShapingBoundary> {
    boundaries.extend(
        empty_spans
            .iter()
            .filter(|span| {
                span.index > 0
                    && span.index < text_len
                    && styles[usize::from(span.style)].vertical_align
                        != crate::VerticalAlign::BASELINE
            })
            .map(|span| ShapingBoundary {
                byte_index: span.index,
            }),
    );
    boundaries.retain(|boundary| boundary.byte_index > 0 && boundary.byte_index < text_len);
    boundaries.sort_unstable_by_key(|boundary| boundary.byte_index);
    boundaries.dedup_by_key(|boundary| boundary.byte_index);
    boundaries
}

fn build_into_layout<B: Brush>(
    layout: &mut Layout<B>,
    text: &str,
    lcx: &mut LayoutContext<B>,
    fcx: &mut FontContext,
    options: BuilderOptions<'_>,
    shaping_boundaries: &[ShapingBoundary],
) {
    assert!(
        options.quantization_mode != QuantizationMode::Logical
            || (options.scale.is_finite() && options.scale > 0.),
        "logical quantization requires a finite positive scale"
    );
    if text.is_empty() && lcx.style_runs.is_empty() {
        lcx.style_table.push(ResolvedStyle::default());
        lcx.style_runs.push(StyleRun {
            style_index: 0,
            range: 0..0,
        });
    }
    assert!(
        !lcx.style_runs.is_empty(),
        "at least one style run is required"
    );

    crate::analysis::analyze_text(
        lcx,
        text,
        options.base_direction,
        options.line_break_override,
    );

    layout.data.clear();
    layout.data.scale = options.scale;
    layout.data.quantize = options.quantize;
    layout.data.quantization_mode = options.quantization_mode;
    layout.data.base_level = lcx.analysis.paragraph_level();
    layout.data.text_len = text.len();

    // Characters not covered by any style run (e.g. the space substituted for empty text during
    // analysis) use the root style.
    lcx.char_style_indices.clear();
    lcx.char_style_indices
        .resize(lcx.analysis.char_info().len(), 0);
    let mut char_index = 0;
    for style_run in &lcx.style_runs {
        for _ in text[style_run.range.clone()].chars() {
            lcx.char_style_indices[char_index] = style_run.style_index;
            char_index += 1;
        }
    }

    // Copy the visual styles into the layout
    layout
        .data
        .styles
        .extend(lcx.style_table.iter().map(|s| s.as_layout_style()));

    // Sort the inline boxes as subsequent code assumes that they are in text index order.
    // Note: It's important that this is a stable sort to allow users to control the order of contiguous inline boxes
    //
    // TODO: consider dropping the sort and requiring `push_inline_box` callers to push boxes in text index order
    // (as `TreeBuilder` already does).
    lcx.inline_boxes.sort_by_key(|b| b.inline_box.index);

    {
        super::shape::shape_text(
            &lcx.rcx,
            fcx,
            &lcx.style_table,
            &lcx.inline_boxes,
            shaping_boundaries,
            &lcx.analysis,
            &lcx.char_style_indices,
            &mut lcx.scx,
            text,
            layout,
            &lcx.analysis_data_sources,
        );
    }

    // After shaping, so that the metrics of fonts used for shaping can be reused.
    crate::layout::style_metrics::resolve_style_metrics(
        &lcx.rcx,
        fcx,
        &lcx.style_table,
        &layout.data.shaped_text,
        layout.data.quantization(),
        &mut layout.data.style_metrics,
    );

    // Move inline boxes into the layout
    layout.data.inline_boxes.clear();
    core::mem::swap(&mut layout.data.inline_boxes, &mut lcx.inline_boxes);
}

fn resolve_range(range: impl RangeBounds<usize>, len: usize) -> Range<usize> {
    let start = match range.start_bound() {
        Bound::Unbounded => 0,
        Bound::Included(n) => *n,
        Bound::Excluded(n) => *n + 1,
    };
    let end = match range.end_bound() {
        Bound::Unbounded => len,
        Bound::Included(n) => *n + 1,
        Bound::Excluded(n) => *n,
    };
    start.min(len)..end.min(len)
}
