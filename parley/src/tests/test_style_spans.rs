// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Normalized ordinary-span ownership without text, shaping or metric changes.

use alloc::vec::Vec;
use core::ops::Range;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    Alignment, AlignmentOptions, BaseDirection, FontContext, FontFamily, InlineBox, InlineBoxKind,
    Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty, TextStyle, TreeBuilder,
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
    let mut builder = lcx.tree_builder(fcx, 1., false, &style());
    builder.set_style_span_tracking(true);
    configure(&mut builder);
    let (mut layout, text) = builder.build();
    layout.break_all_lines(None);
    (layout, text)
}

fn owner(layout: &Layout<ColorBrush>, index: usize) -> usize {
    usize::from(layout.data.style_spans[index].style)
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "expected {expected}, got {actual}"
    );
}

fn close_bounds(actual: (f32, f32), expected: (f32, f32)) {
    close(actual.0, expected.0);
    close(actual.1, expected.1);
}

fn glyphs(layout: &Layout<ColorBrush>) -> Vec<(u32, f32, f32, f32)> {
    layout
        .lines()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => Some(
                run.positioned_glyphs()
                    .map(|g| (g.id, g.advance, g.x, g.y))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

fn snapshot(layout: &Layout<ColorBrush>) -> Vec<(Range<usize>, Vec<Option<(f32, f32)>>)> {
    layout
        .lines()
        .map(|line| {
            (
                line.text_range(),
                layout
                    .data
                    .style_spans
                    .iter()
                    .map(|span| line.style_span_bounds(usize::from(span.style)))
                    .collect(),
            )
        })
        .collect()
}

fn inline_box(id: u64, width: f32, kind: InlineBoxKind) -> InlineBox {
    InlineBox {
        id,
        kind,
        index: 0,
        width,
        height: 12.,
        baseline: Some(9.),
        vertical_align: VerticalAlign::BASELINE,
    }
}

#[test]
fn tracking_is_default_off_switchable_and_cleared_by_builder_and_layout_reuse() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (mut reused, _) = build(&mut fcx, &mut lcx, |b| {
        b.push_style_span(style());
        b.push_text("a");
        b.pop_style_span();
    });
    assert!(
        reused
            .lines()
            .next()
            .unwrap()
            .style_span_bounds(owner(&reused, 0))
            .is_some()
    );
    assert_eq!(reused.lines().next().unwrap().style_span_bounds(0), None);
    assert_eq!(
        reused.lines().next().unwrap().style_span_bounds(usize::MAX),
        None
    );
    for switched in [false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        if switched {
            b.set_style_span_tracking(true);
            b.set_style_span_tracking(false);
        }
        b.push_style_span(style());
        b.push_text("a");
        b.pop_style_span();
        b.build_into(&mut reused);
        reused.break_all_lines(None);
        assert!(reused.data.style_spans.is_empty());
        assert_eq!(reused.lines().next().unwrap().style_span_bounds(1), None);
    }
    let mut ranged = lcx.ranged_builder(&mut fcx, "a", 1., false);
    ranged.push_default(StyleProperty::FontSize(20.));
    ranged.build_into(&mut reused, "a");
    reused.break_all_lines(None);
    assert!(reused.data.style_spans.is_empty());
}

#[test]
fn full_fi_and_combining_atoms_belong_to_their_first_owner() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (first, continuation) in [("f", "i"), ("e", "\u{301}")] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_style_span(style());
            b.push_style_span(style());
            b.push_text(first);
            b.pop_style_span();
            b.push_style_span(style());
            b.push_text(continuation);
            b.pop_style_span();
            b.pop_style_span();
        });
        assert_eq!(text, alloc::format!("{first}{continuation}"));
        assert_eq!(layout.data.shaped_text.runs().len(), 1);
        assert_eq!(
            layout.data.shaped_text.run_slice(0).atoms_start().count(),
            1
        );
        if first == "f" {
            assert_eq!(glyphs(&layout).len(), 1, "bundled font must ligate fi");
        }
        let line = layout.lines().next().unwrap();
        let outer = line.style_span_bounds(owner(&layout, 0)).unwrap();
        close_bounds(line.style_span_bounds(owner(&layout, 1)).unwrap(), outer);
        close_bounds(
            line.style_span_bounds(owner(&layout, 2)).unwrap(),
            (outer.1, outer.1),
        );
    }
}

#[test]
fn normalized_close_follows_only_whitespace_pending_at_capture() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (inside, suffix, expected_text, expected_range) in [
        ("a ", "b", "x a b", 2..4),
        ("a", " b", "x a b", 2..3),
        ("a ", "", "x a", 2..3),
    ] {
        let (layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_text("x ");
            b.push_style_span(style());
            b.push_text(inside);
            b.pop_style_span();
            b.push_text(suffix);
        });
        assert_eq!(text, expected_text);
        assert_eq!(layout.data.style_spans[0].range, expected_range);
        assert!(
            layout
                .lines()
                .next()
                .unwrap()
                .style_span_bounds(owner(&layout, 0))
                .is_some()
        );
    }
    let (automatic, _) = build(&mut fcx, &mut lcx, |b| {
        b.push_text("é ");
        b.push_style_span(style());
        b.push_text("a ");
    });
    assert_eq!(automatic.data.style_spans[0].range, 3..4);
}

#[test]
fn empty_geometry_requires_independent_empty_participation() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut signatures = Vec::new();
    for retain in [false, true] {
        let (layout, _) = build(&mut fcx, &mut lcx, |b| {
            b.set_empty_span_participation(retain);
            b.push_text("f");
            b.push_style_span(style());
            b.pop_style_span();
            b.push_text("i");
        });
        let line = layout.lines().next().unwrap();
        if retain {
            let (_, x) = line.empty_spans().next().unwrap();
            close_bounds(line.style_span_bounds(owner(&layout, 0)).unwrap(), (x, x));
        } else {
            assert_eq!(line.style_span_bounds(owner(&layout, 0)), None);
        }
        signatures.push(glyphs(&layout));
    }
    assert_eq!(signatures[0], signatures[1]);
}

#[test]
fn atomic_ordinals_include_descendants_and_exclude_out_of_flow_width() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, _) = build(&mut fcx, &mut lcx, |b| {
        b.set_empty_span_participation(true);
        b.push_text("a");
        b.push_style_span(style());
        b.pop_style_span();
        b.push_inline_box(inline_box(10, 7., InlineBoxKind::InFlow));
        b.push_style_span(style());
        b.push_style_span(style());
        b.push_inline_box(inline_box(20, 13., InlineBoxKind::InFlow));
        b.pop_style_span();
        b.push_inline_box(inline_box(30, 50., InlineBoxKind::OutOfFlow));
        b.pop_style_span();
        b.push_style_span(style());
        b.pop_style_span();
        b.push_text("b");
    });
    let line = layout.lines().next().unwrap();
    let boxes = line
        .items()
        .filter_map(|item| match item {
            PositionedLayoutItem::InlineBox(b) => Some(b),
            _ => None,
        })
        .collect::<Vec<_>>();
    let first = boxes.iter().find(|b| b.id == 10).unwrap();
    let nested = boxes.iter().find(|b| b.id == 20).unwrap();
    close_bounds(
        line.style_span_bounds(owner(&layout, 0)).unwrap(),
        (first.x, first.x),
    );
    for index in [1, 2] {
        close_bounds(
            line.style_span_bounds(owner(&layout, index)).unwrap(),
            (nested.x, nested.x + nested.width),
        );
    }
    close_bounds(
        line.style_span_bounds(owner(&layout, 3)).unwrap(),
        (nested.x + nested.width, nested.x + nested.width),
    );
}

fn wrapped(fcx: &mut FontContext, lcx: &mut LayoutContext<ColorBrush>) -> Layout<ColorBrush> {
    build(fcx, lcx, |b| {
        b.push_text("x ");
        b.push_style_span(style());
        b.push_text("aa bb\n");
        b.push_style_span(style());
        b.push_text("cc dd");
        b.pop_style_span();
        b.pop_style_span();
    })
    .0
}

#[test]
fn width_rebreaking_both_breakers_and_saved_replay_keep_owned_lines() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut reused = wrapped(&mut fcx, &mut lcx);
    for width in [30., 70., 30.] {
        let mut fresh = wrapped(&mut fcx, &mut lcx);
        fresh.break_all_lines(Some(width));
        reused.break_all_lines(Some(width));
        assert_eq!(snapshot(&reused), snapshot(&fresh));
    }
    for quota in [false, true] {
        let mut plain = wrapped(&mut fcx, &mut lcx);
        let mut replay = wrapped(&mut fcx, &mut lcx);
        {
            let mut breaker = plain.break_lines();
            breaker.state_mut().set_layout_max_advance(35.);
            breaker.state_mut().set_line_max_advance(35.);
            if quota {
                while breaker.break_next_with_length(2).is_some() {}
            } else {
                while breaker.break_next().is_some() {}
            }
            breaker.finish();
        }
        {
            let mut breaker = replay.break_lines();
            breaker.state_mut().set_layout_max_advance(35.);
            breaker.state_mut().set_line_max_advance(35.);
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
            if quota {
                while breaker.break_next_with_length(2).is_some() {}
            } else {
                while breaker.break_next().is_some() {}
            }
            breaker.finish();
        }
        assert_eq!(snapshot(&plain), snapshot(&replay));
        assert_eq!(glyphs(&plain), glyphs(&replay));
    }
}

#[test]
fn forced_breaks_do_not_create_ownership_on_the_synthetic_terminal_line() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, text) = build(&mut fcx, &mut lcx, |b| {
        b.push_style_span(TextStyle {
            white_space_collapse: WhiteSpaceCollapse::Preserve,
            ..style()
        });
        b.push_text("a\nb\n");
        b.pop_style_span();
    });
    assert_eq!(text, "a\nb\n");
    let bounds = layout
        .lines()
        .map(|line| line.style_span_bounds(owner(&layout, 0)))
        .collect::<Vec<_>>();
    assert_eq!(bounds.len(), 3);
    assert!(bounds[0].is_some() && bounds[1].is_some());
    assert_eq!(bounds[2], None);
}

#[test]
fn descendant_whitespace_policy_controls_each_hanging_contribution() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (parent, child) in [
        (WhiteSpaceCollapse::Collapse, WhiteSpaceCollapse::Preserve),
        (WhiteSpaceCollapse::Preserve, WhiteSpaceCollapse::Collapse),
    ] {
        let (mut layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.push_style_span(TextStyle {
                white_space_collapse: parent,
                ..style()
            });
            b.push_text("aa");
            b.push_style_span(TextStyle {
                white_space_collapse: child,
                ..style()
            });
            b.push_text("  ");
            b.pop_style_span();
            b.push_text("bb");
            b.pop_style_span();
        });
        let b_index = text.find('b').unwrap();
        let cluster = crate::Cluster::from_byte_index(&layout, b_index).unwrap();
        let width = cluster.visual_offset().unwrap() + cluster.advance() / 2.;
        layout.break_all_lines(Some(width));
        let line = layout.lines().next().unwrap();
        assert!(line.metrics().hanging_advance > 0.);
        let outer = line.style_span_bounds(owner(&layout, 0)).unwrap();
        let child_bounds = line.style_span_bounds(owner(&layout, 1)).unwrap();
        let visible_end =
            line.metrics().inline_min_coord + line.metrics().offset + line.metrics().advance
                - line.metrics().hanging_advance;
        if child == WhiteSpaceCollapse::Preserve {
            close(outer.1, line.metrics().advance);
            close(child_bounds.1, outer.1);
            assert!(outer.1 > visible_end);
        } else {
            close(outer.1, visible_end);
            close_bounds(child_bounds, (visible_end, visible_end));
        }
    }
}

#[test]
fn owned_unions_follow_alignment_justification_spacing_and_scale() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for scale in [1., 1.5, 2.] {
        let root = TextStyle {
            word_spacing: 2.,
            letter_spacing: 0.5,
            ..style()
        };
        let mut b = lcx.tree_builder(&mut fcx, scale, false, &root);
        b.set_style_span_tracking(true);
        b.push_style_span(root);
        b.push_text("aa bb cc dd");
        b.pop_style_span();
        let (mut layout, _) = b.build();
        layout.break_all_lines(Some(90. * scale));
        for alignment in [Alignment::Center, Alignment::Justify, Alignment::Start] {
            layout.align(alignment, AlignmentOptions::default());
            for line in layout.lines() {
                let bounds = line.style_span_bounds(owner(&layout, 0)).unwrap();
                let start = line.metrics().inline_min_coord + line.metrics().offset;
                close(bounds.0, start);
                let end = line
                    .items()
                    .filter_map(|item| match item {
                        PositionedLayoutItem::GlyphRun(run) => Some(run.offset() + run.advance()),
                        _ => None,
                    })
                    .last()
                    .unwrap();
                close(bounds.1, end - line.metrics().hanging_advance);
            }
        }
    }
}

#[test]
fn glyphless_soft_hyphen_with_spacing_follows_positioned_glyphs_and_boxes() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let spaced = TextStyle {
        letter_spacing: 2.,
        ..style()
    };
    let mut b = lcx.tree_builder(&mut fcx, 1., false, &spaced);
    b.set_style_span_tracking(true);
    b.push_style_span(spaced.clone());
    b.push_text("a\u{ad}");
    b.pop_style_span();
    b.push_inline_box(inline_box(1, 13., InlineBoxKind::InFlow));
    b.push_style_span(spaced);
    b.push_text("b");
    b.pop_style_span();
    let (mut layout, _) = b.build();
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    let mut first_run = None;
    let mut positioned_box = None;
    let mut following_run = None;
    for item in line.items() {
        match item {
            PositionedLayoutItem::GlyphRun(run) if first_run.is_none() => first_run = Some(run),
            PositionedLayoutItem::GlyphRun(run) => following_run = Some(run),
            PositionedLayoutItem::InlineBox(b) => positioned_box = Some(b),
        }
    }
    let first_run = first_run.unwrap();
    let positioned_box = positioned_box.unwrap();
    let following_run = following_run.unwrap();
    close_bounds(
        line.style_span_bounds(owner(&layout, 0)).unwrap(),
        (first_run.offset(), first_run.offset() + first_run.advance()),
    );
    close(positioned_box.x, first_run.offset() + first_run.advance());
    close_bounds(
        line.style_span_bounds(owner(&layout, 1)).unwrap(),
        (
            following_run.offset(),
            following_run.offset() + following_run.advance(),
        ),
    );
    close(
        following_run.offset(),
        positioned_box.x + positioned_box.width,
    );
}

#[test]
fn tracking_keeps_sources_shapes_metrics_and_public_items_unchanged() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut outputs = Vec::new();
    for track in [false, true] {
        let mut b = lcx.tree_builder(&mut fcx, 1., false, &style());
        b.set_style_span_tracking(track);
        b.push_style_span(style());
        b.push_text_with_source("f", 0);
        b.push_style_modification_span(&[StyleProperty::FontSize(20.)]);
        b.push_text_with_source("i ", 1);
        b.pop_style_span();
        b.pop_style_span();
        b.push_text_with_source(" bb", 2);
        let mut layout = Layout::default();
        let (text, sources) = b.build_into_with_sources(&mut layout);
        layout.break_all_lines(Some(40.));
        let shapes = layout
            .data
            .shaped_text
            .runs()
            .iter()
            .map(|run| run.range.byte_range.clone())
            .collect::<Vec<_>>();
        let metrics = layout
            .lines()
            .map(|line| *line.metrics())
            .collect::<Vec<_>>();
        let widths = layout.calculate_content_widths();
        assert!(
            layout
                .lines()
                .flat_map(|line| line.items())
                .all(|item| matches!(item, PositionedLayoutItem::GlyphRun(_)))
        );
        if track {
            assert_eq!(layout.data.style_spans.len(), 1);
        }
        outputs.push((
            text,
            sources,
            shapes,
            metrics,
            widths.min,
            widths.max,
            glyphs(&layout),
        ));
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn rtl_continuation_uses_the_logical_atom_end() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (layout, _) = build(&mut fcx, &mut lcx, |b| {
        b.set_base_direction(BaseDirection::Rtl);
        b.push_style_span(style());
        b.push_style_span(style());
        b.push_text("ا");
        b.pop_style_span();
        b.push_style_span(style());
        b.push_text("\u{654}");
        b.pop_style_span();
        b.pop_style_span();
    });
    assert_eq!(
        layout.data.shaped_text.run_slice(0).atoms_start().count(),
        1,
        "bundled Arabic font must shape a real base/combining atom"
    );
    let line = layout.lines().next().unwrap();
    let outer = line.style_span_bounds(owner(&layout, 0)).unwrap();
    close_bounds(line.style_span_bounds(owner(&layout, 1)).unwrap(), outer);
    close_bounds(
        line.style_span_bounds(owner(&layout, 2)).unwrap(),
        (outer.0, outer.0),
    );
}

#[test]
fn partial_hanging_atoms_keep_their_non_whitespace_prefix_in_both_directions() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (direction, inside, suffix) in [
        (BaseDirection::Ltr, "a\u{0D4E} ", "b c"),
        (BaseDirection::Rtl, "\u{0710}\u{070F} ", "\u{0712} \u{0713}"),
    ] {
        let (mut layout, text) = build(&mut fcx, &mut lcx, |b| {
            b.set_base_direction(direction);
            b.push_style_span(style());
            // These bundled-font content-width fixtures retain a prepend and following
            // space in one atom with separate shaped clusters; only the space hangs.
            b.push_text(inside);
            b.pop_style_span();
            b.push_text(suffix);
        });
        assert_eq!(text, alloc::format!("{inside}{suffix}"));
        let width = layout.calculate_content_widths().min + 0.01;
        layout.break_all_lines(Some(width));
        let line = layout.lines().next().unwrap();
        let metrics = line.metrics();
        assert!(
            line.runs().any(|run| {
                run.line_slice().atoms_start().any(|atom| {
                    let mut hanging = 0.;
                    let mut suffix_only = false;
                    for cluster in atom.shaped_clusters().iter().rev() {
                        if run
                            .line_slice()
                            .characters_in(cluster.chars_range())
                            .iter()
                            .all(|character| {
                                character.whitespace == parley_engine::shape::Whitespace::Space
                            })
                        {
                            hanging += cluster.advance;
                        } else {
                            suffix_only = true;
                            break;
                        }
                    }
                    atom.shaped_clusters().len() >= 2 && suffix_only && hanging > 0.
                })
            }),
            "the fixture must exercise a partially hanging atom"
        );
        assert!(metrics.hanging_advance > 0.);
        assert!(metrics.hanging_advance < metrics.advance);
        let bounds = line.style_span_bounds(owner(&layout, 0)).unwrap();
        let start = metrics.inline_min_coord + metrics.offset;
        let expected = if direction == BaseDirection::Rtl {
            (start + metrics.hanging_advance, start + metrics.advance)
        } else {
            (start, start + metrics.advance - metrics.hanging_advance)
        };
        close_bounds(bounds, expected);
    }
}

#[test]
fn negative_spacing_owned_bounds_follow_positioned_items() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for scale in [1., 1.5, 2.] {
        let root = TextStyle {
            letter_spacing: -0.5,
            word_spacing: -0.25,
            ..style()
        };
        let mut b = lcx.tree_builder(&mut fcx, scale, false, &root);
        b.set_style_span_tracking(true);
        b.push_style_span(root);
        b.push_text("ab cd ef");
        b.pop_style_span();
        let (mut layout, _) = b.build();
        layout.break_all_lines(Some(55. * scale));
        for line in layout.lines() {
            let glyph_bounds = line
                .items()
                .filter_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        Some((run.offset(), run.offset() + run.advance()))
                    }
                    _ => None,
                })
                .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
                .unwrap();
            close_bounds(
                line.style_span_bounds(owner(&layout, 0)).unwrap(),
                (
                    glyph_bounds.0,
                    glyph_bounds.1 - line.metrics().hanging_advance,
                ),
            );
        }
    }
}
