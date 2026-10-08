// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Visual positioning of zero-advance spans, separate from shaping and caret items.

use super::line::{Line, LineItem};
use crate::{InlineBoxKind, style::Brush};

pub(super) fn positions<'a, B: Brush>(
    line: Line<'a, B>,
) -> impl Iterator<Item = (usize, f32)> + Clone + use<'a, B> {
    line.layout.data.empty_spans[line.data.empty_spans.clone()]
        .iter()
        .map(move |span| {
            // Prefer the preceding cluster on this line. At a wrap this keeps the anchor with
            // the breaker's owning line; at a forced break the following line uses its first
            // cluster. Visual offsets are recomputed after justification/alignment.
            let upstream = span
                .index
                .checked_sub(1)
                .and_then(|byte| crate::Cluster::from_byte_index(line.layout, byte))
                .filter(|cluster| cluster.path().line_index() == line.index as usize);
            let downstream = crate::Cluster::from_byte_index(line.layout, span.index)
                .filter(|cluster| cluster.path().line_index() == line.index as usize);
            let mut offset = if let Some(cluster) = upstream {
                cluster.visual_offset().unwrap_or(0.)
                    + if cluster.is_rtl() {
                        0.
                    } else {
                        cluster.advance()
                    }
                    + line.data.metrics.inline_min_coord
            } else if let Some(cluster) = downstream {
                cluster.visual_offset().unwrap_or(0.)
                    + if cluster.is_rtl() {
                        cluster.advance()
                    } else {
                        0.
                    }
                    + line.data.metrics.inline_min_coord
            } else {
                line.data.metrics.inline_min_coord + line.data.metrics.offset
            };
            // Text byte positions alone cannot distinguish empty siblings on either side of
            // contiguous atomic boxes. Retain their tree order and use the owning line's visual
            // item order; out-of-flow boxes have no reserved advance.
            let mut x = line.data.metrics.inline_min_coord + line.data.metrics.offset;
            let mut preceding_box = None;
            let mut following_box = None;
            for (index, item) in line.layout.data.line_items[line.data.item_range.clone()]
                .iter()
                .enumerate()
            {
                match line.item(index) {
                    Some(LineItem::Run(run)) => x += run.advance(),
                    Some(LineItem::InlineBox(inline_box)) => {
                        let width = if inline_box.kind == InlineBoxKind::InFlow {
                            inline_box.width
                        } else {
                            0.
                        };
                        if inline_box.index == span.index {
                            if item.index + 1 == span.boxes_before {
                                preceding_box = Some(x + if item.is_rtl() { 0. } else { width });
                            } else if item.index == span.boxes_before {
                                following_box = Some(x + if item.is_rtl() { width } else { 0. });
                            }
                        }
                        x += width;
                    }
                    None => {}
                }
            }
            if let Some(x) = preceding_box.or(following_box) {
                offset = x;
            }
            if upstream.is_some_and(|cluster| {
                matches!(
                    cluster.style().white_space_collapse,
                    crate::WhiteSpaceCollapse::Collapse | crate::WhiteSpaceCollapse::PreserveBreaks
                )
            }) {
                let metrics = line.metrics();
                let start = metrics.inline_min_coord + metrics.offset;
                let (min, max) = if line.layout.is_rtl() {
                    (start + metrics.hanging_advance, start + metrics.advance)
                } else {
                    (start, start + metrics.advance - metrics.hanging_advance)
                };
                offset = offset.max(min).min(max);
            }
            (usize::from(span.style), offset)
        })
}
