// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Horizontal inline box edges (margin, border and padding) of ordinary spans.
//!
//! Edge values are per style and can be refreshed without reshaping. Their attachment to
//! shaped atoms is fixed when the layout is built: an opening edge travels with the atom that
//! follows it and a closing edge with the atom that precedes it, so that both line breakers,
//! the content widths and every positioned consumer see one advance through the existing
//! [`Gaps`](super::spacing::Gaps) mechanism. An edge inside a whole (for example a cached
//! ligature) atom follows that atom's end.
//!
//! Edges with no atom to attach to (an inline formatting context without text, or an edge
//! enclosed by atomic inline boxes on both sides) are retained as unplaced; callers can query
//! [`Layout::has_unplaced_inline_edges`](crate::Layout::has_unplaced_inline_edges).

use super::data::{LayoutData, LayoutItemKind};
use super::line::Line;
use crate::resolve::tree::SpanEdgeKind;
use crate::style::Brush;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ops::Range;
use parley_engine::shape::Whitespace;

/// Used horizontal inline box edges of an ordinary span, in layout units and logical order.
///
/// `start_*` values belong to the span's first fragment and `end_*` values to its last one
/// (`box-decoration-break: slice`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct InlineEdges {
    /// Inline-start margin.
    pub start_margin: f32,
    /// Inline-start border width.
    pub start_border: f32,
    /// Inline-start padding.
    pub start_padding: f32,
    /// Inline-end padding.
    pub end_padding: f32,
    /// Inline-end border width.
    pub end_border: f32,
    /// Inline-end margin.
    pub end_margin: f32,
}

impl InlineEdges {
    /// All edges zero.
    pub const ZERO: Self = Self {
        start_margin: 0.,
        start_border: 0.,
        start_padding: 0.,
        end_padding: 0.,
        end_border: 0.,
        end_margin: 0.,
    };

    /// The advance reserved before the span's content.
    #[inline]
    pub fn start(&self) -> f32 {
        self.start_margin + self.start_border + self.start_padding
    }

    /// The advance reserved after the span's content.
    #[inline]
    pub fn end(&self) -> f32 {
        self.end_padding + self.end_border + self.end_margin
    }

    /// Whether every edge is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }
}

/// An authored span edge attached to an atom.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct EdgeEvent {
    pub(crate) style: u16,
    pub(crate) open: bool,
}

/// The edge events attached to one atom, in authored order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EdgeSite {
    /// Index of the run in [`LayoutData::runs`].
    pub(crate) run: u32,
    /// The atom's shaped clusters.
    pub(crate) clusters: Range<u32>,
    /// Events before the atom's logical start, into [`LayoutData::edge_events`].
    pub(crate) lead: Range<u32>,
    /// Events after the atom's logical end, into [`LayoutData::edge_events`].
    pub(crate) trail: Range<u32>,
}

/// A nonzero edge advance attached to an atom.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct EdgeGap {
    /// The atom's first shaped cluster.
    pub(crate) cluster: u32,
    /// One past the atom's last shaped cluster.
    pub(crate) end_cluster: u32,
    /// Index of the atom's [`EdgeSite`].
    pub(crate) site: u32,
    /// Advance at the atom's logical start.
    pub(crate) lead: f32,
    /// Advance at the atom's logical end.
    pub(crate) trail: f32,
    /// Advance visually left of the atom.
    pub(crate) before: f32,
    /// Advance visually right of the atom.
    pub(crate) after: f32,
}

struct AtomInfo {
    bytes: Range<usize>,
    clusters: Range<u32>,
    run: u32,
    newline: bool,
}

#[derive(Default)]
struct PendingSite {
    run: u32,
    clusters: Range<u32>,
    lead: Vec<EdgeEvent>,
    trail: Vec<EdgeEvent>,
}

impl<B: Brush> LayoutData<B> {
    /// Attach the authored span edges to atoms. Called once the layout has been shaped and the
    /// tracked span edges are known; resets all edge values to zero.
    pub(crate) fn attach_inline_edges(&mut self) {
        self.inline_edges.clear();
        self.edge_sites.clear();
        self.edge_events.clear();
        self.unplaced_edge_events.clear();
        self.edge_gaps.clear();
        for run in &mut self.runs {
            run.edge_gaps = 0..0;
        }
        if self.span_edges.is_empty() {
            return;
        }
        self.inline_edges
            .resize(self.styles.len(), InlineEdges::ZERO);

        let mut atoms: Vec<AtomInfo> = Vec::new();
        // Without text, the only shaped atom is the analysis placeholder.
        let items: &[_] = if self.text_len == 0 { &[] } else { &self.items };
        for item in items {
            if item.kind != LayoutItemKind::TextRun {
                continue;
            }
            let slice = self.shaped_text.run_slice(item.index as u32);
            for atom in slice.atoms_start() {
                let characters = atom.characters();
                let (Some(first), Some(last)) = (characters.first(), characters.last()) else {
                    continue;
                };
                atoms.push(AtomInfo {
                    bytes: first.text_byte_start as usize..last.text_byte_range().end,
                    clusters: atom.shaped_clusters_range(),
                    run: item.index as u32,
                    newline: first.whitespace == Whitespace::Newline,
                });
            }
        }
        atoms.sort_by_key(|atom| atom.bytes.start);

        let mut sites: BTreeMap<u32, PendingSite> = BTreeMap::new();
        let site_of = |sites: &mut BTreeMap<u32, PendingSite>, atom: &AtomInfo| -> u32 {
            let key = atom.clusters.start;
            sites.entry(key).or_insert_with(|| PendingSite {
                run: atom.run,
                clusters: atom.clusters.clone(),
                ..PendingSite::default()
            });
            key
        };

        // Group the authored stream by byte position and by the inline boxes between them.
        let edges = &self.span_edges;
        let mut index = 0;
        while index < edges.len() {
            let byte = edges[index].index;
            let boxes_at = |ordinal: usize| {
                self.inline_boxes[..ordinal.min(self.inline_boxes.len())]
                    .iter()
                    .filter(|b| b.inline_box.index == byte)
                    .count()
            };
            let group_key = boxes_at(edges[index].boxes_before);
            let mut end = index + 1;
            while end < edges.len()
                && edges[end].index == byte
                && boxes_at(edges[end].boxes_before) == group_key
            {
                end += 1;
            }
            let group = &edges[index..end];
            let events: Vec<EdgeEvent> = group
                .iter()
                .map(|edge| EdgeEvent {
                    style: edge.style,
                    open: edge.kind == SpanEdgeKind::Open,
                })
                .collect();

            let boxes_here = self
                .inline_boxes
                .iter()
                .filter(|b| b.inline_box.index == byte)
                .count();
            let interior = atoms
                .iter()
                .find(|atom| atom.bytes.start < byte && byte < atom.bytes.end);
            if let Some(atom) = interior {
                let key = site_of(&mut sites, atom);
                sites.get_mut(&key).unwrap().trail.extend(events);
            } else {
                let before = (group_key == 0)
                    .then(|| atoms.iter().find(|atom| atom.bytes.end == byte))
                    .flatten();
                // Edges that only close right after a forced break end the line the break ends:
                // they stay with the content before the break.
                let upstream = match before {
                    Some(newline) if newline.newline => events
                        .iter()
                        .all(|event| !event.open)
                        .then(|| {
                            atoms
                                .iter()
                                .find(|atom| atom.bytes.end == newline.bytes.start)
                        })
                        .flatten()
                        .filter(|atom| !atom.newline),
                    other => other,
                };
                let downstream = (group_key == boxes_here)
                    .then(|| atoms.iter().find(|atom| atom.bytes.start == byte))
                    .flatten()
                    .filter(|atom| !atom.newline);
                // Closing edges stay with the preceding content and opening edges travel with the
                // following content; edges of an empty span stay upstream like its anchor.
                let split = events
                    .iter()
                    .rposition(|event| !event.open)
                    .map_or(0, |last_close| last_close + 1);
                match (upstream, downstream) {
                    (Some(up), Some(down)) => {
                        let key = site_of(&mut sites, up);
                        sites
                            .get_mut(&key)
                            .unwrap()
                            .trail
                            .extend_from_slice(&events[..split]);
                        let key = site_of(&mut sites, down);
                        sites
                            .get_mut(&key)
                            .unwrap()
                            .lead
                            .extend_from_slice(&events[split..]);
                    }
                    (Some(up), None) => {
                        let key = site_of(&mut sites, up);
                        sites.get_mut(&key).unwrap().trail.extend(events);
                    }
                    (None, Some(down)) => {
                        let key = site_of(&mut sites, down);
                        sites.get_mut(&key).unwrap().lead.extend(events);
                    }
                    (None, None) => self.unplaced_edge_events.extend(events),
                }
            }
            index = end;
        }

        for (_, site) in sites {
            let lead_start = self.edge_events.len() as u32;
            self.edge_events.extend(site.lead);
            let trail_start = self.edge_events.len() as u32;
            self.edge_events.extend(site.trail);
            let end = self.edge_events.len() as u32;
            self.edge_sites.push(EdgeSite {
                run: site.run,
                clusters: site.clusters,
                lead: lead_start..trail_start,
                trail: trail_start..end,
            });
        }
    }

    /// The advance of one edge event under the current edge values.
    #[inline]
    pub(crate) fn edge_event_width(&self, event: EdgeEvent) -> f32 {
        let edges = self
            .inline_edges
            .get(usize::from(event.style))
            .copied()
            .unwrap_or_default();
        if event.open {
            edges.start()
        } else {
            edges.end()
        }
    }

    fn edge_events_width(&self, events: &Range<u32>) -> f32 {
        let mut width = 0.;
        for event in &self.edge_events[events.start as usize..events.end as usize] {
            width += self.edge_event_width(*event);
        }
        width
    }

    /// Recompute the nonzero per-atom edge advances from the current edge values.
    pub(crate) fn refresh_edge_gaps(&mut self) {
        self.edge_gaps.clear();
        for run in &mut self.runs {
            run.edge_gaps = 0..0;
        }
        let mut gaps: Vec<(u32, EdgeGap)> = Vec::new();
        for (index, site) in self.edge_sites.iter().enumerate() {
            let lead = self.edge_events_width(&site.lead);
            let trail = self.edge_events_width(&site.trail);
            if lead == 0. && trail == 0. {
                continue;
            }
            let rtl = self.shaped_text.runs()[site.run as usize]
                .bidi_level
                .is_rtl();
            let (before, after) = if rtl { (trail, lead) } else { (lead, trail) };
            gaps.push((
                site.run,
                EdgeGap {
                    cluster: site.clusters.start,
                    end_cluster: site.clusters.end,
                    site: index as u32,
                    lead,
                    trail,
                    before,
                    after,
                },
            ));
        }
        gaps.sort_by_key(|(run, gap)| (*run, gap.cluster));
        let mut start = 0;
        while start < gaps.len() {
            let run = gaps[start].0;
            let mut end = start + 1;
            while end < gaps.len() && gaps[end].0 == run {
                end += 1;
            }
            let base = self.edge_gaps.len() as u32;
            self.edge_gaps
                .extend(gaps[start..end].iter().map(|(_, gap)| *gap));
            self.runs[run as usize].edge_gaps = base..self.edge_gaps.len() as u32;
            start = end;
        }
    }

    /// The nonzero edge advances of run `run`'s atoms.
    #[inline]
    pub(crate) fn run_edge_gaps(&self, run: usize) -> &[EdgeGap] {
        let range = self.runs[run].edge_gaps.clone();
        &self.edge_gaps[range.start as usize..range.end as usize]
    }

    /// Whether `style` is `ancestor` or one of its descendants.
    pub(crate) fn style_within(&self, mut style: u16, ancestor: u16) -> bool {
        loop {
            if style == ancestor {
                return true;
            }
            let Some(parent) = self.styles.get(usize::from(style)).map(|s| s.parent) else {
                return false;
            };
            if parent == style {
                return false;
            }
            style = parent;
        }
    }
}

/// The horizontal extent covered by `target`'s box edges within an atom's edge gaps, and
/// whether `target` has an event there.
///
/// `start` is the visual left of the atom's advance (including its edges) and `content` its
/// visual content extent. Extents exclude `target`'s own margins and include the full advance
/// of its descendants' edges.
pub(crate) fn gap_extents<B: Brush>(
    data: &LayoutData<B>,
    gap: &EdgeGap,
    rtl: bool,
    x: f32,
    advance: f32,
    target: u16,
    descendants: bool,
    mut include: impl FnMut(f32, f32),
) -> bool {
    let site = &data.edge_sites[gap.site as usize];
    let mut found = false;
    // Logical start and end positions of the lead and trail gaps.
    let (lead_origin, trail_origin, sign) = if rtl {
        (x + advance, x + gap.before, -1.)
    } else {
        (x, x + advance - gap.after, 1.)
    };
    for (events, origin) in [(&site.lead, lead_origin), (&site.trail, trail_origin)] {
        let mut offset = 0.;
        for event in &data.edge_events[events.start as usize..events.end as usize] {
            let width = data.edge_event_width(*event);
            let (from, to) = if event.style == target {
                found = true;
                let edges = data.inline_edges[usize::from(target)];
                if event.open {
                    (offset + edges.start_margin, offset + width)
                } else {
                    (offset, offset + edges.end_padding + edges.end_border)
                }
            } else if descendants && data.style_within(event.style, target) {
                (offset, offset + width)
            } else {
                offset += width;
                continue;
            };
            let a = origin + sign * from;
            let b = origin + sign * to;
            include(a.min(b), a.max(b));
            offset += width;
        }
    }
    found
}

/// Visual edge insets of a range of shaped clusters within a run (left, right).
pub(crate) fn cluster_range_insets<B: Brush>(
    data: &LayoutData<B>,
    run: usize,
    clusters: Range<u32>,
    rtl: bool,
) -> (f32, f32) {
    let gaps = data.run_edge_gaps(run);
    if gaps.is_empty() || clusters.is_empty() {
        return (0., 0.);
    }
    let first = gaps
        .binary_search_by_key(&clusters.start, |gap| gap.cluster)
        .ok()
        .map(|index| gaps[index]);
    let last = gaps
        .iter()
        .find(|gap| gap.end_cluster == clusters.end)
        .copied();
    if rtl {
        (last.map_or(0., |g| g.before), first.map_or(0., |g| g.after))
    } else {
        (first.map_or(0., |g| g.before), last.map_or(0., |g| g.after))
    }
}

impl<B: Brush> Line<'_, B> {
    /// Whether any atom on this line carries a nonzero inline box edge advance.
    pub(crate) fn has_edge_gaps(&self) -> bool {
        let data = &self.layout.data;
        !data.edge_gaps.is_empty()
            && data.line_items[self.data.item_range.clone()]
                .iter()
                .any(|item| {
                    item.kind == LayoutItemKind::TextRun
                        && !data.run_edge_gaps(item.index).is_empty()
                })
    }
}

/// The extent of `style`'s box edges on `line`, from the atoms carrying its edge events.
///
/// Returns `None` if no atom on the line with a nonzero edge advance carries an event of
/// `style`. A zero-edge style between nonzero edges gets a point at its authored position.
pub(crate) fn edge_event_bounds<B: Brush>(line: &Line<'_, B>, style: u16) -> Option<(f32, f32)> {
    if !line.has_edge_gaps() {
        return None;
    }
    let mut bounds: Option<(f32, f32)> = None;
    for item in super::flow::LineFlow::new(line.clone()).content() {
        let super::flow::ContentItem::Atom(item) = item else {
            continue;
        };
        let Some(edge) = item.run.line_spacing().edge(&item.atom).copied() else {
            continue;
        };
        gap_extents(
            &line.layout.data,
            &edge,
            item.run.is_rtl(),
            item.x,
            item.advance,
            style,
            true,
            |min, max| {
                bounds = Some(bounds.map_or((min, max), |(a, b)| (a.min(min), b.max(max))));
            },
        );
    }
    bounds
}

impl<B: Brush> Line<'_, B> {
    /// The horizontal extents of `style_index`'s own opening and closing border and padding on
    /// this line, in layout units, without its margins, content or descendants. Empty unless
    /// the style has nonzero inline box edges here (see
    /// [`Layout::update_inline_edges`](crate::Layout::update_inline_edges)).
    pub fn inline_edge_extents(&self, style_index: usize) -> Vec<(f32, f32)> {
        let mut extents = Vec::new();
        let Ok(style) = u16::try_from(style_index) else {
            return extents;
        };
        if !self.has_edge_gaps() {
            return extents;
        }
        for item in super::flow::LineFlow::new(self.clone()).content() {
            let super::flow::ContentItem::Atom(item) = item else {
                continue;
            };
            let Some(edge) = item.run.line_spacing().edge(&item.atom).copied() else {
                continue;
            };
            gap_extents(
                &self.layout.data,
                &edge,
                item.run.is_rtl(),
                item.x,
                item.advance,
                style,
                false,
                |min, max| {
                    if max > min {
                        extents.push((min, max));
                    }
                },
            );
        }
        extents
    }

    /// Whether this line carries `style_index`'s opening and closing box edges (logical start,
    /// end), among atoms with nonzero inline box edge advances. With `box-decoration-break:
    /// slice`, a span's start border is drawn only on the fragment carrying its opening edge.
    pub fn inline_edge_sides(&self, style_index: usize) -> (bool, bool) {
        let mut sides = (false, false);
        let Ok(style) = u16::try_from(style_index) else {
            return sides;
        };
        if !self.has_edge_gaps() {
            return sides;
        }
        let data = &self.layout.data;
        for item in super::flow::LineFlow::new(self.clone()).content() {
            let super::flow::ContentItem::Atom(item) = item else {
                continue;
            };
            let Some(edge) = item.run.line_spacing().edge(&item.atom) else {
                continue;
            };
            let site = &data.edge_sites[edge.site as usize];
            for event in &data.edge_events[site.lead.start as usize..site.trail.end as usize] {
                if event.style == style {
                    if event.open {
                        sides.0 = true;
                    } else {
                        sides.1 = true;
                    }
                }
            }
        }
        sides
    }
}
