// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Parent-relative alignment after the participating span boxes on a line are known.
//!
//! Font and line-height inputs are layout-wide. A span's positioned descendant union is
//! line-specific: align children before their parent, then accumulate baseline offsets.
//! Line-relative spans remain independent subtrees. This private pass is used only by
//! the enabled logical policy; device and unquantized layouts keep their eager metrics.

use alloc::vec::Vec;
use smallvec::SmallVec;

use super::data::StyleBaselineOffset;
use super::style_metrics::{BoxMetrics, StyleMetrics, shift_from_parent};
use super::{Quantization, Style};
use crate::style::Brush;
use crate::{InlineBox, VerticalAlign};

#[derive(Clone, Copy, Debug)]
pub(super) struct Extents {
    pub(super) over: f32,
    pub(super) under: f32,
}

impl Default for Extents {
    fn default() -> Self {
        // Negative leading and shifted boxes can put the baseline outside the box.
        Self {
            over: f32::NEG_INFINITY,
            under: f32::NEG_INFINITY,
        }
    }
}

impl Extents {
    pub(super) fn or_zero(self) -> Self {
        if self.over == f32::NEG_INFINITY && self.under == f32::NEG_INFINITY {
            Self {
                over: 0.,
                under: 0.,
            }
        } else {
            self
        }
    }

    pub(super) fn is_set(self) -> bool {
        self.over != f32::NEG_INFINITY || self.under != f32::NEG_INFINITY
    }

    #[inline(always)]
    pub(super) fn add(&mut self, offset: f32, over: f32, under: f32) -> bool {
        let over = self.over.max(offset + over);
        let under = self.under.max(under - offset);
        let changed = over != self.over || under != self.under;
        self.over = over;
        self.under = under;
        changed
    }

    pub(super) fn height(self) -> f32 {
        let this = self.or_zero();
        this.over + this.under
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SubtreeExtents {
    pub(super) root: u16,
    pub(super) line_box: Extents,
    pub(super) content_box: Extents,
}

impl SubtreeExtents {
    pub(super) fn new(root: u16) -> Self {
        Self {
            root,
            line_box: Extents::default(),
            content_box: Extents::default(),
        }
    }
}

#[derive(Clone, Debug)]
struct Span {
    style: u16,
    metrics: StyleMetrics,
    align: VerticalAlign,
    own_line: Extents,
    own_content: Extents,
    line: Extents,
    content: Extents,
    shift: f32,
    offset: f32,
}

/// Sparse, parent-first participation state. Cloned with saved line-breaking opportunities.
#[derive(Clone, Debug, Default)]
pub(super) struct LineAlignment {
    spans: Vec<Span>,
    /// Caller-supplied boxes already positioned against the root baseline.
    root_boxes: Extents,
    root_content: Extents,
}

impl LineAlignment {
    pub(super) fn reset(&mut self) {
        self.spans.clear();
        self.root_boxes = Extents::default();
        self.root_content = Extents::default();
    }

    pub(super) fn add_root_box(&mut self, ascent: f32, descent: f32) {
        self.root_boxes.add(0., ascent, descent);
        self.root_content.add(0., ascent, descent);
    }

    /// A public default breaker state can carry caller-supplied root boxes before
    /// being installed on a layout. Retain those extents when enabling its policy.
    pub(super) fn from_root(root: &SubtreeExtents) -> Self {
        Self {
            root_boxes: root.line_box,
            root_content: root.content_box,
            ..Self::default()
        }
    }
    fn index(&self, style: u16) -> Result<usize, usize> {
        self.spans.binary_search_by_key(&style, |span| span.style)
    }

    /// Add a span's own box and all participating ancestors, even textless ones.
    pub(super) fn add_style<B: Brush>(
        &mut self,
        style: u16,
        metrics: &[StyleMetrics],
        styles: &[Style<B>],
    ) -> bool {
        let mut style = style;
        let mut changed = false;
        loop {
            let Some(&metric) = metrics.get(usize::from(style)) else {
                return changed;
            };
            let index = match self.index(style) {
                Ok(index) => index,
                Err(index) => {
                    changed = true;
                    self.spans.insert(
                        index,
                        Span {
                            style,
                            metrics: metric,
                            align: styles[usize::from(style)].vertical_align,
                            own_line: Extents::default(),
                            own_content: Extents::default(),
                            line: Extents::default(),
                            content: Extents::default(),
                            shift: 0.,
                            offset: 0.,
                        },
                    );
                    index
                }
            };
            changed |= self.spans[index]
                .own_line
                .add(0., metric.over, metric.under);
            changed |= self.spans[index]
                .own_content
                .add(0., metric.ascent, metric.descent);
            if style == 0 {
                return changed;
            }
            style = metric.parent;
        }
    }

    pub(super) fn add_run(&mut self, style: u16, metrics: BoxMetrics) -> bool {
        if let Ok(index) = self.index(style) {
            let span = &mut self.spans[index];
            let changed = span.own_line.add(0., metrics.over, metrics.under);
            let content_changed = span.own_content.add(0., metrics.ascent, metrics.descent);
            changed || content_changed
        } else {
            false
        }
    }

    pub(super) fn add_inline_box<B: Brush>(
        &mut self,
        parent: u16,
        inline_box: &InlineBox,
        metrics: &[StyleMetrics],
        styles: &[Style<B>],
        quantization: Quantization,
    ) {
        // An atomic-only root without a strut has no span box of its own. Nested spans
        // still participate. Direct text adds its root box separately via add_style.
        if parent != 0 {
            self.add_style(parent, metrics, styles);
        }
        if self.index(parent).is_err() {
            let Some(&metric) = metrics.get(usize::from(parent)) else {
                return;
            };
            self.spans.push(Span {
                style: parent,
                metrics: metric,
                align: styles[usize::from(parent)].vertical_align,
                own_line: Extents::default(),
                own_content: Extents::default(),
                line: Extents::default(),
                content: Extents::default(),
                shift: 0.,
                offset: 0.,
            });
        }
        if inline_box.vertical_align.is_line_relative() {
            return;
        }
        let index = self.index(parent).unwrap();
        let span = &mut self.spans[index];
        let ascent = inline_box.baseline.unwrap_or(inline_box.height);
        let descent = inline_box.height - ascent;
        let shift = quantization.position_round(shift_from_parent(
            inline_box.vertical_align,
            ascent,
            descent,
            &span.metrics,
        ));
        span.own_line.add(shift, ascent, descent);
        span.own_content.add(shift, ascent, descent);
    }

    /// Rebuild from original contributions, so changing descendant extents can also shrink
    /// or move an ancestor's aligned union. Never accumulate into a previously shifted union.
    pub(super) fn resolve(
        &mut self,
        subtrees: &mut SmallVec<[SubtreeExtents; 2]>,
        quantization: Quantization,
    ) {
        subtrees.clear();
        subtrees.push(SubtreeExtents::new(0));
        for span in &mut self.spans {
            span.line = span.own_line;
            span.content = span.own_content;
            span.shift = 0.;
        }
        for index in (0..self.spans.len()).rev() {
            let span = &self.spans[index];
            if span.style == 0 || span.align.is_line_relative() {
                let subtree = SubtreeExtents {
                    root: span.style,
                    line_box: span.line,
                    content_box: span.content,
                };
                if span.style == 0 {
                    subtrees[0] = subtree;
                } else {
                    subtrees.push(subtree);
                }
                continue;
            }
            let parent = self.index(span.metrics.parent).unwrap();
            let line = span.line;
            let content = span.content;
            let shift = if line.is_set() {
                quantization.position_round(shift_from_parent(
                    span.align,
                    line.over,
                    line.under,
                    &self.spans[parent].metrics,
                ))
            } else {
                0.
            };
            self.spans[index].shift = shift;
            self.spans[parent].line.add(shift, line.over, line.under);
            self.spans[parent]
                .content
                .add(shift, content.over, content.under);
        }
        for index in 0..self.spans.len() {
            let span = &self.spans[index];
            let offset = if span.style == 0 || span.align.is_line_relative() {
                0.
            } else {
                self.spans[self.index(span.metrics.parent).unwrap()].offset + span.shift
            };
            self.spans[index].offset = offset;
        }
        subtrees[0]
            .line_box
            .add(0., self.root_boxes.over, self.root_boxes.under);
        subtrees[0]
            .content_box
            .add(0., self.root_content.over, self.root_content.under);
    }

    pub(super) fn offsets(&self) -> impl Iterator<Item = StyleBaselineOffset> + '_ {
        self.spans.iter().map(|span| StyleBaselineOffset {
            style: span.style,
            aligned_subtree: span.metrics.aligned_subtree,
            baseline_offset: span.offset,
        })
    }
}
