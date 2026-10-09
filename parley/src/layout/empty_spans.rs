// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Visual positioning of zero-advance spans, separate from shaping and caret items.

use super::flow::LineFlow;
use super::line::Line;
use crate::style::Brush;

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
                .filter(|cluster| cluster.path().line_index() == line.index as usize)
                .map(|cluster| {
                    // A cross-element ligature belongs to its first text item. An empty
                    // element within that atom follows its full advance, not a caret component.
                    cluster
                        .run()
                        .cluster_containing_char(cluster.atom.char_range().end - 1)
                        .unwrap_or(cluster)
                });
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
            let mut preceding_box = None;
            let mut following_box = None;
            for item in LineFlow::new(line.clone()).boxes() {
                if item.inline_box.index == span.index {
                    if item.box_index + 1 == span.boxes_before {
                        preceding_box = Some(item.x + if item.is_rtl { 0. } else { item.advance });
                    } else if item.box_index == span.boxes_before {
                        following_box = Some(item.x + if item.is_rtl { item.advance } else { 0. });
                    }
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
