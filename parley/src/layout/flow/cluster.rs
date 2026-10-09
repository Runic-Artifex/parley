// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Cluster traversal with the existing run and grapheme accumulation policies.

use crate::layout::line::{Line, LineItem};
use crate::style::Brush;
use crate::{Cluster, ClusterSide};

pub(super) fn from_point<'a, B: Brush>(
    line: Line<'a, B>,
    x: f32,
    exact: bool,
) -> Option<(Cluster<'a, B>, ClusterSide)> {
    let mut offset = line.metrics().offset + line.metrics().inline_min_coord;
    let last_run_index = line.len().saturating_sub(1);
    for item in line.items_nonpositioned() {
        match item {
            LineItem::Run(run) => {
                let is_last_run = run.index as usize == last_run_index;
                let run_advance = run.advance();
                if x > offset + run_advance && (exact || !is_last_run) {
                    offset += run_advance;
                    continue;
                }
                for cluster in run.visual_clusters() {
                    let is_last_cluster = is_last_run && cluster.is_line_right_end_of_run();
                    let cluster_advance = cluster.advance();
                    let edge = offset;
                    offset += cluster_advance;
                    if x > offset && (exact || !is_last_cluster) {
                        continue;
                    }
                    if x < edge && exact {
                        continue;
                    }
                    let side = if x <= edge + cluster_advance * 0.5 {
                        ClusterSide::Left
                    } else {
                        ClusterSide::Right
                    };
                    return Some((cluster, side));
                }
            }
            LineItem::InlineBox(inline_box) => {
                offset += inline_box.width;
            }
        }
    }
    None
}

pub(super) fn visual_offset<'a, B: Brush>(
    line: Line<'a, B>,
    target: Cluster<'a, B>,
) -> Option<f32> {
    let mut offset = line.metrics().offset;
    for run_index in 0..=target.run.index as usize {
        let item = line.item(run_index)?;
        match item {
            LineItem::Run(run) => {
                if run_index != target.run.index as usize {
                    offset += run.advance();
                } else {
                    for cluster in run.visual_clusters() {
                        if cluster.grapheme.char_range().start == target.grapheme.char_range().start
                        {
                            break;
                        }
                        offset += cluster.advance();
                    }
                }
            }
            LineItem::InlineBox(inline_box) => {
                offset += inline_box.width;
            }
        }
    }
    Some(offset)
}
