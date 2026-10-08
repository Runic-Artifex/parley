// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Caller-requested ordinary-span shaping edges, separate from empty participation.

use alloc::vec::Vec;
use core::ops::Range;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    FontContext, FontFamily, InlineBox, InlineBoxKind, Layout, LayoutContext, LineHeight,
    PositionedLayoutItem, StyleProperty, TextSourceRange, TextStyle, TreeBuilder, VerticalAlign,
    WhiteSpaceCollapse,
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
    let mut builder = lcx.tree_builder(fcx, 1., false, &style());
    configure(&mut builder);
    let (mut layout, text) = builder.build();
    layout.break_all_lines(None);
    (layout, text)
}

fn glyphs(layout: &Layout<ColorBrush>) -> Vec<(u32, f32)> {
    layout
        .lines()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => Some(run),
            _ => None,
        })
        .flat_map(|run| run.glyphs().map(|g| (g.id, g.advance)).collect::<Vec<_>>())
        .collect()
}

fn ranges(layout: &Layout<ColorBrush>) -> Vec<Range<usize>> {
    layout
        .data
        .shaped_text
        .runs()
        .iter()
        .map(|run| run.range.byte_range.clone())
        .collect()
}

// An existing zero-width atomic box independently forces shaping itemization. Compare glyphs
// only: unlike a requested shaping edge, the box also adds a layout item and wrap opportunity.
fn atomic_reference(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<ColorBrush>,
    text: &str,
    cuts: &[usize],
) -> Layout<ColorBrush> {
    build(fcx, lcx, |builder| {
        let mut cursor = 0;
        for &cut in cuts {
            builder.push_text(&text[cursor..cut]);
            builder.push_inline_box(InlineBox {
                id: cut as u64,
                kind: InlineBoxKind::InFlow,
                index: 0,
                width: 0.,
                height: 0.,
                baseline: Some(0.),
                vertical_align: VerticalAlign::BASELINE,
            });
            cursor = cut;
        }
        builder.push_text(&text[cursor..]);
    })
    .0
}

#[test]
fn ordinary_and_unrequested_spans_preserve_a_proven_crossing_fi_ligature() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (joined, _) = build(&mut fcx, &mut lcx, |b| b.push_text("fi"));
    assert_eq!(glyphs(&joined).len(), 1, "bundled font must ligate fi");
    let split = atomic_reference(&mut fcx, &mut lcx, "fi", &[1]);
    assert_eq!(glyphs(&split).len(), 2, "reference must split real fi");
    assert_ne!(glyphs(&joined), glyphs(&split));

    for ordinary in [false, true] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text("f");
            if ordinary {
                b.push_style_span(style());
            } else {
                b.push_style_span_with_shaping_boundaries(style(), false, false);
            }
            b.push_text("i");
            b.pop_style_span();
        });
        assert_eq!(text, "fi");
        assert_eq!(glyphs(&layout), glyphs(&joined));
        assert_eq!(ranges(&layout), alloc::vec![0..2]);
    }
}

#[test]
fn nonempty_open_close_and_both_edges_match_atomic_shaping_references() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (prefix, inside, suffix, before, after, cuts) in [
        ("f", "i", "", true, false, &[1][..]),
        ("", "f", "i", false, true, &[1][..]),
        ("f", "if", "i", true, true, &[1, 3][..]),
    ] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text(prefix);
            b.push_style_span_with_shaping_boundaries(style(), before, after);
            b.push_text(inside);
            b.pop_style_span();
            b.push_text(suffix);
        });
        let reference = atomic_reference(&mut fcx, &mut lcx, &text, cuts);
        assert_eq!(glyphs(&layout), glyphs(&reference));
        assert_eq!(ranges(&layout), ranges(&reference));
        assert!(
            layout
                .lines()
                .flat_map(|l| l.items())
                .all(|item| { matches!(item, PositionedLayoutItem::GlyphRun(_)) })
        );
    }
}

#[test]
fn ligatures_wholly_inside_and_after_a_marked_span_remain_eligible() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (joined, _) = build(&mut fcx, &mut lcx, |b| b.push_text("fi"));
    assert_eq!(glyphs(&joined).len(), 1);
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("f");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("fi");
        b.pop_style_span();
        b.push_text("fi");
    });
    assert_eq!(text, "ffifi");
    let reference = atomic_reference(&mut fcx, &mut lcx, &text, &[1, 3]);
    assert_eq!(glyphs(&layout), glyphs(&reference));
    assert_eq!(&glyphs(&layout)[1..], [glyphs(&joined)[0]; 2].as_slice());
}

#[test]
fn explicit_empty_edges_work_without_empty_participation_or_geometric_shift() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let reference = atomic_reference(&mut fcx, &mut lcx, "fi", &[1]);
    for retain in [false, true] {
        for align in [VerticalAlign::BASELINE, VerticalAlign::length(0.)] {
            let (layout, text) = build(&mut fcx, &mut lcx, |b| {
                b.set_empty_span_participation(retain);
                b.push_text("f");
                b.push_style_span_with_shaping_boundaries(
                    TextStyle {
                        vertical_align: align,
                        ..style()
                    },
                    true,
                    true,
                );
                b.pop_style_span();
                b.push_text("i");
            });
            assert_eq!(text, "fi");
            assert_eq!(glyphs(&layout), glyphs(&reference));
            assert_eq!(ranges(&layout), ranges(&reference));
            assert_eq!(
                layout.lines().next().unwrap().empty_spans().count(),
                usize::from(retain)
            );
        }
    }
}

#[test]
fn nested_coincident_edges_deduplicate_and_closing_edges_are_sorted() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("f");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.pop_style_span();
        b.push_text("i");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("f");
        b.pop_style_span();
        b.pop_style_span();
        b.push_text("i");
    });
    assert_eq!(text, "fifi");
    let reference = atomic_reference(&mut fcx, &mut lcx, &text, &[1, 2, 3]);
    assert_eq!(ranges(&layout), ranges(&reference));
    assert_eq!(glyphs(&layout), glyphs(&reference));
}

#[test]
fn closing_edge_moves_for_already_pending_space_but_not_later_sibling_space() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (inside, suffix, cut) in [("i ", "z", 3), ("i", " z", 2)] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text("f");
            b.push_style_span_with_shaping_boundaries(style(), false, true);
            b.push_text(inside);
            b.pop_style_span();
            b.push_text(suffix);
        });
        assert_eq!(text, "fi z");
        assert_eq!(ranges(&layout), alloc::vec![0..cut, cut..4]);
    }
}

#[test]
fn empty_edges_move_for_external_pending_space_but_not_space_started_after_close() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (prefix, suffix, cut) in [("f ", "i", 2), ("f", " i", 1)] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text(prefix);
            b.push_style_span_with_shaping_boundaries(style(), true, true);
            b.pop_style_span();
            b.push_text(suffix);
        });
        assert_eq!(text, "f i");
        assert_eq!(ranges(&layout), alloc::vec![0..cut, cut..3]);
    }
}

#[test]
fn whitespace_only_spans_keep_distinct_edges_and_discarded_spaces_make_no_endpoint_item() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("f");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text(" \t");
        b.pop_style_span();
        b.push_text("i");
    });
    assert_eq!(text, "f i");
    assert_eq!(ranges(&layout), alloc::vec![0..1, 1..2, 2..3]);
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("  ");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("fi  ");
    });
    assert_eq!(text, "fi");
    assert_eq!(ranges(&layout), alloc::vec![0..2]);
    assert_eq!(glyphs(&layout).len(), 1);
}

#[test]
fn automatic_closure_and_explicit_closure_have_the_same_interior_edges() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut signatures = Vec::new();
    for close in [false, true] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text("f");
            b.push_style_span_with_shaping_boundaries(style(), true, true);
            b.push_text("fi ");
            if close {
                b.pop_style_span();
            }
        });
        assert_eq!(text, "ffi");
        assert_eq!(ranges(&layout), alloc::vec![0..1, 1..3]);
        signatures.push(glyphs(&layout));
    }
    assert_eq!(signatures[0], signatures[1]);
}

#[test]
fn normalized_edges_use_utf8_bytes_and_resolve_spaces_around_multibyte_text() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("é ");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("fi ");
        b.pop_style_span();
        b.push_text("z");
    });
    assert_eq!(text, "é fi z");
    assert_eq!(ranges(&layout), alloc::vec![0..3, 3..6, 6..7]);
    for range in ranges(&layout) {
        assert!(text.is_char_boundary(range.start));
        assert!(text.is_char_boundary(range.end));
    }
    let reference = atomic_reference(&mut fcx, &mut lcx, &text, &[3, 6]);
    assert_eq!(glyphs(&layout), glyphs(&reference));
}

#[test]
fn modification_scopes_do_not_inherit_edges_but_ordinary_descendants_can_request_them() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (joined, _) = build(&mut fcx, &mut lcx, |b| b.push_text("fi"));
    let reference = atomic_reference(&mut fcx, &mut lcx, "fi", &[1]);
    for descendant_edge in [false, true] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            // These parent edges are paragraph endpoints; its modification child must not
            // inherit them as additional internal boundaries.
            b.push_style_span_with_shaping_boundaries(style(), true, true);
            b.push_text("f");
            b.push_style_modification_span(&[StyleProperty::VerticalAlign(
                VerticalAlign::BASELINE,
            )]);
            if descendant_edge {
                b.push_style_span_with_shaping_boundaries(style(), true, false);
            }
            b.push_text("i");
            if descendant_edge {
                b.pop_style_span();
            }
            b.pop_style_span();
            b.pop_style_span();
        });
        assert_eq!(text, "fi");
        assert_eq!(
            glyphs(&layout),
            if descendant_edge {
                glyphs(&reference)
            } else {
                glyphs(&joined)
            }
        );
    }
}

#[test]
fn source_ranges_survive_requested_edges_and_delayed_whitespace_unchanged() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut outputs = Vec::new();
    for requested in [false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        b.push_text_with_source("f ", 0);
        b.push_style_span_with_shaping_boundaries(style(), requested, requested);
        b.push_text_with_source("i ", 1);
        b.pop_style_span();
        b.push_text_with_source("fi", 2);
        let mut layout = Layout::default();
        let output = b.build_into_with_sources(&mut layout);
        assert_eq!(layout.data.text_len, 6);
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0].0, "f i fi");
    assert_eq!(
        outputs[0].1,
        alloc::vec![
            TextSourceRange {
                range: 0..2,
                source: 0
            },
            TextSourceRange {
                range: 2..4,
                source: 1
            },
            TextSourceRange {
                range: 4..6,
                source: 2
            },
        ]
    );
}

#[test]
fn reused_context_and_layout_clear_requests_including_abandoned_builders() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut layout = Layout::default();
    for requested in [true, false] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        b.push_text("f");
        b.push_style_span_with_shaping_boundaries(style(), requested, requested);
        b.pop_style_span();
        b.push_text("i");
        assert_eq!(b.build_into(&mut layout), "fi");
        layout.break_all_lines(None);
        assert_eq!(glyphs(&layout).len(), if requested { 2 } else { 1 });
    }
    {
        let mut abandoned = lcx.tree_builder(&mut fcx, 1., false, &style());
        abandoned.push_text("f ");
        abandoned.push_style_span_with_shaping_boundaries(style(), true, true);
        abandoned.pop_style_span();
    }
    let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
    b.push_text("f");
    b.push_style_modification_span(&[StyleProperty::VerticalAlign(VerticalAlign::BASELINE)]);
    b.push_text("i");
    b.pop_style_span();
    b.build_into(&mut layout);
    layout.break_all_lines(None);
    assert_eq!(ranges(&layout), alloc::vec![0..2]);
    assert_eq!(glyphs(&layout).len(), 1);
}

fn marked_paragraph(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<ColorBrush>,
) -> Layout<ColorBrush> {
    build(fcx, lcx, |b| {
        b.push_text("f");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("if");
        b.pop_style_span();
        b.push_text("i fi fi");
    })
    .0
}

fn line_ranges(layout: &Layout<ColorBrush>) -> Vec<Range<usize>> {
    layout.lines().map(|line| line.text_range()).collect()
}

#[test]
fn width_rewrap_and_quota_replay_preserve_the_same_shaped_edges() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut reused = marked_paragraph(&mut fcx, &mut lcx);
    let shaped_ranges = ranges(&reused);
    let shaped_glyphs = glyphs(&reused);
    for width in [25., 60., 25.] {
        let mut fresh = marked_paragraph(&mut fcx, &mut lcx);
        fresh.break_all_lines(Some(width));
        reused.break_all_lines(Some(width));
        assert_eq!(line_ranges(&reused), line_ranges(&fresh));
        assert_eq!(glyphs(&reused), glyphs(&fresh));
        assert_eq!(ranges(&reused), shaped_ranges);
    }
    let mut sequential = marked_paragraph(&mut fcx, &mut lcx);
    let mut replayed = marked_paragraph(&mut fcx, &mut lcx);
    {
        let mut breaker = sequential.break_lines();
        while breaker.break_next_with_length(1).is_some() {}
        breaker.finish();
    }
    {
        let mut breaker = replayed.break_lines();
        assert!(breaker.break_next_with_length(1).is_some());
        let saved = breaker.state().clone();
        assert!(breaker.break_next_with_length(1).is_some());
        breaker.revert_to(saved);
        while breaker.break_next_with_length(1).is_some() {}
        breaker.finish();
    }
    assert_eq!(line_ranges(&replayed), line_ranges(&sequential));
    assert_eq!(glyphs(&replayed), glyphs(&sequential));
    assert_eq!(ranges(&replayed), shaped_ranges);
    assert_eq!(glyphs(&replayed), shaped_glyphs);
}
