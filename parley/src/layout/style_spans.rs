// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Read-only ownership bounds for ordinary spans, separate from shaping and metrics.

use super::flow::{ContentItem, LineFlow};
use super::inline_edges::gap_extents;
use super::line::Line;
use super::whitespace::whitespace_hangs;
use crate::{InlineBoxKind, WhiteSpaceCollapse, style::Brush};
use parley_engine::Atom;
use parley_engine::shape::Whitespace;

pub(super) fn bounds<B: Brush>(line: Line<'_, B>, style_index: usize) -> Option<(f32, f32)> {
    let style = u16::try_from(style_index).ok()?;
    let span = line
        .layout
        .data
        .style_spans
        .iter()
        .find(|span| span.style == style)?;
    let mut bounds: Option<(f32, f32)> = None;
    let mut include = |start: f32, end: f32| {
        let min = start.min(end);
        let max = start.max(end);
        bounds = Some(bounds.map_or((min, max), |(old_min, old_max)| {
            (old_min.min(min), old_max.max(max))
        }));
    };
    let metrics = line.metrics();
    let start = metrics.inline_min_coord + metrics.offset;
    let (visible_min, visible_max) = if line.layout.is_rtl() {
        (start + metrics.hanging_advance, start + metrics.advance)
    } else {
        (start, start + metrics.advance - metrics.hanging_advance)
    };
    // A forced break right after the span's own nonzero closing edge sits past that edge
    // (and past the span's own margin); its zero-width newline does not extend the border box.
    let closes_here = line
        .layout
        .data
        .inline_edges
        .get(style_index)
        .is_some_and(|edges| edges.end() != 0.)
        && line.inline_edge_sides(style_index).1;
    for item in LineFlow::new(line.clone()).content() {
        match item {
            ContentItem::Atom(item) => {
                let run = item.run;
                let atom = item.atom;
                let x = item.x;
                let advance = item.advance;
                let chars = atom.characters();
                let first = chars.first()?.text_byte_start as usize;
                let end = chars.last()?.text_byte_range().end;
                let edge = run.line_spacing().edge(&atom).copied();
                let mut owns_edge = false;
                if let Some(edge) = &edge {
                    owns_edge = gap_extents(
                        &line.layout.data,
                        edge,
                        run.is_rtl(),
                        x,
                        advance,
                        style,
                        true,
                        &mut include,
                    );
                }
                let (x, advance) = match &edge {
                    // The atom's own extent, without the box edges around it.
                    Some(edge) => (x + edge.before, advance - edge.before - edge.after),
                    None => (x, advance),
                };
                if closes_here
                    && advance == 0.
                    && chars.iter().all(|c| c.whitespace == Whitespace::Newline)
                {
                    continue;
                }
                let (collapsed, all_collapse) = collapsed_suffix(run, &atom);
                let (mut left, mut right) = (x, x + advance);
                if all_collapse {
                    left = left.max(visible_min).min(visible_max);
                    right = right.max(visible_min).min(visible_max);
                } else if collapsed > 0. {
                    // Trim only the collapsing suffix. A preserved child or a non-whitespace
                    // prefix within the same atom still owns its full positioned extent.
                    if run.is_rtl() {
                        left = left.max(visible_min).min(x + collapsed);
                    } else {
                        right = right.min(visible_max).max(x + advance - collapsed);
                    }
                }
                if span.range.contains(&first) {
                    include(left, right);
                } else if !owns_edge
                    && !span.range.is_empty()
                    && span.range.start < end
                    && first < span.range.end
                {
                    // Continuation-only ownership has no caret fraction: the atom belongs
                    // in full to its first code unit's owner, and this span follows its end.
                    let end = if run.is_rtl() { left } else { right };
                    include(end, end);
                }
            }
            ContentItem::InlineBox(item) => {
                let inline_box = item.inline_box;
                if inline_box.kind == InlineBoxKind::InFlow {
                    if (span.boxes_before..span.boxes_after).contains(&item.box_index) {
                        include(item.x, item.x + item.advance);
                    }
                }
            }
        }
    }
    bounds.or_else(|| {
        line.empty_spans()
            .find(|(index, _)| *index == style_index)
            .map(|(_, x)| (x, x))
    })
}

/// Like whitespace hanging, but only a suffix processed by a collapsing policy can be
/// trimmed from ownership. Preserved descendants keep their extent even under a collapsing
/// ancestor; mixed-policy clusters are retained rather than clipping preserved characters.
fn collapsed_suffix<B: Brush>(run: super::run::Run<'_, B>, atom: &Atom<'_>) -> (f32, bool) {
    let slice = run.line_slice();
    let styles = &run.layout.data.styles;
    let mut advance = 0.;
    let mut all_collapse = true;
    for cluster in atom.shaped_clusters().iter().rev() {
        let collapses = slice
            .characters_in(cluster.chars_range())
            .iter()
            .all(|character| {
                let style = &styles[character.style_index as usize];
                matches!(
                    style.white_space_collapse,
                    WhiteSpaceCollapse::Collapse | WhiteSpaceCollapse::PreserveBreaks
                ) && whitespace_hangs(character.whitespace, style)
            });
        if !collapses {
            all_collapse = false;
            break;
        }
        advance += cluster.advance;
    }
    let mut gaps = run.line_spacing().gaps(atom);
    // Inline box edges are not part of the collapsed text.
    gaps.before -= gaps.edge_before;
    gaps.after -= gaps.edge_after;
    if advance != 0. || all_collapse {
        advance += if run.is_rtl() {
            gaps.before
        } else {
            gaps.after
        };
    }
    if all_collapse {
        advance += if run.is_rtl() {
            gaps.after
        } else {
            gaps.before
        };
    }
    (advance, all_collapse)
}
