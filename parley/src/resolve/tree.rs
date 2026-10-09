// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hierarchical tree based style application.
use alloc::{string::String, vec::Vec};

use crate::shape::ShapingBoundary;
use crate::{
    TextSourceRange,
    style::{TextWrapMode, WhiteSpaceCollapse},
};
use core::ops::Range;

use super::{Brush, ResolvedProperty, ResolvedStyle, StyleRun};

#[derive(Debug, Clone)]
struct StyleTreeNode<B: Brush> {
    parent: Option<usize>,
    style: ResolvedStyle<B>,
    style_id: Option<u16>,
    /// The style's id when it is used for whitespace that wrapping is allowed after, which differs
    /// from `style_id` only for spans that disable wrapping.
    wrappable_style_id: Option<u16>,
    anchor: usize,
    end_anchor: usize,
    boundary_before: bool,
    boundary_after: bool,
    ordinary_span: bool,
    has_content: bool,
    boxes_before: usize,
    boxes_after: usize,
    open_order: usize,
    close_order: usize,
}

/// An ordinary span erased from the normalized text, separate from shaping and provenance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EmptySpan {
    pub(crate) index: usize,
    pub(crate) style: u16,
    pub(crate) boxes_before: usize,
}

/// Normalized ordinary-span ownership, independent of shaping and source metadata.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StyleSpan {
    pub(crate) range: Range<usize>,
    pub(crate) style: u16,
    pub(crate) boxes_before: usize,
    pub(crate) boxes_after: usize,
}

/// An authored ordinary-span edge, after whitespace normalization.
///
/// Edge order and box ordinals are independent of the UTF-8 position. In particular,
/// coincident empty spans cannot be reconstructed by sorting or pairing endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpanEdge {
    pub(crate) index: usize,
    pub(crate) style: u16,
    pub(crate) boxes_before: usize,
    pub(crate) kind: SpanEdgeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpanEdgeKind {
    Open,
    Close,
}

/// Authored control order in normalized text, independent of shaped item order.
///
/// Box anchors retain normalization history that `InlineBox::index` loses when
/// an out-of-flow box is pushed while a collapsible space is still pending.
/// These sites are metadata, not used advances or positioned line items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpanControl {
    /// Index in the immutable authored `span_edges` stream.
    Edge(usize),
    /// Global box identity and its normalized byte anchor.
    InlineBox { index: usize, anchor: usize },
}

/// A collapsible whitespace sequence that has not been committed yet.
#[derive(Debug, Clone, Copy)]
struct PendingWhitespace {
    /// The span the sequence started in, which the collapsed space is attributed to.
    span: usize,
    /// Whether any of the collapsed whitespace came from a span that allows wrapping, in which
    /// case the collapsed space is a soft wrap opportunity.
    wrappable: bool,
    source: Option<u64>,
}

#[derive(Clone, Copy)]
enum PendingAnchor {
    Open(usize),
    Close(usize),
    Box(usize),
}

/// Whether `c` is a segment break, i.e. a character which is a forced line break when preserved.
fn is_segment_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Builder for constructing a tree of styles
#[derive(Clone)]
pub(crate) struct TreeStyleBuilder<B: Brush> {
    tree: Vec<StyleTreeNode<B>>,
    style_table: Vec<ResolvedStyle<B>>,
    style_runs: Vec<StyleRun>,
    text: String,
    uncommitted_text: String,
    uncommitted_sources: Vec<TextSourceRange>,
    sources: Vec<TextSourceRange>,
    pending_anchors: Vec<PendingAnchor>,
    boundary_spans: Vec<usize>,
    style_spans: Vec<StyleSpan>,
    span_edges: Vec<SpanEdge>,
    span_controls: Vec<SpanControl>,
    normalized_box_anchors: Vec<usize>,
    track_style_spans: bool,
    next_edge_order: usize,
    box_count: usize,
    current_span: usize,
    /// The span that a not-yet-committed collapsible whitespace sequence belongs to.
    ///
    /// Collapsible whitespace is only committed once it is known to be followed by content in the
    /// same inline formatting context, so that it can collapse across span and inline box
    /// boundaries and be removed at the end of the text.
    pending_whitespace: Option<PendingWhitespace>,
    /// Whether the most recently pushed item is an inline box, in which case pending collapsible
    /// whitespace is not at the start of the inline formatting context.
    last_item_is_inline_box: bool,
}

impl<B: Brush> Default for TreeStyleBuilder<B> {
    fn default() -> Self {
        Self {
            tree: Vec::new(),
            style_table: Vec::new(),
            style_runs: Vec::new(),
            text: String::new(),
            uncommitted_text: String::new(),
            uncommitted_sources: Vec::new(),
            sources: Vec::new(),
            pending_anchors: Vec::new(),
            boundary_spans: Vec::new(),
            style_spans: Vec::new(),
            span_edges: Vec::new(),
            span_controls: Vec::new(),
            normalized_box_anchors: Vec::new(),
            track_style_spans: false,
            next_edge_order: 0,
            box_count: 0,
            current_span: usize::MAX,
            pending_whitespace: None,
            last_item_is_inline_box: false,
        }
    }
}

impl<B: Brush> TreeStyleBuilder<B> {
    /// The style of the span that text is currently being pushed into.
    fn current_style(&self) -> ResolvedStyle<B> {
        self.tree[self.current_span].style.clone()
    }

    /// Prepares the builder for accepting a tree of styles and text.
    ///
    /// The provided `root_style` is the default style applied to all text unless overridden.
    pub(crate) fn begin(&mut self, root_style: ResolvedStyle<B>) {
        self.tree.clear();
        self.style_table.clear();
        self.style_runs.clear();
        self.text.clear();
        self.uncommitted_text.clear();
        self.uncommitted_sources.clear();
        self.sources.clear();
        self.pending_anchors.clear();
        self.boundary_spans.clear();
        self.style_spans.clear();
        self.span_edges.clear();
        self.span_controls.clear();
        self.normalized_box_anchors.clear();
        self.track_style_spans = false;
        self.next_edge_order = 0;
        self.box_count = 0;
        self.pending_whitespace = None;
        self.last_item_is_inline_box = false;

        // The root style is always materialised at index 0 so that it can act as the parent of
        // every other style (and as the strut for otherwise empty lines).
        let mut root_style = root_style;
        root_style.parent = 0;
        self.style_table.push(root_style.clone());
        self.tree.push(StyleTreeNode {
            parent: None,
            style: root_style,
            style_id: Some(0),
            wrappable_style_id: None,
            anchor: 0,
            end_anchor: 0,
            boundary_before: false,
            boundary_after: false,
            ordinary_span: false,
            has_content: false,
            boxes_before: 0,
            boxes_after: 0,
            open_order: usize::MAX,
            close_order: usize::MAX,
        });
        self.current_span = 0;
    }

    /// Records that an inline box has been pushed, so that following collapsible whitespace is not
    /// treated as whitespace at the start of the inline formatting context.
    pub(crate) fn set_last_item_is_inline_box(&mut self) {
        self.last_item_is_inline_box = true;
    }

    /// Applies white space processing to the buffered text and commits the result, leaving any
    /// trailing collapsible whitespace pending.
    pub(crate) fn commit_uncommitted_text(&mut self) {
        let uncommitted_text = core::mem::take(&mut self.uncommitted_text);
        let sources = core::mem::take(&mut self.uncommitted_sources);
        if uncommitted_text.is_empty() {
            return;
        }

        let span = self.current_span;
        match self.tree[span].style.white_space_collapse {
            WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces => {
                if uncommitted_text.starts_with(is_segment_break) {
                    // Pending whitespace is always from a `WhiteSpaceCollapse::Collapse` or
                    // `WhiteSpaceCollapse::PreserveBreaks` span, and following CSS Text 4 § 4.3.1 Rule 1
                    // must be removed if it immediately precedes a preserved segment break.
                    self.discard_pending_whitespace();
                }
                self.flush_pending_whitespace();
                self.commit_text_with_sources(
                    span,
                    &uncommitted_text,
                    0..uncommitted_text.len(),
                    &sources,
                );
            }
            mode @ (WhiteSpaceCollapse::Collapse | WhiteSpaceCollapse::PreserveBreaks) => {
                let mut rest = uncommitted_text.as_str();
                while !rest.is_empty() {
                    let whitespace_len =
                        rest.find(|c| !mode.is_collapsible(c)).unwrap_or(rest.len());
                    if whitespace_len > 0 {
                        // The collapsed space is attributed to the span the whitespace sequence
                        // started in, but is a wrap opportunity if any of the spans it collapses
                        // whitespace from allows wrapping.
                        let wrappable = self.tree[span].style.text_wrap_mode == TextWrapMode::Wrap;
                        let pending =
                            self.pending_whitespace
                                .get_or_insert_with(|| PendingWhitespace {
                                    span,
                                    wrappable,
                                    source: sources
                                        .iter()
                                        .find(|source| {
                                            source
                                                .range
                                                .contains(&(uncommitted_text.len() - rest.len()))
                                        })
                                        .map(|source| source.source),
                                });
                        pending.wrappable |= wrappable;
                        rest = &rest[whitespace_len..];
                        continue;
                    }

                    if rest.starts_with(is_segment_break) {
                        // Collapsible whitespace immediately preceding a segment break is removed.
                        // In `Collapse` mode ASCII `CR` and `LF` newlines are collapsible and never
                        // reach this point, so only `LS` and `PS` are treated as forced breaks. In
                        // `PreserveBreaks` mode `CR` and `LF` are not collapsible and are also
                        // preserved as forced breaks.
                        self.discard_pending_whitespace();
                        let break_len = rest.chars().next().unwrap().len_utf8();
                        let start = uncommitted_text.len() - rest.len();
                        self.commit_text_with_sources(
                            span,
                            &rest[..break_len],
                            start..start + break_len,
                            &sources,
                        );
                        rest = &rest[break_len..];
                        continue;
                    }
                    let text_len = rest
                        .find(|c| mode.is_collapsible(c) || is_segment_break(c))
                        .unwrap_or(rest.len());
                    self.flush_pending_whitespace();
                    let start = uncommitted_text.len() - rest.len();
                    self.commit_text_with_sources(
                        span,
                        &rest[..text_len],
                        start..start + text_len,
                        &sources,
                    );
                    rest = &rest[text_len..];
                }
            }
        }
    }

    /// Resolves a pending collapsible whitespace sequence, committing a single space for it if it
    /// is followed by content that it can collapse into, and dropping it otherwise.
    pub(crate) fn flush_pending_whitespace(&mut self) {
        let Some(pending) = self.pending_whitespace.take() else {
            return;
        };

        // Whitespace at the start of the inline formatting context is removed, as is whitespace
        // immediately following a preserved segment break. Whitespace following a preserved space
        // or tab is retained, as only collapsible whitespace collapses.
        let is_at_start = self.text.is_empty() && !self.last_item_is_inline_box;
        if is_at_start || self.text.ends_with(is_segment_break) {
            self.resolve_pending_anchors(false);
            return;
        }

        self.resolve_pending_anchors(true);
        self.mark_content(pending.span);

        let style_index = if pending.wrappable {
            self.resolve_wrappable_style_id(pending.span)
        } else {
            self.resolve_style_id(pending.span)
        };
        let start = self.text.len();
        self.commit_styled_text(style_index, " ");
        if let Some(source) = pending.source {
            self.record_source(start..start + 1, source);
        }
    }

    fn resolve_pending_anchors(&mut self, retained: bool) {
        for anchor in self.pending_anchors.drain(..) {
            let target = match anchor {
                PendingAnchor::Open(index) => &mut self.tree[index].anchor,
                PendingAnchor::Close(index) => &mut self.tree[index].end_anchor,
                PendingAnchor::Box(index) => &mut self.normalized_box_anchors[index],
            };
            *target += usize::from(retained);
        }
    }

    fn discard_pending_whitespace(&mut self) {
        self.pending_whitespace = None;
        self.resolve_pending_anchors(false);
    }

    /// The style table index of the span that text is currently being pushed into.
    pub(crate) fn resolve_current_style_id(&mut self) -> u16 {
        self.resolve_style_id(self.current_span)
    }

    /// Appends already white space processed `text` to the buffer, attributed to `span`.
    fn commit_text_with_sources(
        &mut self,
        span: usize,
        text: &str,
        input: Range<usize>,
        sources: &[TextSourceRange],
    ) {
        self.mark_content(span);
        let style_index = self.resolve_style_id(span);
        let output_start = self.text.len();
        self.commit_styled_text(style_index, text);
        for source in sources {
            let start = input.start.max(source.range.start);
            let end = input.end.min(source.range.end);
            if start < end {
                self.record_source(
                    output_start + start - input.start..output_start + end - input.start,
                    source.source,
                );
            }
        }
    }

    fn record_source(&mut self, range: Range<usize>, source: u64) {
        if let Some(last) = self
            .sources
            .last_mut()
            .filter(|last| last.source == source && last.range.end == range.start)
        {
            last.range.end = range.end;
        } else {
            self.sources.push(TextSourceRange { range, source });
        }
    }

    pub(crate) fn take_sources(&mut self) -> Vec<TextSourceRange> {
        core::mem::take(&mut self.sources)
    }

    /// Appends already white space processed `text` to the buffer with the given style.
    fn commit_styled_text(&mut self, style_index: u16, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        match self.style_runs.last_mut() {
            Some(run) if run.style_index == style_index && run.range.end == start => {
                run.range.end = self.text.len();
            }
            _ => self.style_runs.push(StyleRun {
                style_index,
                range: start..self.text.len(),
            }),
        }
        self.last_item_is_inline_box = false;
    }

    fn mark_content(&mut self, mut span: usize) {
        loop {
            if self.tree[span].has_content {
                break;
            }
            self.tree[span].has_content = true;
            let Some(parent) = self.tree[span].parent else {
                break;
            };
            span = parent;
        }
    }

    pub(crate) fn mark_current_span_content(&mut self) {
        self.mark_content(self.current_span);
    }

    pub(crate) fn note_inline_box(&mut self) {
        self.normalized_box_anchors.push(self.text.len());
        if self.pending_whitespace.is_some() {
            self.pending_anchors
                .push(PendingAnchor::Box(self.box_count));
        }
        self.box_count += 1;
    }

    /// The index of `span`'s style in the style table, adding it (and any ancestors that have not
    /// yet been added) to the table if necessary. Ancestors are added first so that a style's
    /// parent always has a lower index than the style itself.
    fn resolve_style_id(&mut self, span: usize) -> u16 {
        if let Some(style_id) = self.tree[span].style_id {
            return style_id;
        }
        let parent = self.tree[span]
            .parent
            .expect("root style is materialised in `begin`");
        let parent_id = self.resolve_style_id(parent);
        let mut style = self.tree[span].style.clone();
        style.parent = parent_id;
        let style_id = self.push_style(style);
        self.tree[span].style_id = Some(style_id);
        style_id
    }

    fn push_style(&mut self, style: ResolvedStyle<B>) -> u16 {
        let style_id = self.style_table.len();
        assert!(style_id <= u16::MAX as usize, "too many styles");
        self.style_table.push(style);
        style_id as u16
    }

    /// The index of `span`'s style with wrapping enabled, adding it to the table if necessary.
    fn resolve_wrappable_style_id(&mut self, span: usize) -> u16 {
        if self.tree[span].style.text_wrap_mode == TextWrapMode::Wrap {
            return self.resolve_style_id(span);
        }
        if let Some(style_id) = self.tree[span].wrappable_style_id {
            return style_id;
        }
        // Materialise the span itself first so the wrappable variant shares its parent.
        let span_id = self.resolve_style_id(span);
        let mut style = self.style_table[usize::from(span_id)].clone();
        style.text_wrap_mode = TextWrapMode::Wrap;
        let style_id = self.push_style(style);
        self.tree[span].wrappable_style_id = Some(style_id);
        style_id
    }

    /// The length in bytes of the text committed so far, excluding buffered text.
    pub(crate) fn committed_text_len(&self) -> usize {
        self.text.len()
    }

    pub(crate) fn set_style_span_tracking(&mut self, enabled: bool) {
        self.track_style_spans = enabled;
    }

    pub(crate) fn take_style_spans(&mut self) -> Vec<StyleSpan> {
        core::mem::take(&mut self.style_spans)
    }

    pub(crate) fn take_span_edges(&mut self) -> Vec<SpanEdge> {
        core::mem::take(&mut self.span_edges)
    }

    pub(crate) fn take_span_controls(&mut self) -> Vec<SpanControl> {
        core::mem::take(&mut self.span_controls)
    }

    /// Begins a child span with the given style, which subsequent text is attributed to.
    pub(crate) fn push_style_span(&mut self, style: ResolvedStyle<B>) {
        self.push_style_span_with_shaping_boundaries(style, false, false);
    }

    pub(crate) fn push_style_span_with_shaping_boundaries(
        &mut self,
        style: ResolvedStyle<B>,
        before: bool,
        after: bool,
    ) {
        self.commit_uncommitted_text();

        let open_order = self.next_edge_order;
        self.next_edge_order += 1;
        self.tree.push(StyleTreeNode {
            parent: Some(self.current_span),
            style,
            style_id: None,
            wrappable_style_id: None,
            anchor: self.text.len(),
            end_anchor: self.text.len(),
            boundary_before: before,
            boundary_after: after,
            ordinary_span: true,
            has_content: false,
            boxes_before: self.box_count,
            boxes_after: self.box_count,
            open_order,
            close_order: usize::MAX,
        });
        self.current_span = self.tree.len() - 1;
        if self.pending_whitespace.is_some() {
            self.pending_anchors
                .push(PendingAnchor::Open(self.current_span));
        }
        if before || after {
            self.boundary_spans.push(self.current_span);
        }
    }

    /// Begins a child span with the current style modified by the given properties.
    pub(crate) fn push_style_modification_span(
        &mut self,
        properties: impl Iterator<Item = ResolvedProperty<B>>,
    ) {
        let mut style = self.current_style();
        for prop in properties {
            style.apply(prop);
        }
        self.push_style_span(style);
        self.tree[self.current_span].ordinary_span = false;
    }

    /// Ends the current span, returning to its parent.
    pub(crate) fn pop_style_span(&mut self) {
        self.commit_uncommitted_text();

        self.tree[self.current_span].end_anchor = self.text.len();
        self.tree[self.current_span].boxes_after = self.box_count;
        self.tree[self.current_span].close_order = self.next_edge_order;
        self.next_edge_order += 1;
        // Ownership follows whitespace already pending at closing, independently of whether
        // this edge requests shaping isolation. Later sibling whitespace must not move it.
        if self.tree[self.current_span].ordinary_span && self.pending_whitespace.is_some() {
            self.pending_anchors
                .push(PendingAnchor::Close(self.current_span));
        }

        self.current_span = self.tree[self.current_span]
            .parent
            .expect("Popped root style");
    }

    /// Buffers text in the current span, to be white space processed when it is committed.
    pub(crate) fn push_text(&mut self, text: &str) {
        self.uncommitted_text.push_str(text);
    }

    pub(crate) fn push_text_with_source(&mut self, text: &str, source: u64) {
        let start = self.uncommitted_text.len();
        self.push_text(text);
        if !text.is_empty() {
            self.uncommitted_sources.push(TextSourceRange {
                range: start..start + text.len(),
                source,
            });
        }
    }

    /// Returns captured edges after finishing whitespace normalization and auto-closing spans.
    pub(crate) fn shaping_boundaries(&self) -> Vec<ShapingBoundary> {
        self.boundary_spans
            .iter()
            .flat_map(|&index| {
                let span = &self.tree[index];
                [
                    span.boundary_before.then_some(ShapingBoundary {
                        byte_index: span.anchor,
                    }),
                    span.boundary_after.then_some(ShapingBoundary {
                        byte_index: span.end_anchor,
                    }),
                ]
            })
            .flatten()
            .collect()
    }

    /// Computes style table + style runs and returns the final text buffer.
    #[cfg(test)]
    pub(crate) fn finish(
        &mut self,
        style_table: &mut Vec<ResolvedStyle<B>>,
        style_runs: &mut Vec<StyleRun>,
    ) -> String {
        self.finish_with_empty(style_table, style_runs, &mut Vec::new(), false)
    }

    pub(crate) fn finish_with_empty(
        &mut self,
        style_table: &mut Vec<ResolvedStyle<B>>,
        style_runs: &mut Vec<StyleRun>,
        empty_spans: &mut Vec<EmptySpan>,
        retain_empty: bool,
    ) -> String {
        while self.tree[self.current_span].parent.is_some() {
            self.pop_style_span();
        }

        self.commit_uncommitted_text();

        // Trailing collapsed spaces are removed. Resolve their anchors before materialising
        // empty spans; collecting candidates first keeps nested empty children independent.
        self.discard_pending_whitespace();
        empty_spans.clear();
        if retain_empty {
            let candidates: Vec<_> = self
                .tree
                .iter()
                .enumerate()
                .filter(|(_, span)| span.ordinary_span && !span.has_content)
                .map(|(index, _)| index)
                .collect();
            for span in candidates {
                let style = self.resolve_style_id(span);
                empty_spans.push(EmptySpan {
                    index: self.tree[span].anchor,
                    style,
                    boxes_before: self.tree[span].boxes_before,
                });
            }
            empty_spans.sort_by_key(|span| span.index);
        }

        self.style_spans.clear();
        self.span_edges.clear();
        self.span_controls.clear();
        if self.track_style_spans {
            let mut ordered_edges = Vec::new();
            let candidates: Vec<_> = self
                .tree
                .iter()
                .enumerate()
                .filter(|(_, span)| span.ordinary_span)
                .map(|(index, _)| index)
                .collect();
            for span in candidates {
                let style = self.resolve_style_id(span);
                let span = &self.tree[span];
                self.style_spans.push(StyleSpan {
                    range: span.anchor..span.end_anchor,
                    style,
                    boxes_before: span.boxes_before,
                    boxes_after: span.boxes_after,
                });
                ordered_edges.push((
                    span.open_order,
                    SpanEdge {
                        index: span.anchor,
                        style,
                        boxes_before: span.boxes_before,
                        kind: SpanEdgeKind::Open,
                    },
                ));
                ordered_edges.push((
                    span.close_order,
                    SpanEdge {
                        index: span.end_anchor,
                        style,
                        boxes_before: span.boxes_after,
                        kind: SpanEdgeKind::Close,
                    },
                ));
            }
            ordered_edges.sort_unstable_by_key(|(order, _)| *order);
            self.span_edges
                .extend(ordered_edges.into_iter().map(|(_, edge)| edge));

            // Merge two authored streams by global box ordinal, never by byte
            // sorting. Equal-byte nested/sibling edges keep their exact order.
            let mut next_box = 0;
            let mut previous_anchor = 0;
            for (ordinal, edge) in self.span_edges.iter().enumerate() {
                while next_box < edge.boxes_before {
                    let anchor = self.normalized_box_anchors[next_box];
                    debug_assert!(anchor >= previous_anchor);
                    self.span_controls.push(SpanControl::InlineBox {
                        index: next_box,
                        anchor,
                    });
                    previous_anchor = anchor;
                    next_box += 1;
                }
                debug_assert!(edge.index >= previous_anchor);
                self.span_controls.push(SpanControl::Edge(ordinal));
                previous_anchor = edge.index;
            }
            while next_box < self.normalized_box_anchors.len() {
                let anchor = self.normalized_box_anchors[next_box];
                debug_assert!(anchor >= previous_anchor);
                self.span_controls.push(SpanControl::InlineBox {
                    index: next_box,
                    anchor,
                });
                previous_anchor = anchor;
                next_box += 1;
            }
        }

        style_table.clear();
        style_runs.clear();
        style_table.extend_from_slice(&self.style_table);
        style_runs.extend_from_slice(&self.style_runs);

        if style_runs.is_empty() {
            // If there's no text, the layout still needs the root style, e.g., to size a cursor.
            style_runs.push(StyleRun {
                style_index: 0,
                range: 0..0,
            });
        }

        core::mem::take(&mut self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::ops::Range;

    #[test]
    fn tracked_ownership_closes_follow_only_already_pending_whitespace() {
        for (prefix, inner, suffix, expected, range) in [
            ("f", "i ", "z", "fi z", 1..3),
            ("f", "i", " z", "fi z", 1..2),
            ("f ", "", "i", "f i", 2..2),
            ("f", "", " i", "f i", 1..1),
            (" ", "", "i", "i", 0..0),
            ("f", " ", "", "f", 1..1),
            ("é ", "i ", "z", "é i z", 3..5),
            ("f", "i ", "\nz", "fi z", 1..3),
        ] {
            let style = ResolvedStyle {
                white_space_collapse: WhiteSpaceCollapse::Collapse,
                ..ResolvedStyle::default()
            };
            let mut builder = TreeStyleBuilder::<()>::default();
            builder.begin(style.clone());
            builder.set_style_span_tracking(true);
            builder.push_text(prefix);
            builder.push_style_span(style);
            builder.push_text(inner);
            builder.pop_style_span();
            builder.push_text(suffix);
            let text = builder.finish(&mut Vec::new(), &mut Vec::new());
            let spans = builder.take_style_spans();
            assert_eq!(text, expected);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].range, range, "{prefix:?}/{inner:?}/{suffix:?}");
        }
    }

    #[test]
    fn tracked_ownership_can_be_enabled_after_closing_and_resets_on_begin() {
        let style = ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        };
        let mut builder = TreeStyleBuilder::<()>::default();
        builder.begin(style.clone());
        builder.push_text("f");
        builder.push_style_span(style.clone());
        builder.push_text("i ");
        builder.pop_style_span();
        builder.push_text("z");
        builder.set_style_span_tracking(true);
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "fi z");
        assert_eq!(builder.take_style_spans()[0].range, 1..3);
        builder.begin(style.clone());
        builder.push_style_span(style);
        builder.push_text("x");
        builder.pop_style_span();
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "x");
        assert!(builder.take_style_spans().is_empty());
    }

    #[test]
    fn tracked_ownership_auto_closes_and_keeps_modification_scopes_transparent() {
        let style = ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        };
        let mut builder = TreeStyleBuilder::<()>::default();
        builder.begin(style.clone());
        builder.set_style_span_tracking(true);
        builder.push_text("é");
        builder.push_style_span(style);
        builder.push_text("f");
        builder.push_style_modification_span(core::iter::empty());
        builder.push_text("i ");
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "éfi");
        let spans = builder.take_style_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].range, 2..4);
    }

    #[test]
    fn tracked_ownership_keeps_both_atomic_ordinals_at_equal_text_positions() {
        let style = ResolvedStyle::default();
        let mut builder = TreeStyleBuilder::<()>::default();
        builder.begin(style.clone());
        builder.set_style_span_tracking(true);
        builder.note_inline_box();
        builder.push_style_span(style.clone());
        builder.note_inline_box();
        builder.push_style_span(style);
        builder.note_inline_box();
        builder.pop_style_span();
        builder.note_inline_box();
        builder.pop_style_span();
        builder.finish(&mut Vec::new(), &mut Vec::new());
        let spans = builder.take_style_spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].range, 0..0);
        assert_eq!((spans[0].boxes_before, spans[0].boxes_after), (1, 4));
        assert_eq!(spans[1].range, 0..0);
        assert_eq!((spans[1].boxes_before, spans[1].boxes_after), (2, 3));
    }

    #[test]
    fn tracked_closing_range_drops_pending_space_before_preserved_newline() {
        let style = ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::PreserveBreaks,
            ..ResolvedStyle::default()
        };
        let mut builder = TreeStyleBuilder::<()>::default();
        builder.begin(style.clone());
        builder.set_style_span_tracking(true);
        builder.push_text("f");
        builder.push_style_span(style);
        builder.push_text("i ");
        builder.pop_style_span();
        builder.push_text("\nz");
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "fi\nz");
        assert_eq!(builder.take_style_spans()[0].range, 1..2);
    }

    #[test]
    fn shaping_edges_follow_only_whitespace_pending_at_capture() {
        for (prefix, inner, suffix, expected, edges) in [
            ("f", "i ", "z", "fi z", [1, 3]),
            ("f", "i", " z", "fi z", [1, 2]),
            ("f ", "", "i", "f i", [2, 2]),
            ("f", "", " i", "f i", [1, 1]),
            (" ", "", "i", "i", [0, 0]),
            ("f", " ", "", "f", [1, 1]),
            ("é ", "i ", "z", "é i z", [3, 5]),
        ] {
            let style = ResolvedStyle {
                white_space_collapse: WhiteSpaceCollapse::Collapse,
                ..ResolvedStyle::default()
            };
            let mut builder = TreeStyleBuilder::<u32>::default();
            builder.begin(style.clone());
            builder.push_text(prefix);
            builder.push_style_span_with_shaping_boundaries(style, true, true);
            builder.push_text(inner);
            builder.pop_style_span();
            builder.push_text(suffix);
            assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), expected);
            assert_eq!(
                builder
                    .shaping_boundaries()
                    .iter()
                    .map(|edge| edge.byte_index)
                    .collect::<Vec<_>>(),
                edges,
                "{prefix:?} <span>{inner:?}</span> {suffix:?}",
            );
        }
    }

    #[test]
    fn source_boundaries_do_not_change_normalization_or_style_runs() {
        for mode in [
            WhiteSpaceCollapse::Collapse,
            WhiteSpaceCollapse::PreserveBreaks,
            WhiteSpaceCollapse::Preserve,
            WhiteSpaceCollapse::BreakSpaces,
        ] {
            let input = " \t\u{00e9} f\r\n fi\n\u{2028} \u{00a0}B \t";
            for split in (0..=input.len()).filter(|&i| input.is_char_boundary(i)) {
                let style = ResolvedStyle {
                    white_space_collapse: mode,
                    ..ResolvedStyle::default()
                };
                let mut plain = TreeStyleBuilder::<u32>::default();
                plain.begin(style.clone());
                plain.push_text(&input[..split]);
                plain.push_text(&input[split..]);
                let mut plain_styles = Vec::new();
                let mut plain_runs = Vec::new();
                let text = plain.finish(&mut plain_styles, &mut plain_runs);
                let mut tagged = TreeStyleBuilder::<u32>::default();
                tagged.begin(style);
                tagged.push_text_with_source(&input[..split], 0);
                tagged.push_text_with_source(&input[split..], 1);
                let mut tagged_styles = Vec::new();
                let mut tagged_runs = Vec::new();
                assert_eq!(tagged.finish(&mut tagged_styles, &mut tagged_runs), text);
                assert_eq!(tagged_styles, plain_styles);
                assert_eq!(
                    tagged_runs
                        .iter()
                        .map(|run| (run.style_index, run.range.clone()))
                        .collect::<Vec<_>>(),
                    plain_runs
                        .iter()
                        .map(|run| (run.style_index, run.range.clone()))
                        .collect::<Vec<_>>()
                );
                let sources = tagged.take_sources();
                assert_eq!(
                    sources
                        .iter()
                        .map(|source| source.range.len())
                        .sum::<usize>(),
                    text.len()
                );
                assert!(
                    sources
                        .iter()
                        .all(|source| text.is_char_boundary(source.range.start)
                            && text.is_char_boundary(source.range.end))
                );
                assert!(
                    sources
                        .windows(2)
                        .all(|pair| pair[0].range.end == pair[1].range.start)
                );
            }
        }
    }

    #[test]
    fn preserve_breaks_collapses_across_text_and_style_boundaries() {
        for (input, expected) in [
            ("a \t b\n  c", "a b\nc"),
            (" \ta \t\r \t\n \tb \t", "a\r\nb"),
            ("\n \t\n \t", "\n\n"),
            (" \t", ""),
            ("a \u{2028} \tb \u{2029} c", "a\u{2028}b\u{2029}c"),
            ("a \u{00a0} \u{3000} b", "a \u{00a0} \u{3000} b"),
            ("a\u{000b}\u{000c}\u{0085}b", "a\u{000b}\u{000c}\u{0085}b"),
        ] {
            for split in (0..=input.len()).filter(|&i| input.is_char_boundary(i)) {
                let mut builder = TreeStyleBuilder::<u32>::default();
                builder.begin(ResolvedStyle {
                    white_space_collapse: WhiteSpaceCollapse::PreserveBreaks,
                    ..ResolvedStyle::default()
                });
                builder.push_text(&input[..split]);
                builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
                builder.push_text(&input[split..]);
                builder.pop_style_span();
                let text = builder.finish(&mut Vec::new(), &mut Vec::new());
                assert_eq!(text, expected, "{input:?} split at {split}");
            }
        }
    }

    #[test]
    fn preserve_breaks_keeps_pending_whitespace_provenance_across_modes() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_style_modification_span(
            [
                ResolvedProperty::Brush(1),
                ResolvedProperty::TextWrapMode(TextWrapMode::NoWrap),
            ]
            .into_iter(),
        );
        builder.push_text("a ");
        builder.pop_style_span();
        builder.push_style_modification_span(
            [ResolvedProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::PreserveBreaks,
            )]
            .into_iter(),
        );
        builder.push_text("\tb \t\n \tc");

        let mut styles = Vec::new();
        let mut runs = Vec::new();
        let text = builder.finish(&mut styles, &mut runs);
        assert_eq!(text, "a b\nc");
        let space = runs.iter().find(|run| run.range.contains(&1)).unwrap();
        let style = &styles[space.style_index as usize];
        assert_eq!(style.brush, 1);
        assert_eq!(style.text_wrap_mode, TextWrapMode::Wrap);
        assert_eq!(style.white_space_collapse, WhiteSpaceCollapse::Collapse);
    }

    #[test]
    fn sources_keep_adjacent_same_style_text_separate() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_text_with_source("A", 1);
        builder.push_text_with_source("\u{00e9}", 2);

        let text = builder.finish(&mut Vec::new(), &mut Vec::new());
        assert_eq!(text, "A\u{00e9}");
        assert_eq!(
            builder.take_sources(),
            Vec::from([
                TextSourceRange {
                    range: 0..1,
                    source: 1,
                },
                TextSourceRange {
                    range: 1..3,
                    source: 2,
                },
            ])
        );
    }

    #[test]
    fn sources_use_normalized_output_bytes_for_transformed_non_ascii_text() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());

        // Text transforms happen in the caller. The map intentionally describes
        // their output rather than pretending it still has source-text offsets.
        let transformed = "\u{00df}\u{00e9}".to_uppercase();
        builder.push_text_with_source(&transformed, 41);

        let text = builder.finish(&mut Vec::new(), &mut Vec::new());
        assert_eq!(text, "SS\u{00c9}");
        assert_eq!(
            builder.take_sources(),
            Vec::from([TextSourceRange {
                range: 0..text.len(),
                source: 41,
            }])
        );
    }

    #[test]
    fn sources_follow_pending_whitespace_across_collapse_and_preserve_modes() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_text_with_source("A ", 10);
        builder.push_style_modification_span(
            [ResolvedProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::PreserveBreaks,
            )]
            .into_iter(),
        );
        builder.push_text_with_source("\tB", 20);
        builder.pop_style_span();
        builder.push_style_modification_span(
            [ResolvedProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::Preserve,
            )]
            .into_iter(),
        );
        builder.push_text_with_source("\nC", 30);

        let text = builder.finish(&mut Vec::new(), &mut Vec::new());
        assert_eq!(text, "A B\nC");
        assert_eq!(
            builder.take_sources(),
            Vec::from([
                TextSourceRange {
                    range: 0..2,
                    source: 10,
                },
                TextSourceRange {
                    range: 2..3,
                    source: 20,
                },
                TextSourceRange {
                    range: 3..5,
                    source: 30,
                },
            ])
        );
    }

    #[test]
    fn sources_leave_synthetic_text_and_atomic_boundaries_unattributed() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_text_with_source("A ", 1);
        builder.commit_uncommitted_text();
        builder.flush_pending_whitespace();
        // This is the normalizer state after an atomic inline item. It owns no
        // normalized text range itself, but prevents following whitespace from
        // becoming initial whitespace.
        builder.set_last_item_is_inline_box();
        builder.push_text(" ");
        builder.push_text_with_source("B", 2);

        let text = builder.finish(&mut Vec::new(), &mut Vec::new());
        assert_eq!(text, "A  B");
        assert_eq!(
            builder.take_sources(),
            Vec::from([
                TextSourceRange {
                    range: 0..2,
                    source: 1,
                },
                TextSourceRange {
                    range: 3..4,
                    source: 2,
                },
            ])
        );
    }

    #[test]
    fn begin_clears_sources_before_reusing_the_builder() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_text_with_source("old", 1);
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "old");
        assert_eq!(builder.take_sources().len(), 1);

        builder.begin(ResolvedStyle::default());
        builder.push_text_with_source("new", 2);
        assert_eq!(builder.finish(&mut Vec::new(), &mut Vec::new()), "new");
        assert_eq!(
            builder.take_sources(),
            Vec::from([TextSourceRange {
                range: 0..3,
                source: 2,
            }])
        );
    }

    #[test]
    fn collapses_ascii_whitespace_without_trimming_non_ascii_whitespace() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_text(" \u{00a0}text\u{00a0} ");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "\u{00a0}text\u{00a0}");
    }

    #[test]
    fn collapsible_whitespace_at_span_end_is_committed_with_the_span_style() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
        builder.push_text("A ");
        builder.pop_style_span();
        builder.push_text("B");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "A B");
        assert_eq!(style_runs.len(), 2);
        assert_eq!(style_runs[0].style_index, 1);
        assert_eq!(style_runs[0].range, Range { start: 0, end: 2 });
        assert_eq!(style_runs[1].style_index, 0);
        assert_eq!(style_runs[1].range, Range { start: 2, end: 3 });
    }

    #[test]
    fn collapsible_whitespace_before_preserved_text_is_committed() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_text("A ");
        builder.push_style_modification_span(
            [ResolvedProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::Preserve,
            )]
            .into_iter(),
        );
        builder.push_text("  B  ");
        builder.pop_style_span();
        builder.push_text("  C");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "A   B   C");
    }

    #[test]
    fn collapsible_whitespace_around_a_preserved_segment_break_is_removed() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..ResolvedStyle::default()
        });
        builder.push_text("A  ");
        builder.push_style_modification_span(
            [ResolvedProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::Preserve,
            )]
            .into_iter(),
        );
        builder.push_text("\n");
        builder.pop_style_span();
        builder.push_text("  B");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "A\nB");
    }

    #[test]
    fn reuses_style_id_when_returning_to_parent_span() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_text("A");
        builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
        builder.push_text("B");
        builder.pop_style_span();
        builder.push_text("C");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "ABC");
        assert_eq!(style_table.len(), 2);
        assert_eq!(style_runs.len(), 3);
        assert_eq!(style_runs[0].style_index, 0);
        assert_eq!(style_runs[1].style_index, 1);
        assert_eq!(style_runs[2].style_index, 0);
        assert_eq!(style_runs[0].range, Range { start: 0, end: 1 });
        assert_eq!(style_runs[1].range, Range { start: 1, end: 2 });
        assert_eq!(style_runs[2].range, Range { start: 2, end: 3 });
    }

    #[test]
    fn reuses_root_style_id_across_multiple_pop_return_cycles() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_text("A");
        builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
        builder.push_text("B");
        builder.pop_style_span();
        builder.push_text("C");
        builder.push_style_modification_span([ResolvedProperty::LetterSpacing(1.)].into_iter());
        builder.push_text("D");
        builder.pop_style_span();
        builder.push_text("E");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "ABCDE");
        assert_eq!(style_table.len(), 3);
        assert_eq!(style_runs.len(), 5);
        assert_eq!(style_runs[0].style_index, 0);
        assert_eq!(style_runs[1].style_index, 1);
        assert_eq!(style_runs[2].style_index, 0);
        assert_eq!(style_runs[3].style_index, 2);
        assert_eq!(style_runs[4].style_index, 0);
    }

    #[test]
    fn materialises_ancestor_spans_without_text() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_style_modification_span([ResolvedProperty::FontSize(40.)].into_iter());
        builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
        builder.push_text("B");
        builder.pop_style_span();
        builder.pop_style_span();

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "B");
        assert_eq!(style_table.len(), 3);
        assert_eq!(style_table[0].parent, 0);
        assert_eq!(style_table[1].parent, 0);
        assert_eq!(style_table[1].font_size, 40.);
        assert_eq!(style_table[2].parent, 1);
        assert_eq!(style_table[2].font_size, 20.);
        assert_eq!(style_runs.len(), 1);
        assert_eq!(style_runs[0].style_index, 2);
    }

    #[test]
    fn empty_text_uses_root_style() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(style_table.len(), 1);
        assert_eq!(style_runs.len(), 1);
        assert_eq!(style_runs[0].style_index, 0);
        assert_eq!(style_runs[0].range, Range { start: 0, end: 0 });
    }

    #[test]
    fn reuses_parent_and_root_style_ids_after_nested_pop() {
        let mut builder = TreeStyleBuilder::<u32>::default();
        builder.begin(ResolvedStyle::default());
        builder.push_text("R");
        builder.push_style_modification_span([ResolvedProperty::FontSize(20.)].into_iter());
        builder.push_text("A");
        builder.push_style_modification_span([ResolvedProperty::LetterSpacing(1.)].into_iter());
        builder.push_text("B");
        builder.pop_style_span();
        builder.push_text("C");
        builder.pop_style_span();
        builder.push_text("D");

        let mut style_table = Vec::new();
        let mut style_runs = Vec::new();
        let text = builder.finish(&mut style_table, &mut style_runs);

        assert_eq!(text, "RABCD");
        assert_eq!(style_table.len(), 3);
        assert_eq!(style_runs.len(), 5);
        assert_eq!(style_runs[0].style_index, 0);
        assert_eq!(style_runs[1].style_index, 1);
        assert_eq!(style_runs[2].style_index, 2);
        assert_eq!(style_runs[3].style_index, 1);
        assert_eq!(style_runs[4].style_index, 0);
    }
}
