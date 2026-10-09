// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::Line;
use crate::layout::LineItem;
use crate::style::Brush;
use core::ops::Range;

/// Final segment before the selection consumer adds its newline indicator.
pub(crate) struct SelectionEnd {
    pub(crate) start_x: f64,
    pub(crate) end_x: f64,
    pub(crate) cluster_count: usize,
}

/// Selected horizontal segments using the existing f64 accumulation policy.
/// All box widths participate; leading boxes are excluded and intervening box
/// advances remain deferred until the next cluster.
pub(super) fn segments<B: Brush>(
    line: Line<'_, B>,
    text_range: &Range<usize>,
    mut visit: impl FnMut(f64, f64),
) -> SelectionEnd {
    let metrics = line.metrics();
    let mut start_x = metrics.offset as f64 + metrics.inline_min_coord as f64;
    let mut cur_x = start_x;
    let mut cluster_count = 0;
    let mut box_advance = 0.0;
    let mut have_seen_any_runs = false;
    for item in line.items_nonpositioned() {
        match item {
            LineItem::Run(run) => {
                have_seen_any_runs = true;
                for cluster in run.visual_clusters() {
                    let advance = cluster.advance() as f64 + box_advance;
                    box_advance = 0.0;
                    if text_range.contains(&cluster.text_range().start) {
                        cluster_count += 1;
                        cur_x += advance;
                    } else {
                        if cur_x != start_x {
                            visit(start_x, cur_x);
                        }
                        cur_x += advance;
                        start_x = cur_x;
                    }
                }
            }
            LineItem::InlineBox(inline_box) => {
                box_advance += inline_box.width as f64;
                // Keep leading boxes outside the selected text, matching the
                // existing cursor/selection compatibility policy.
                if !have_seen_any_runs {
                    cur_x += box_advance;
                    box_advance = 0.0;
                    start_x = cur_x;
                }
            }
        }
    }
    SelectionEnd {
        start_x,
        end_x: cur_x,
        cluster_count,
    }
}
