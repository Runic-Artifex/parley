// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Authored ordinary-span edge order is immutable metadata, separate from used flow.

use alloc::vec::Vec;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::resolve::tree::SpanEdgeKind;
use crate::{
    Alignment, AlignmentOptions, FontContext, FontFamily, InlineBox, InlineBoxKind, Layout,
    LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty, TextStyle, TreeBuilder,
    VerticalAlign, WhiteSpaceCollapse,
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
) -> (Layout<ColorBrush>, alloc::string::String) {
    let mut b = lcx.tree_builder(fcx, 1., false, &style());
    b.set_style_span_tracking(true);
    configure(&mut b);
    b.build()
}

// Include the owner: an Open/Close sequence alone cannot distinguish nested empty spans
// from siblings, or detect a close mistakenly attributed to its parent.
fn events(layout: &Layout<ColorBrush>) -> Vec<(usize, u16, usize, bool)> {
    layout
        .data
        .span_edges
        .iter()
        .map(|edge| {
            (
                edge.index,
                edge.style,
                edge.boxes_before,
                matches!(edge.kind, SpanEdgeKind::Open),
            )
        })
        .collect()
}

fn owner(layout: &Layout<ColorBrush>, ordinal: usize) -> u16 {
    layout.data.style_spans[ordinal].style
}

fn inline_box(id: u64, kind: InlineBoxKind) -> InlineBox {
    InlineBox {
        id,
        kind,
        index: 0,
        width: 13.,
        height: 12.,
        baseline: Some(9.),
        vertical_align: VerticalAlign::BASELINE,
    }
}

#[test]
fn equal_position_nested_and_sibling_edges_retain_authored_order() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for nested in [false, true] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_style_span(style());
            if !nested {
                b.pop_style_span();
            }
            b.push_style_span(style());
            b.pop_style_span();
            if nested {
                b.pop_style_span();
            }
        });
        assert!(text.is_empty());
        let (a, b) = (owner(&layout, 0), owner(&layout, 1));
        assert_ne!(a, b);
        let expected = if nested {
            alloc::vec![
                (0, a, 0, true),
                (0, b, 0, true),
                (0, b, 0, false),
                (0, a, 0, false)
            ]
        } else {
            alloc::vec![
                (0, a, 0, true),
                (0, a, 0, false),
                (0, b, 0, true),
                (0, b, 0, false)
            ]
        };
        assert_eq!(events(&layout), expected);
        assert!(layout.data.empty_spans.is_empty());
    }
}

#[test]
fn modification_scopes_are_transparent_to_ordinary_descendant_edges() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_style_modification_span(&[StyleProperty::FontSize(21.)]);
        b.push_text("a");
        b.push_style_span(style());
        b.push_style_modification_span(&[StyleProperty::FontSize(22.)]);
        b.push_style_span(style());
        b.push_text("b");
        b.pop_style_span();
        b.pop_style_span();
        b.push_text("c");
        b.pop_style_span();
        b.pop_style_span();
    });
    assert_eq!(text, "abc");
    assert_eq!(layout.data.style_spans.len(), 2);
    let (a, b) = (owner(&layout, 0), owner(&layout, 1));
    assert_eq!(
        events(&layout),
        alloc::vec![
            (1, a, 0, true),
            (1, b, 0, true),
            (2, b, 0, false),
            (3, a, 0, false)
        ]
    );
}

#[test]
fn normalized_endpoints_follow_pending_retained_discarded_and_trailing_whitespace() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (prefix, inside, suffix, preserve_breaks, text, start, end) in [
        ("a ", "b ", " c", false, "a b c", 2, 4),
        ("a", "b", " c", false, "ab c", 1, 2),
        ("a ", "", "b", false, "a b", 2, 2),
        ("a", " ", "b", false, "a b", 1, 2),
        ("a ", "b ", "", false, "a b", 2, 3),
        ("a ", " ", "", false, "a", 1, 1),
        ("a ", "\n", "b", true, "a\nb", 1, 2),
        ("é ", "🙂 ", "x", false, "é 🙂 x", 3, 8),
    ] {
        let (layout, actual_text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text(prefix);
            let mut span_style = style();
            if preserve_breaks {
                span_style.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
            }
            b.push_style_span(span_style);
            b.push_text(inside);
            b.pop_style_span();
            b.push_text(suffix);
        });
        assert_eq!(actual_text, text);
        let s = owner(&layout, 0);
        assert_eq!(
            events(&layout),
            alloc::vec![(start, s, 0, true), (end, s, 0, false)]
        );
    }
}

#[test]
fn coincident_edges_keep_ordinals_for_all_box_kinds_and_nested_spans() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_inline_box(inline_box(0, InlineBoxKind::InFlow));
        b.push_style_span(style());
        b.push_inline_box(inline_box(1, InlineBoxKind::OutOfFlow));
        b.push_style_span(style());
        b.push_inline_box(inline_box(2, InlineBoxKind::CustomOutOfFlow));
        b.pop_style_span();
        b.push_inline_box(inline_box(3, InlineBoxKind::InFlow));
        b.pop_style_span();
        b.push_style_span(style());
        b.pop_style_span();
    });
    assert!(text.is_empty());
    let (a, b, c) = (owner(&layout, 0), owner(&layout, 1), owner(&layout, 2));
    assert_eq!(
        events(&layout),
        alloc::vec![
            (0, a, 1, true),
            (0, b, 2, true),
            (0, b, 3, false),
            (0, a, 4, false),
            (0, c, 4, true),
            (0, c, 4, false)
        ]
    );
    assert_eq!(layout.inline_boxes().count(), 4);
}

#[test]
fn pending_whitespace_can_move_span_endpoints_past_an_authored_out_of_flow_box() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for kind in [InlineBoxKind::OutOfFlow, InlineBoxKind::CustomOutOfFlow] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text("a ");
            b.push_style_span(style());
            b.push_inline_box(inline_box(0, kind));
            b.pop_style_span();
            b.push_text("b");
        });
        assert_eq!(text, "a b");
        let s = owner(&layout, 0);
        assert_eq!(
            events(&layout),
            alloc::vec![(2, s, 0, true), (2, s, 1, false)]
        );
        assert_eq!(layout.inline_boxes().next().unwrap().index, 1);
    }
}

#[test]
fn automatic_closing_preserves_the_explicit_authored_edge_sequence() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut outputs = Vec::new();
    for explicit in [false, true] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_style_span(style());
            b.push_text("a ");
            b.push_style_span(style());
            b.push_text("b ");
            if explicit {
                b.pop_style_span();
                b.pop_style_span();
            }
        });
        outputs.push((text, events(&layout)));
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0].0, "a b");
    assert_eq!(
        outputs[0].1.iter().map(|e| (e.0, e.3)).collect::<Vec<_>>(),
        alloc::vec![(0, true), (2, true), (3, false), (3, false)]
    );
}

#[test]
fn final_tracking_switch_controls_the_complete_authored_history() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for final_enabled in [false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        b.push_style_span(style());
        b.push_text("a");
        b.pop_style_span();
        b.set_style_span_tracking(true);
        b.push_style_span(style());
        b.push_text("b");
        b.pop_style_span();
        b.set_style_span_tracking(false);
        b.push_style_span(style());
        b.push_text("c");
        b.pop_style_span();
        b.set_style_span_tracking(final_enabled);
        let (layout, text) = b.build();
        assert_eq!(text, "abc");
        if final_enabled {
            let (a, b, c) = (owner(&layout, 0), owner(&layout, 1), owner(&layout, 2));
            assert_eq!(
                events(&layout),
                alloc::vec![
                    (0, a, 0, true),
                    (1, a, 0, false),
                    (1, b, 0, true),
                    (2, b, 0, false),
                    (2, c, 0, true),
                    (3, c, 0, false)
                ]
            );
        } else {
            assert!(layout.data.span_edges.is_empty());
        }
    }
}

#[test]
fn reused_context_ranged_builders_and_layout_clear_remove_previous_edges() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut layout = Layout::default();
    for tracked in [true, false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        if tracked {
            b.set_style_span_tracking(true);
        }
        b.push_style_span(style());
        b.push_text("a");
        b.pop_style_span();
        b.build_into(&mut layout);
        assert_eq!(layout.data.span_edges.len(), if tracked { 2 } else { 0 });
    }
    let mut ranged = lcx.ranged_builder(&mut fcx, "a", 1., false);
    ranged.push_default(StyleProperty::FontSize(20.));
    ranged.build_into(&mut layout, "a");
    assert!(layout.data.span_edges.is_empty());
    layout = build(&mut fcx, &mut lcx, |b| {
        b.push_style_span(style());
        b.push_text("a");
    })
    .0;
    layout.clear();
    assert!(layout.data.span_edges.is_empty());
    {
        let mut abandoned = lcx.tree_builder(&mut fcx, 1., false, &style());
        abandoned.set_style_span_tracking(true);
        abandoned.push_style_span(style());
        abandoned.push_text("stale");
    }
    let b = lcx.tree_builder(&mut fcx, 1., false, &style());
    b.build_into(&mut layout);
    assert!(layout.data.span_edges.is_empty());
}

fn paragraph(fcx: &mut FontContext, lcx: &mut LayoutContext<ColorBrush>) -> Layout<ColorBrush> {
    build(fcx, lcx, |b| {
        b.push_style_span(style());
        b.push_text("aa bb ");
        b.push_style_span(style());
        b.push_text("cc dd");
        b.pop_style_span();
        b.push_text(" ee");
        b.pop_style_span();
    })
    .0
}

#[test]
fn clone_rewrap_both_breakers_and_checkpoint_replay_keep_edges_immutable() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let original = paragraph(&mut fcx, &mut lcx);
    let edges = events(&original);
    let mut reused = original.clone();
    for width in [30., 70., 30.] {
        reused.break_all_lines(Some(width));
        reused.align(Alignment::Justify, AlignmentOptions::default());
        assert_eq!(events(&reused), edges);
    }
    for quota in [false, true] {
        let mut sequential = original.clone();
        let mut replay = original.clone();
        for (layout, retry) in [(&mut sequential, false), (&mut replay, true)] {
            let mut breaker = layout.break_lines();
            breaker.state_mut().set_layout_max_advance(35.);
            breaker.state_mut().set_line_max_advance(35.);
            if retry {
                if quota {
                    assert!(breaker.break_next_with_length(2).is_some());
                } else {
                    assert!(breaker.break_next().is_some());
                }
                let saved = breaker.state().clone();
                if quota {
                    assert!(breaker.break_next_with_length(2).is_some());
                } else {
                    assert!(breaker.break_next().is_some());
                }
                breaker.revert_to(saved);
            }
            if quota {
                while breaker.break_next_with_length(2).is_some() {}
            } else {
                while breaker.break_next().is_some() {}
            }
            breaker.finish();
        }
        assert_eq!(events(&sequential), edges);
        assert_eq!(events(&replay), edges);
        assert_eq!(sequential.data.lines, replay.data.lines);
        assert_eq!(sequential.data.line_items, replay.data.line_items);
    }
    assert_eq!(events(&original), edges);
}

#[derive(Debug, PartialEq)]
enum Positioned {
    Glyphs(u16, f32, f32, f32, Vec<(u32, f32, f32, f32)>),
    Box(u64, InlineBoxKind, f32, f32, f32, f32),
}

fn positioned(layout: &Layout<ColorBrush>) -> Vec<Vec<Positioned>> {
    layout
        .lines()
        .map(|line| {
            line.items()
                .map(|item| match item {
                    PositionedLayoutItem::GlyphRun(run) => Positioned::Glyphs(
                        run.style_index(),
                        run.offset(),
                        run.baseline(),
                        run.advance(),
                        run.positioned_glyphs()
                            .map(|g| (g.id, g.advance, g.x, g.y))
                            .collect(),
                    ),
                    PositionedLayoutItem::InlineBox(b) => {
                        Positioned::Box(b.id, b.kind, b.x, b.y, b.width, b.height)
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn captured_edges_add_no_sources_styles_shapes_metrics_or_public_positioned_items() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut outputs = Vec::new();
    for tracked in [false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        b.set_style_span_tracking(tracked);
        b.push_style_span(style());
        b.push_text_with_source("f", 0);
        b.push_style_modification_span(&[StyleProperty::FontSize(20.)]);
        b.push_text_with_source("i ", 1);
        b.pop_style_span();
        b.push_style_span(style());
        b.push_text_with_source("bb", 2);
        b.push_inline_box(inline_box(3, InlineBoxKind::InFlow));
        b.push_inline_box(inline_box(4, InlineBoxKind::OutOfFlow));
        b.pop_style_span();
        b.pop_style_span();
        b.push_text_with_source(" cc", 5);
        let mut layout = Layout::default();
        let (text, sources) = b.build_into_with_sources(&mut layout);
        assert_eq!(layout.data.span_edges.len(), if tracked { 4 } else { 0 });
        layout.break_all_lines(Some(60.));
        layout.align(Alignment::Justify, AlignmentOptions::default());
        let widths = layout.calculate_content_widths();
        outputs.push((
            (
                text,
                sources,
                layout.styles().to_vec(),
                layout.data.shaped_text.clone(),
            ),
            (
                layout.data.runs.clone(),
                layout.data.items.clone(),
                layout.data.lines.clone(),
                layout.data.line_items.clone(),
            ),
            (
                widths.min,
                widths.max,
                layout.width(),
                layout.full_width(),
                layout.height(),
            ),
            positioned(&layout),
        ));
    }
    assert_eq!(outputs[0], outputs[1]);
}
