// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Public traversal results retain the independent pre-extraction positioning policies.

use alloc::vec::Vec;
use core::ops::Range;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::editing::{Cursor, Selection};
use crate::layout::LineItem;
use crate::{
    Affinity, BoundingBox, Cluster, ClusterPath, ClusterSide, FontFamily, InlineBox, InlineBoxKind,
    Layout, LayoutContext, Line, LineHeight, PositionedLayoutItem, TextStyle, VerticalAlign,
    WhiteSpaceCollapse,
};

fn fixture(tracked: bool) -> (Layout<ColorBrush>, alloc::string::String) {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let style = TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size: 17.125,
        line_height: LineHeight::Absolute(30.),
        letter_spacing: 0.3,
        word_spacing: 0.7,
        white_space_collapse: WhiteSpaceCollapse::Preserve,
        ..TextStyle::default()
    };
    let mut b = lcx.tree_builder(&mut fcx, 1., false, &style);
    b.set_style_span_tracking(tracked);
    b.push_text("ab");
    b.push_style_span(TextStyle {
        brush: ColorBrush::new(peniko::Color::from_rgba8(180, 30, 20, 255)),
        ..style.clone()
    });
    b.push_text("cd");
    b.pop_style_span();
    b.push_text(" e");
    for (id, width, kind) in [
        (10, 7.3, InlineBoxKind::InFlow),
        (20, 51.7, InlineBoxKind::OutOfFlow),
        (30, 61.2, InlineBoxKind::CustomOutOfFlow),
    ] {
        b.push_inline_box(InlineBox {
            id,
            width,
            kind,
            index: 0,
            height: 12.375,
            baseline: Some(9.125),
            vertical_align: VerticalAlign::BASELINE,
        });
    }
    b.push_style_span(TextStyle {
        font_size: 19.375,
        letter_spacing: 0.35,
        ..style
    });
    b.push_text("fg hi\n");
    b.pop_style_span();
    let (mut layout, text) = b.build();
    layout.break_all_lines(None);
    assert_eq!(layout.len(), 2, "preserved terminal newline adds a line");
    (layout, text)
}

const ORIGINS: [(f32, f32); 4] = [
    (0.1, 0.2),
    (4096.125, 0.3),
    (16384.25, 4096.7),
    (1_048_576.125, 0.3),
];

fn set_origin(layout: &mut Layout<ColorBrush>, origin: (f32, f32)) {
    for line in &mut layout.data.lines {
        line.metrics.inline_min_coord = origin.0;
        line.metrics.offset = origin.1;
    }
}

#[derive(Debug, PartialEq)]
enum Paint {
    Glyphs(
        usize,
        u16,
        Range<usize>,
        u32,
        u32,
        u32,
        Vec<(u32, u32, u32, u32)>,
    ),
    Box(u64, InlineBoxKind, u32, u32, u32, u32, Option<u32>),
}

fn actual_paint(line: Line<'_, ColorBrush>) -> Vec<Paint> {
    line.items()
        .map(|item| match item {
            PositionedLayoutItem::GlyphRun(g) => Paint::Glyphs(
                g.run().index(),
                g.style_index(),
                g.text_range(),
                g.offset().to_bits(),
                g.baseline().to_bits(),
                g.advance().to_bits(),
                g.positioned_glyphs()
                    .map(|g| (g.id, g.x.to_bits(), g.y.to_bits(), g.advance.to_bits()))
                    .collect(),
            ),
            PositionedLayoutItem::InlineBox(b) => Paint::Box(
                b.id,
                b.kind,
                b.x.to_bits(),
                b.y.to_bits(),
                b.width.to_bits(),
                b.height.to_bits(),
                b.baseline.map(f32::to_bits),
            ),
        })
        .collect()
}

// Original GlyphRunIter: accumulate per-style atom sums into a relative f32
// cursor, then add inline_min_coord and alignment separately at each yield.
// This deliberately does not use any flow cursor or AtomIter.
fn legacy_paint(line: Line<'_, ColorBrush>) -> Vec<Paint> {
    let mut output = Vec::new();
    let mut relative = 0_f32;
    for index in 0..line.len() {
        match line.item(index).unwrap() {
            LineItem::InlineBox(b) => {
                let box_index =
                    line.layout.data.line_items[line.data.item_range.start + index].index;
                output.push(Paint::Box(
                    b.id,
                    b.kind,
                    (relative + line.metrics().inline_min_coord + line.metrics().offset).to_bits(),
                    line.inline_box_top(box_index).to_bits(),
                    b.width.to_bits(),
                    b.height.to_bits(),
                    b.baseline.map(f32::to_bits),
                ));
                if b.kind == InlineBoxKind::InFlow {
                    relative += b.width;
                }
            }
            LineItem::Run(run) => {
                let mut atoms = run.line_slice().atoms_start().peekable();
                assert!(!run.is_rtl(), "legacy arithmetic reduction uses LTR atoms");
                while atoms.peek().is_some() {
                    let mut group: Option<(u16, Range<u32>)> = None;
                    let mut advance = 0_f32;
                    while let Some(atom) = atoms.peek() {
                        let count: usize = atom
                            .shaped_clusters()
                            .iter()
                            .map(|c| usize::from(c.glyph_len()))
                            .sum();
                        if count != 0 {
                            let style = atom.characters()[0].style_index;
                            let range = atom.shaped_clusters_range();
                            if let Some((previous, bounds)) = &mut group {
                                if *previous != style {
                                    break;
                                }
                                bounds.start = bounds.start.min(range.start);
                                bounds.end = bounds.end.max(range.end);
                            } else {
                                group = Some((style, range));
                            }
                            let mut measured = atom.advance();
                            if !run.line_spacing().is_zero() {
                                measured += run.line_spacing().gaps(atom).total();
                            }
                            advance += measured;
                        }
                        atoms.next();
                    }
                    if let Some((style, range)) = group {
                        let offset =
                            relative + line.metrics().inline_min_coord + line.metrics().offset;
                        let baseline = line.style_baseline(style);
                        let mut glyph_offset = offset;
                        let glyphs = run
                            .glyphs_in(range.clone())
                            .map(|g| {
                                let positioned = (
                                    g.id,
                                    (g.x + glyph_offset).to_bits(),
                                    (g.y + baseline).to_bits(),
                                    g.advance.to_bits(),
                                );
                                glyph_offset += g.advance;
                                positioned
                            })
                            .collect();
                        let slice = run.full_slice().narrow(range);
                        output.push(Paint::Glyphs(
                            index,
                            style,
                            slice.text_byte_range(slice.char_range()),
                            offset.to_bits(),
                            baseline.to_bits(),
                            advance.to_bits(),
                            glyphs,
                        ));
                        relative += advance;
                    }
                }
            }
        }
    }
    output
}

fn legacy_visual_offset(cluster: Cluster<'_, ColorBrush>) -> f32 {
    let line = cluster.line();
    let mut x = line.metrics().offset;
    for index in 0..=cluster.run().index() {
        match line.item(index).unwrap() {
            LineItem::InlineBox(b) => x += b.width,
            LineItem::Run(run) if index != cluster.run().index() => x += run.advance(),
            LineItem::Run(run) => {
                for preceding in run.visual_clusters() {
                    if preceding.path() == cluster.path() {
                        break;
                    }
                    x += preceding.advance();
                }
            }
        }
    }
    x
}

// Original point lookup starts from a combined f32 origin and reserves all box
// kinds. It skips complete runs before descending into a candidate run.
fn legacy_point(
    layout: &Layout<ColorBrush>,
    x: f32,
    y: f32,
    exact: bool,
) -> Option<(ClusterPath, ClusterSide)> {
    let (line_index, line) = layout.line_for_offset(y)?;
    let mut offset = line.metrics().offset + line.metrics().inline_min_coord;
    for index in 0..line.len() {
        match line.item(index).unwrap() {
            LineItem::InlineBox(b) => offset += b.width,
            LineItem::Run(run) => {
                let last_run = index == line.len().saturating_sub(1);
                if x > offset + run.advance() && (exact || !last_run) {
                    offset += run.advance();
                    continue;
                }
                let clusters = run.visual_clusters().collect::<Vec<_>>();
                for (i, cluster) in clusters.iter().enumerate() {
                    let last_cluster = last_run && i + 1 == clusters.len();
                    let advance = cluster.advance();
                    let edge = offset;
                    offset += advance;
                    if x > offset && (exact || !last_cluster) {
                        continue;
                    }
                    if x < edge && exact {
                        continue;
                    }
                    return Some((
                        cluster.path(),
                        if x <= edge + advance * 0.5 {
                            ClusterSide::Left
                        } else {
                            ClusterSide::Right
                        },
                    ));
                }
            }
        }
    }
    if y <= 0. && !exact {
        let cluster = layout.get(line_index)?.item(0)?.run()?.clusters().next()?;
        Some((cluster.path(), ClusterSide::Left))
    } else {
        None
    }
}

fn rect_bits(rect: BoundingBox) -> [u64; 4] {
    [
        rect.x0.to_bits(),
        rect.y0.to_bits(),
        rect.x1.to_bits(),
        rect.y1.to_bits(),
    ]
}

fn legacy_cursor_rect(cluster: Cluster<'_, ColorBrush>, at_end: bool, width: f32) -> [u64; 4] {
    let mut x = legacy_visual_offset(cluster);
    if at_end {
        x += cluster.advance();
    }
    let line = cluster.line();
    let m = line.metrics();
    rect_bits(BoundingBox::new(
        f64::from(x),
        f64::from(m.block_min_coord.min(m.content_block_min_coord)),
        f64::from(x + width),
        f64::from(m.block_max_coord.max(m.content_block_max_coord)),
    ))
}

#[test]
fn public_paint_preserves_style_groups_relative_origin_and_inflow_only_box_advance() {
    let (mut tracked, _) = fixture(true);
    let (mut untracked, _) = fixture(false);
    let first = tracked.get(0).unwrap();
    let groups = first
        .items()
        .filter(|item| matches!(item, PositionedLayoutItem::GlyphRun(_)))
        .count();
    assert!(
        groups > first.runs().count(),
        "fixture must split a shaped run by paint style"
    );
    let mut distinguishes_combined_origin = false;
    for origin in ORIGINS {
        set_origin(&mut tracked, origin);
        set_origin(&mut untracked, origin);
        for (a, b) in tracked.lines().zip(untracked.lines()) {
            assert_eq!(actual_paint(a), legacy_paint(a));
            assert_eq!(
                actual_paint(a),
                actual_paint(b),
                "tracking is behavior preserving"
            );
            let mut relative = 0_f32;
            for item in actual_paint(a) {
                let (position, advance) = match item {
                    Paint::Glyphs(_, _, _, x, _, advance, _) => (x, f32::from_bits(advance)),
                    Paint::Box(_, kind, x, _, width, _, _) => (
                        x,
                        if kind == InlineBoxKind::InFlow {
                            f32::from_bits(width)
                        } else {
                            0.
                        },
                    ),
                };
                distinguishes_combined_origin |= position
                    != ((a.metrics().inline_min_coord + a.metrics().offset) + relative).to_bits();
                relative += advance;
            }
        }
        assert!(tracked.lines().last().unwrap().items().next().is_none());
    }
    assert!(
        distinguishes_combined_origin,
        "fixture rejects starting paint accumulation from a combined absolute origin"
    );
}

#[test]
fn public_cluster_lookup_preserves_all_box_widths_and_distinct_origin_policies() {
    let (mut layout, _) = fixture(true);
    let mut checked = 0;
    for origin in ORIGINS {
        set_origin(&mut layout, origin);
        let line = layout.get(0).unwrap();
        let y = line.metrics().baseline;
        let mut x = line.metrics().offset + line.metrics().inline_min_coord;
        let mut samples = alloc::vec![x - 100., x];
        for index in 0..line.len() {
            match line.item(index).unwrap() {
                LineItem::InlineBox(b) => {
                    samples.push(x + b.width * 0.5);
                    x += b.width;
                }
                LineItem::Run(run) => {
                    for c in run.visual_clusters() {
                        assert_eq!(
                            c.visual_offset().unwrap().to_bits(),
                            legacy_visual_offset(c).to_bits()
                        );
                        samples.extend([x, x + c.advance() * 0.25, x + c.advance() * 0.75]);
                        x += c.advance();
                        checked += 1;
                    }
                }
            }
        }
        samples.push(x + 100.);
        for sample in samples {
            for exact in [false, true] {
                let actual = if exact {
                    Cluster::from_point_exact(&layout, sample, y)
                } else {
                    Cluster::from_point(&layout, sample, y)
                };
                assert_eq!(
                    actual.map(|(c, side)| (c.path(), side)),
                    legacy_point(&layout, sample, y, exact)
                );
            }
        }
    }
    assert!(
        checked > 40,
        "bundled-font cluster lookup is actually exercised"
    );
}

#[test]
fn public_cursor_geometry_keeps_f32_accumulation_and_terminal_alignment_only_origin() {
    let (mut layout, text) = fixture(true);
    for origin in ORIGINS {
        set_origin(&mut layout, origin);
        for index in 0..text.len() {
            let cursor = Cursor::from_byte_index(&layout, index, Affinity::Downstream);
            let (cluster, at_end) = if index == 0 {
                (Cluster::from_byte_index(&layout, 0).unwrap(), false)
            } else {
                (Cluster::from_byte_index(&layout, index - 1).unwrap(), true)
            };
            for width in [0., 0.3, 1.125] {
                assert_eq!(
                    rect_bits(cursor.geometry(&layout, width)),
                    legacy_cursor_rect(cluster, at_end, width)
                );
            }
        }
        let end = Cursor::from_byte_index(&layout, text.len(), Affinity::Upstream);
        let last_line = layout.lines().last().unwrap();
        let m = last_line.metrics();
        for width in [0., 0.3, 1.125] {
            let expected = BoundingBox::new(
                f64::from(m.offset),
                f64::from(m.block_min_coord.min(m.content_block_min_coord)),
                f64::from(m.offset + width),
                f64::from(m.block_max_coord.max(m.content_block_max_coord)),
            );
            assert_eq!(rect_bits(end.geometry(&layout, width)), rect_bits(expected));
        }
    }
}

#[test]
fn public_selection_geometry_keeps_f64_origin_all_box_widths_and_newline_extension() {
    let (mut layout, text) = fixture(true);
    let mut detects_f32_origin = false;
    for origin in ORIGINS {
        set_origin(&mut layout, origin);
        let line = layout.get(0).unwrap();
        let m = line.metrics();
        let start = f64::from(m.offset) + f64::from(m.inline_min_coord);
        detects_f32_origin |= start.to_bits() != f64::from(m.offset + m.inline_min_coord).to_bits();
        // Original selection buffers all box widths until the next cluster,
        // and performs the cluster+box and current-x additions in f64.
        let mut end = start;
        let mut boxes = 0_f64;
        for index in 0..line.len() {
            match line.item(index).unwrap() {
                LineItem::InlineBox(b) => boxes += f64::from(b.width),
                LineItem::Run(run) => {
                    for cluster in run.visual_clusters() {
                        end += f64::from(cluster.advance()) + boxes;
                        boxes = 0.;
                    }
                }
            }
        }
        let newline = Cluster::from_byte_index(&layout, text.len() - 1).unwrap();
        assert!(newline.is_hard_line_break());
        let newline_run = newline.run();
        let font = newline_run.font_metrics();
        end += (f64::from(font.ascent) + f64::from(font.descent)) * 0.25;
        let expected = BoundingBox::new(
            start,
            f64::from(m.block_min_coord.min(m.content_block_min_coord)),
            end,
            f64::from(m.block_max_coord.max(m.content_block_max_coord)),
        );
        let selection = Selection::new(
            Cursor::from_byte_index(&layout, 0, Affinity::Downstream),
            Cursor::from_byte_index(&layout, text.len(), Affinity::Upstream),
        );
        let actual = selection
            .geometry(&layout)
            .into_iter()
            .map(|(rect, line)| (rect_bits(rect), line))
            .collect::<Vec<_>>();
        assert_eq!(actual, alloc::vec![(rect_bits(expected), 0)]);
    }
    assert!(
        detects_f32_origin,
        "fixture distinguishes separately widened selection origin"
    );
}
