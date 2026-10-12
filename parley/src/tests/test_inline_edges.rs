// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Horizontal inline box edges of tracked ordinary spans.

use alloc::vec;
use alloc::vec::Vec;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    FontContext, FontFamily, InlineEdges, Layout, LayoutContext, LineHeight, PositionedLayoutItem,
    TextStyle, TreeBuilder, WhiteSpaceCollapse,
};

fn style() -> TextStyle<'static, 'static, ColorBrush> {
    TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size: 20.,
        line_height: LineHeight::Absolute(30.),
        white_space_collapse: WhiteSpaceCollapse::Collapse,
        ..TextStyle::default()
    }
}

fn build(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<ColorBrush>,
    configure: impl FnOnce(&mut TreeBuilder<'_, ColorBrush>),
) -> Layout<ColorBrush> {
    let mut builder = lcx.tree_builder(fcx, 1., false, &style());
    builder.set_style_span_tracking(true);
    configure(&mut builder);
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    layout
}

/// `a<span>b</span>c`, with the span's shaping boundaries on both sides.
fn simple(builder: &mut TreeBuilder<'_, ColorBrush>) {
    builder.push_text("a");
    builder.push_style_span_with_shaping_boundaries(style(), true, true);
    builder.push_text("b");
    builder.pop_style_span();
    builder.push_text("c");
}

fn glyphs(layout: &Layout<ColorBrush>) -> Vec<(u32, f32, f32)> {
    layout
        .lines()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => Some(
                run.positioned_glyphs()
                    .map(|g| (g.id, g.advance, g.x))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "expected {expected}, got {actual}"
    );
}

const EDGES: InlineEdges = InlineEdges {
    start_margin: 1.,
    start_border: 2.,
    start_padding: 3.,
    end_padding: 4.,
    end_border: 5.,
    end_margin: 6.,
};

fn span_style(layout: &Layout<ColorBrush>, index: usize) -> usize {
    usize::from(layout.data.style_spans[index].style)
}

fn set(layout: &mut Layout<ColorBrush>, style: usize, edges: InlineEdges) {
    let mut all = vec![None; layout.styles().len()];
    all[style] = Some(edges);
    assert!(layout.update_inline_edges(&all));
}

#[test]
fn edges_shift_following_content_and_bound_border_box() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, simple);
    let before = glyphs(&layout);
    let widths = layout.calculate_content_widths();
    let advance = layout.lines().next().unwrap().metrics().advance;
    let span = span_style(&layout, 0);
    let bounds = layout
        .lines()
        .next()
        .unwrap()
        .style_span_bounds(span)
        .unwrap();

    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    let after = glyphs(&layout);
    assert_eq!(before.len(), 3);
    close(after[0].2, before[0].2);
    close(after[1].2, before[1].2 + 6.);
    close(after[2].2, before[2].2 + 21.);
    let line = layout.lines().next().unwrap();
    close(line.metrics().advance, advance + 21.);
    let edged = line.style_span_bounds(span).unwrap();
    // Border box: excludes the span's own margins.
    close(edged.0, bounds.0 + 1.);
    close(edged.1, bounds.1 + 6. + 4. + 5.);
    let edged_widths = layout.calculate_content_widths();
    close(edged_widths.max, widths.max + 21.);
    close(edged_widths.min, widths.min + 21.);
    assert!(!layout.has_unplaced_inline_edges());

    // Glyph run insets name the reserved advances.
    let insets: Vec<_> = layout
        .lines()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => Some(run.inline_edge_insets()),
            _ => None,
        })
        .collect();
    assert_eq!(insets, vec![(0., 0.), (6., 15.), (0., 0.)]);
}

#[test]
fn zero_edges_are_identical_and_refresh_is_reversible() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, simple);
    let before = glyphs(&layout);
    let span = span_style(&layout, 0);
    let bounds = layout.lines().next().unwrap().style_span_bounds(span);

    set(&mut layout, span, InlineEdges::ZERO);
    layout.break_all_lines(None);
    assert_eq!(glyphs(&layout), before);

    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    assert_ne!(glyphs(&layout), before);
    set(&mut layout, span, InlineEdges::ZERO);
    layout.break_all_lines(None);
    assert_eq!(glyphs(&layout), before);
    assert_eq!(
        layout.lines().next().unwrap().style_span_bounds(span),
        bounds
    );
}

#[test]
fn invalid_edges_are_rejected() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, simple);
    let span = span_style(&layout, 0);
    let mut negative = EDGES;
    negative.end_margin = -1.;
    let mut all = vec![None; layout.styles().len()];
    all[span] = Some(negative);
    assert!(!layout.update_inline_edges(&all));
    assert!(!layout.update_inline_edges(&[]));
    assert_eq!(layout.inline_edges(span), InlineEdges::ZERO);
}

#[test]
fn empty_span_edges_take_space() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_text("a");
        builder.push_style_span(style());
        builder.pop_style_span();
        builder.push_text("b");
    });
    let before = glyphs(&layout);
    let span = span_style(&layout, 0);
    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    let after = glyphs(&layout);
    close(after[1].2, before[1].2 + 21.);
    let a_end = before[0].2 + before[0].1;
    let bounds = layout
        .lines()
        .next()
        .unwrap()
        .style_span_bounds(span)
        .unwrap();
    close(bounds.0, a_end + 1.);
    close(bounds.1, a_end + 15.);
}

#[test]
fn wrapped_span_slices_edges() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let configure = |builder: &mut TreeBuilder<'_, ColorBrush>| {
        builder.push_text("aa ");
        builder.push_style_span_with_shaping_boundaries(style(), true, true);
        builder.push_text("bb cc");
        builder.pop_style_span();
        builder.push_text(" dd");
    };
    let mut layout = build(&mut fcx, &mut lcx, configure);
    let span = span_style(&layout, 0);
    set(&mut layout, span, EDGES);
    // Width of about one word plus edges per line.
    layout.break_all_lines(Some(60.));
    let lines: Vec<_> = layout.lines().collect();
    assert!(lines.len() >= 3, "{} lines", lines.len());
    let fragments: Vec<_> = lines
        .iter()
        .filter_map(|line| line.style_span_bounds(span).map(|b| (line.index, b)))
        .collect();
    assert!(fragments.len() >= 2);
    // The first fragment starts with the opening border and padding, the last ends with the
    // closing padding and border; neither edge is repeated.
    let first_line = &lines[fragments[0].0 as usize];
    let first_text = first_line
        .items()
        .find_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) if usize::from(run.style_index()) == span => {
                Some(run.offset() + run.inline_edge_insets().0)
            }
            _ => None,
        })
        .unwrap();
    close(fragments[0].1.0, first_text - 5.);
    let last_line = &lines[fragments.last().unwrap().0 as usize];
    let (last_offset, last_end) = last_line
        .items()
        .find_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) if usize::from(run.style_index()) == span => {
                Some((
                    run.offset() + run.inline_edge_insets().0,
                    run.offset() + run.advance() - run.inline_edge_insets().1,
                ))
            }
            _ => None,
        })
        .unwrap();
    close(fragments.last().unwrap().1.0, last_offset);
    close(fragments.last().unwrap().1.1, last_end + 9.);
}

#[test]
fn closing_edge_after_trailing_space_does_not_hang() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_text("a");
        builder.push_style_span(style());
        builder.push_text("b ");
        builder.pop_style_span();
    });
    let hanging = layout.lines().next().unwrap().metrics().hanging_advance;
    let advance = layout.lines().next().unwrap().metrics().advance;
    let span = span_style(&layout, 0);
    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    let metrics = *layout.lines().next().unwrap().metrics();
    close(metrics.hanging_advance, hanging);
    close(metrics.advance, advance + 21.);
}

#[test]
fn edges_without_text_are_unplaced() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let mut layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_style_span(style());
        builder.pop_style_span();
    });
    let span = span_style(&layout, 0);
    let advance = layout.lines().next().unwrap().metrics().advance;
    assert!(!layout.has_unplaced_inline_edges());
    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    assert!(layout.has_unplaced_inline_edges());
    close(layout.lines().next().unwrap().metrics().advance, advance);
}

#[test]
fn closing_edge_right_after_a_forced_break_ends_the_first_line() {
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    let preserve = || TextStyle {
        white_space_collapse: WhiteSpaceCollapse::Preserve,
        ..style()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &preserve());
    builder.set_style_span_tracking(true);
    builder.push_text("a");
    builder.push_style_span_with_shaping_boundaries(preserve(), true, true);
    builder.push_text("b\n");
    builder.pop_style_span();
    builder.push_text("c");
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let first = layout.lines().next().unwrap().metrics().advance;
    let second = layout.lines().nth(1).unwrap().metrics().advance;
    let span = span_style(&layout, 0);
    set(&mut layout, span, EDGES);
    layout.break_all_lines(None);
    let lines: Vec<_> = layout.lines().collect();
    assert_eq!(lines.len(), 2);
    // Both edges are on the first line; the line after the break starts without one.
    close(lines[0].metrics().advance, first + 21.);
    close(lines[1].metrics().advance, second);
    assert!(lines[1].style_span_bounds(span).is_none());
}

#[test]
fn forced_break_after_the_closing_edge_does_not_extend_the_border_box() {
    // `a<span>b\n</span>c` under pre, and `a<span>b<br></span>c` (a preserved newline in a
    // non-ordinary child span).
    let preserve = || TextStyle {
        white_space_collapse: WhiteSpaceCollapse::Preserve,
        ..style()
    };
    let shapes: [&dyn Fn(&mut TreeBuilder<'_, ColorBrush>); 2] = [
        &|builder| {
            builder.push_style_span_with_shaping_boundaries(preserve(), true, true);
            builder.push_text("b\n");
            builder.pop_style_span();
        },
        &|builder| {
            builder.push_style_span_with_shaping_boundaries(style(), true, true);
            builder.push_text("b");
            builder.push_style_modification_span(&[crate::StyleProperty::WhiteSpaceCollapse(
                WhiteSpaceCollapse::Preserve,
            )]);
            builder.push_text("\n");
            builder.pop_style_span();
            builder.pop_style_span();
        },
    ];
    for shape in shapes {
        let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
        let mut layout = build(&mut fcx, &mut lcx, |builder| {
            builder.push_text("a");
            shape(builder);
            builder.push_text("c");
        });
        let span = span_style(&layout, 0);
        let before = layout
            .lines()
            .next()
            .unwrap()
            .style_span_bounds(span)
            .unwrap();
        set(&mut layout, span, EDGES);
        layout.break_all_lines(None);
        assert_eq!(layout.len(), 2);
        let after = layout
            .lines()
            .next()
            .unwrap()
            .style_span_bounds(span)
            .unwrap();
        // The border box ends at the closing border (6 + 4 + 5 past the content), not at the
        // newline after the closing margin (6 + 15).
        close(after.0, before.0 + 1.);
        close(after.1, before.1 + 15.);
    }
}

#[test]
fn out_of_flow_box_pushed_at_pending_whitespace_marks_its_edge_side() {
    use crate::{InlineBox, InlineBoxKind, VerticalAlign};
    let oof = || InlineBox {
        id: 9,
        kind: InlineBoxKind::OutOfFlow,
        index: 0,
        width: 9.,
        height: 12.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    };
    let (mut fcx, mut lcx) = (create_font_context(), LayoutContext::new());
    // `e <span><oof/>\u{301}</span>Z`: the box is placed before the pending space, the
    // opening edge after it.
    let layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_text("e ");
        builder.push_style_span_with_shaping_boundaries(style(), false, false);
        builder.push_inline_box(oof());
        builder.push_text("\u{301}");
        builder.pop_style_span();
        builder.push_text("Z");
    });
    let span = span_style(&layout, 0);
    assert_eq!(
        layout.inline_edge_sides_with_pending_out_of_flow_box(span),
        (true, false)
    );
    // `e<span><oof/> \u{301}</span>Z`: nothing is pending when the box is pushed.
    let layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_text("e");
        builder.push_style_span_with_shaping_boundaries(style(), false, false);
        builder.push_inline_box(oof());
        builder.push_text(" \u{301}");
        builder.pop_style_span();
        builder.push_text("Z");
    });
    let span = span_style(&layout, 0);
    assert_eq!(
        layout.inline_edge_sides_with_pending_out_of_flow_box(span),
        (false, false)
    );
    // `<span>a </span><oof/>b`: a box after the close shares the closing edge's position.
    let layout = build(&mut fcx, &mut lcx, |builder| {
        builder.push_style_span_with_shaping_boundaries(style(), false, false);
        builder.push_text("a ");
        builder.pop_style_span();
        builder.push_inline_box(oof());
        builder.push_text("b");
    });
    let span = span_style(&layout, 0);
    assert_eq!(
        layout.inline_edge_sides_with_pending_out_of_flow_box(span),
        (false, true)
    );
}
