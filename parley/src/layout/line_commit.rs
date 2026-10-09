// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Emit committed items from explicit original-run slices and original box identities.
//!
//! This layer owns source coverage, not horizontal placement or fitting. The legacy adapter
//! preserves its original first/last narrowing; a future edge scheduler can instead supply
//! several atom-aligned slices of the same original run without replaying that run in full.

use alloc::vec::Vec;
use core::ops::Range;
use parlance::BidiLevel;

use super::{Layout, LayoutItem, LayoutItemKind, LineItemData};
use crate::style::Brush;

/// One logical item to commit. Numeric controls are deliberately not line items.
#[derive(Clone, Debug)]
pub(crate) enum CommitItem {
    TextRun {
        /// Identity in the immutable shaped text, not the committed visual ordinal.
        run_index: usize,
        /// Global, whole-atom endpoints within that original run.
        shaped_cluster_range: Range<u32>,
    },
    InlineBox {
        /// Original `inline_boxes` ordinal, not the box's caller-supplied id or byte site.
        box_index: usize,
        bidi_level: BidiLevel,
    },
}

#[derive(Debug)]
pub(crate) struct CommitSummary {
    pub(crate) text_range: Range<usize>,
    pub(crate) last_item_kind: LayoutItemKind,
    pub(crate) committed_text_run: bool,
    pub(crate) needs_reorder: bool,
}

/// Translate the existing contiguous-item breaker state without changing its grouping.
pub(crate) fn legacy_commit_items<'a, B: Brush>(
    layout: &'a Layout<B>,
    items: &'a [LayoutItem],
    clusters: Range<u32>,
) -> impl Iterator<Item = CommitItem> + 'a {
    let is_text_run = |item: &LayoutItem| item.kind == LayoutItemKind::TextRun;
    let first_run_pos = items.iter().position(is_text_run).unwrap_or(0);
    let last_run_pos = items.iter().rposition(is_text_run).unwrap_or(0);
    items
        .iter()
        .enumerate()
        .filter_map(move |(i, item)| match item.kind {
            LayoutItemKind::InlineBox => Some(CommitItem::InlineBox {
                box_index: item.index,
                bidi_level: item.bidi_level,
            }),
            LayoutItemKind::TextRun => {
                let shaped_run = &layout.data.shaped_text.runs()[item.index];
                let mut range = shaped_run.shaped_clusters_range.clone();
                if i == first_run_pos {
                    range.start = clusters.start;
                }
                if i == last_run_pos {
                    range.end = clusters.end;
                }
                // The legacy breaker revisits the preceding run at the next line's start.
                // Preserve its consumed-run skip here, rather than accepting invalid explicit slices.
                if range.start >= shaped_run.shaped_clusters_range.end {
                    return None;
                }
                Some(CommitItem::TextRun {
                    run_index: item.index,
                    shaped_cluster_range: range,
                })
            }
        })
}

/// Append logical slices before bidi reordering. Every slice derives bytes and bidi level
/// from its original shaped run. The caller owns exactly-once consumption and item ordering.
pub(crate) fn append_line_items<B: Brush>(
    layout: &Layout<B>,
    output: &mut Vec<LineItemData>,
    items: impl IntoIterator<Item = CommitItem>,
) -> CommitSummary {
    let shaped_text = &layout.data.shaped_text;
    let shaped_clusters = shaped_text.shaped_clusters();
    let mut summary = CommitSummary {
        // Retain the legacy empty-line sentinel; finish_line supplies its terminal byte site.
        text_range: usize::MAX..0,
        last_item_kind: LayoutItemKind::TextRun,
        committed_text_run: false,
        needs_reorder: false,
    };
    for item in items {
        match item {
            CommitItem::InlineBox {
                box_index,
                bidi_level,
            } => {
                assert!(
                    box_index < layout.data.inline_boxes.len(),
                    "invalid original box ordinal"
                );
                output.push(LineItemData {
                    kind: LayoutItemKind::InlineBox,
                    index: box_index,
                    bidi_level,
                    shaped_cluster_range: 0..0,
                    text_range: 0..0,
                });
                summary.last_item_kind = LayoutItemKind::InlineBox;
            }
            CommitItem::TextRun {
                run_index,
                shaped_cluster_range,
            } => {
                let shaped_run = &shaped_text.runs()[run_index];
                let original_range = &shaped_run.shaped_clusters_range;
                assert!(
                    original_range.start <= shaped_cluster_range.start
                        && shaped_cluster_range.start <= shaped_cluster_range.end
                        && shaped_cluster_range.end <= original_range.end,
                    "committed slice is outside its original run"
                );
                let is_atom_boundary = |index: u32| {
                    index == original_range.start
                        || index == original_range.end
                        || shaped_clusters[index as usize].is_grapheme_start()
                };
                assert!(
                    is_atom_boundary(shaped_cluster_range.start)
                        && is_atom_boundary(shaped_cluster_range.end),
                    "committed slice endpoints must be whole-atom boundaries"
                );
                let slice = shaped_text.run_slice(run_index as u32);
                let text_range = if shaped_cluster_range.is_empty() {
                    let text_pos = if shaped_cluster_range.start == original_range.end {
                        shaped_run.range.byte_range.end
                    } else {
                        let char_pos = shaped_clusters[shaped_cluster_range.start as usize]
                            .chars_range()
                            .start;
                        slice.text_byte_at(char_pos)
                    };
                    text_pos..text_pos
                } else {
                    let char_range = shaped_clusters[shaped_cluster_range.start as usize]
                        .chars_range()
                        .start
                        ..shaped_clusters[shaped_cluster_range.end as usize - 1]
                            .chars_range()
                            .end;
                    slice.text_byte_range(char_range)
                };
                summary.text_range.start = summary.text_range.start.min(text_range.start);
                summary.text_range.end = summary.text_range.end.max(text_range.end);
                summary.needs_reorder |= shaped_run.bidi_level != BidiLevel::new(0);
                summary.last_item_kind = LayoutItemKind::TextRun;
                summary.committed_text_run = true;
                output.push(LineItemData {
                    kind: LayoutItemKind::TextRun,
                    index: run_index,
                    bidi_level: shaped_run.bidi_level,
                    shaped_cluster_range,
                    text_range,
                });
            }
        }
    }
    summary
}

/// Reorder items within line according to the bidi levels of the items
pub(crate) fn reorder_line_items(runs: &mut [LineItemData]) {
    let run_count = runs.len();

    // Find the max level and the min *odd* level
    let mut max_level = 0;
    let mut lowest_odd_level = 255;
    for run in runs.iter() {
        let level = run.bidi_level;
        let is_odd = level.to_u8() & 1 != 0;

        // Update max level
        if level.to_u8() > max_level {
            max_level = level.to_u8();
        }

        // Update min odd level
        if is_odd && level.to_u8() < lowest_odd_level {
            lowest_odd_level = level.to_u8();
        }
    }

    // Iterate over bidi levels
    for level in (lowest_odd_level..=max_level).rev() {
        // Iterate over text runs
        let mut i = 0;
        while i < run_count {
            if runs[i].bidi_level.to_u8() >= level {
                let mut end = i + 1;
                while end < run_count && runs[end].bidi_level.to_u8() >= level {
                    end += 1;
                }

                runs[i..end].reverse();
                i = end;
            }
            i += 1;
        }
    }
}
