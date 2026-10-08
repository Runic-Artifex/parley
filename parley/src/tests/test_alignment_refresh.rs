// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Cached vertical-alignment refresh must retain shaping and require only line rebreaking.

use alloc::{string::String, vec::Vec};
use core::ops::Range;
use parley_engine::{
    ShapedRun,
    shape::{Character, ShapedCluster},
};

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    FontContext, FontFamily, Glyph, InlineBox, InlineBoxKind, Layout, LayoutContext, LineHeight,
    LineMetrics, PositionedLayoutItem, QuantizationMode, TextSourceRange, TextStyle, VerticalAlign,
};

fn style(size: f32, height: f32, align: VerticalAlign) -> TextStyle<'static, 'static, ColorBrush> {
    TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size: size,
        line_height: LineHeight::Absolute(height),
        vertical_align: align,
        ..TextStyle::default()
    }
}

const INITIAL: [VerticalAlign; 5] = [
    VerticalAlign::BASELINE,
    VerticalAlign::BASELINE,
    VerticalAlign::length(2.),
    VerticalAlign::TOP,
    VerticalAlign::BOTTOM,
];

// Inputs are in layout units, like the refresh API. Convert back for the builder, which scales
// authored lengths. Every ordinary span explicitly splits shaping in both the baseline and
// rebuilt reference, so fresh geometry does not accidentally compare different shaped text.
fn paragraph(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<ColorBrush>,
    scale: f32,
    quantize: bool,
    mode: QuantizationMode,
    alignments: [VerticalAlign; 5],
) -> (Layout<ColorBrush>, String, Vec<TextSourceRange>) {
    let css = alignments.map(|align| align.scale(1. / scale));
    let mut b = lcx.tree_builder(fcx, scale, quantize, &style(20., 30., css[0]));
    b.set_quantization_mode(mode);
    b.push_text_with_source("fi ", 0);
    b.push_style_span_with_shaping_boundaries(style(20., 30., css[1]), true, true);
    b.push_text_with_source("fi ", 1);
    b.push_style_span_with_shaping_boundaries(style(12., 18., css[2]), true, true);
    b.push_text_with_source("fi ", 2);
    b.push_inline_box(InlineBox {
        id: 91,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 8. * scale,
        height: 14. * scale,
        baseline: Some(10. * scale),
        vertical_align: VerticalAlign::BASELINE,
    });
    b.pop_style_span();
    b.pop_style_span();
    b.push_style_span_with_shaping_boundaries(style(28., 42., css[3]), true, true);
    b.push_text_with_source(" fi ", 3);
    b.pop_style_span();
    b.push_style_span_with_shaping_boundaries(style(10., 16., css[4]), true, true);
    b.push_text_with_source("fi ", 4);
    b.pop_style_span();
    b.push_text_with_source("fi", 0);
    let mut layout = Layout::default();
    let (text, sources) = b.build_into_with_sources(&mut layout);
    assert_eq!(
        layout.styles().len(),
        5,
        "fixture must have stable parent-first style ids"
    );
    layout.break_all_lines(None);
    (layout, text, sources)
}

#[derive(Debug, PartialEq)]
struct ShapeSnapshot {
    runs: Vec<ShapedRun>,
    characters: Vec<Character>,
    clusters: Vec<ShapedCluster>,
    glyphs: Vec<Glyph>,
    // Compare allocation identity as well as contents. No pointer is dereferenced.
    arenas: [usize; 6],
    text_len: usize,
}

fn shape_snapshot(layout: &Layout<ColorBrush>) -> ShapeSnapshot {
    let shaped = &layout.data.shaped_text;
    ShapeSnapshot {
        runs: shaped.runs().to_vec(),
        characters: shaped.characters().to_vec(),
        clusters: shaped.shaped_clusters().to_vec(),
        glyphs: shaped.glyphs().to_vec(),
        arenas: [
            shaped.runs().as_ptr() as usize,
            shaped.characters().as_ptr() as usize,
            shaped.shaped_clusters().as_ptr() as usize,
            shaped.glyphs().as_ptr() as usize,
            shaped.fonts().as_ptr() as usize,
            shaped.normalized_coords().as_ptr() as usize,
        ],
        text_len: layout.data.text_len,
    }
}

#[derive(Debug, PartialEq)]
struct PositionedLine {
    range: Range<usize>,
    metrics: LineMetrics,
    baselines: Vec<f32>,
    glyphs: Vec<Glyph>,
    boxes: Vec<(u64, f32, f32, f32, f32)>,
}

fn positions(layout: &Layout<ColorBrush>) -> Vec<PositionedLine> {
    layout
        .lines()
        .map(|line| {
            let mut glyphs = Vec::new();
            let mut boxes = Vec::new();
            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(run) => glyphs.extend(run.positioned_glyphs()),
                    PositionedLayoutItem::InlineBox(b) => {
                        boxes.push((b.id, b.x, b.y, b.width, b.height))
                    }
                }
            }
            PositionedLine {
                range: line.text_range(),
                metrics: *line.metrics(),
                baselines: (0..layout.styles().len())
                    .map(|id| line.style_baseline(id as u16))
                    .collect(),
                glyphs,
                boxes,
            }
        })
        .collect()
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "expected {expected}, got {actual}"
    );
}

// Alignment refresh may change accumulated offsets/subtree identity, but all cached primary
// font and line-height inputs must remain byte-for-byte equivalent.
fn font_inputs(layout: &Layout<ColorBrush>) -> Vec<[f32; 8]> {
    layout
        .data
        .style_metrics
        .iter()
        .map(|m| {
            [
                m.ascent,
                m.descent,
                m.x_height,
                m.over,
                m.under,
                m.line_height,
                m.font_size,
                f32::from(m.parent),
            ]
        })
        .collect()
}

#[test]
fn refresh_matches_fresh_nested_and_line_relative_geometry_without_changing_shaping() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (quantize, mode) in [
        (true, QuantizationMode::Device),
        (true, QuantizationMode::Logical),
        (false, QuantizationMode::Device),
        (false, QuantizationMode::Logical),
    ] {
        for updated in [
            [
                VerticalAlign::length(9.),
                VerticalAlign::length(20.),
                VerticalAlign::SUPER,
                VerticalAlign::BOTTOM,
                VerticalAlign::TOP,
            ],
            [
                VerticalAlign::BASELINE,
                VerticalAlign::TOP,
                VerticalAlign::TEXT_BOTTOM,
                VerticalAlign::MIDDLE,
                VerticalAlign::length(-4.),
            ],
            [
                VerticalAlign::BASELINE,
                VerticalAlign::BOTTOM,
                VerticalAlign::TEXT_TOP,
                VerticalAlign::SUB,
                VerticalAlign::BASELINE,
            ],
        ] {
            let (mut actual, text, sources) =
                paragraph(&mut fcx, &mut lcx, 1.25, quantize, mode, INITIAL);
            let original_shape = shape_snapshot(&actual);
            let original_fonts = font_inputs(&actual);
            let before_positions = positions(&actual);
            assert!(actual.update_vertical_alignments(&updated.map(Some)));
            assert_eq!(shape_snapshot(&actual), original_shape);
            assert_eq!(font_inputs(&actual), original_fonts);
            actual.break_all_lines(None);
            let (expected, expected_text, expected_sources) =
                paragraph(&mut fcx, &mut lcx, 1.25, quantize, mode, updated);
            assert_eq!(text, expected_text);
            assert_eq!(sources, expected_sources);
            assert_eq!(actual.data.style_metrics, expected.data.style_metrics);
            assert_eq!(positions(&actual), positions(&expected));
            assert_ne!(
                positions(&actual),
                before_positions,
                "fixture must expose an alignment change"
            );
            assert_eq!(shape_snapshot(&actual), original_shape);
        }
    }
}

#[test]
fn sparse_parent_updates_recompute_kept_descendants_parent_first() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (mut layout, _, _) = paragraph(
        &mut fcx,
        &mut lcx,
        1.,
        false,
        QuantizationMode::Device,
        INITIAL,
    );
    let shape = shape_snapshot(&layout);
    assert!(layout.update_vertical_alignments(&[
        None,
        Some(VerticalAlign::length(6.4)),
        None,
        None,
        None
    ]));
    assert_close(layout.data.style_metrics[1].exact_baseline_offset, 6.4);
    assert_close(layout.data.style_metrics[2].exact_baseline_offset, 8.4);
    assert_eq!(layout.styles()[2].vertical_align, INITIAL[2]);
    assert!(layout.update_vertical_alignments(&[None, Some(VerticalAlign::TOP), None, None, None]));
    assert_eq!(layout.data.style_metrics[1].aligned_subtree, 1);
    assert_eq!(layout.data.style_metrics[2].aligned_subtree, 1);
    assert_close(layout.data.style_metrics[2].exact_baseline_offset, 2.);
    layout.break_all_lines(None);
    let mut expected_alignments = INITIAL;
    expected_alignments[1] = VerticalAlign::TOP;
    let (expected, _, _) = paragraph(
        &mut fcx,
        &mut lcx,
        1.,
        false,
        QuantizationMode::Device,
        expected_alignments,
    );
    assert_eq!(positions(&layout), positions(&expected));
    assert_eq!(shape_snapshot(&layout), shape);
}

#[test]
fn refresh_lengths_are_layout_units_and_round_only_accumulated_offsets() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (quantize, mode) in [
        (true, QuantizationMode::Device),
        (true, QuantizationMode::Logical),
        (false, QuantizationMode::Device),
        (false, QuantizationMode::Logical),
    ] {
        let (mut layout, _, _) = paragraph(&mut fcx, &mut lcx, 1.25, quantize, mode, INITIAL);
        assert!(layout.update_vertical_alignments(&[
            Some(VerticalAlign::length(13.)),
            Some(VerticalAlign::length(6.4)),
            Some(VerticalAlign::length(0.4)),
            None,
            None,
        ]));
        let metrics = &layout.data.style_metrics;
        assert_close(metrics[0].exact_baseline_offset, 0.);
        assert_close(metrics[1].exact_baseline_offset, 6.4);
        assert_close(metrics[2].exact_baseline_offset, 6.8);
        let rounded = quantize && mode == QuantizationMode::Device;
        assert_close(metrics[1].baseline_offset, if rounded { 6. } else { 6.4 });
        assert_close(metrics[2].baseline_offset, if rounded { 7. } else { 6.8 });
        assert_eq!(
            layout.styles()[0].vertical_align,
            VerticalAlign::length(13.)
        );
        assert_eq!(
            layout.styles()[1].vertical_align,
            VerticalAlign::length(6.4)
        );
        layout.break_all_lines(None);
    }
}

#[test]
fn resetting_lengths_to_zero_restores_geometry_and_shape_without_a_rebuild() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let zero = [VerticalAlign::BASELINE; 5];
    let (mut layout, _, _) = paragraph(
        &mut fcx,
        &mut lcx,
        1.25,
        true,
        QuantizationMode::Logical,
        zero,
    );
    let original_shape = shape_snapshot(&layout);
    let original_positions = positions(&layout);
    assert!(layout.update_vertical_alignments(&[
        None,
        Some(VerticalAlign::length(21.)),
        Some(VerticalAlign::length(-7.)),
        None,
        None
    ]));
    layout.break_all_lines(None);
    assert_ne!(positions(&layout), original_positions);
    assert!(layout.update_vertical_alignments(&zero.map(Some)));
    layout.break_all_lines(None);
    assert_eq!(positions(&layout), original_positions);
    assert_eq!(shape_snapshot(&layout), original_shape);
}

#[test]
fn invalid_length_is_atomic_and_valid_unchanged_updates_are_noops() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let (mut layout, _, _) = paragraph(
        &mut fcx,
        &mut lcx,
        1.,
        true,
        QuantizationMode::Logical,
        INITIAL,
    );
    let shape = shape_snapshot(&layout);
    let styles = layout.styles().to_vec();
    let metrics = layout.data.style_metrics.clone();
    let positioned = positions(&layout);
    let metric_arena = layout.data.style_metrics.as_ptr();
    let style_arena = layout.styles().as_ptr();
    for bad in [
        alloc::vec![],
        alloc::vec![Some(VerticalAlign::SUPER); 4],
        alloc::vec![Some(VerticalAlign::BOTTOM); 6],
    ] {
        assert!(!layout.update_vertical_alignments(&bad));
        assert_eq!(layout.styles(), styles.as_slice());
        assert_eq!(layout.data.style_metrics, metrics);
        assert_eq!(positions(&layout), positioned);
        assert_eq!(shape_snapshot(&layout), shape);
    }
    for unchanged in [[None; 5], INITIAL.map(Some)] {
        assert!(layout.update_vertical_alignments(&unchanged));
        assert_eq!(layout.styles(), styles.as_slice());
        assert_eq!(layout.data.style_metrics, metrics);
        assert_eq!(positions(&layout), positioned);
        assert_eq!(shape_snapshot(&layout), shape);
        assert_eq!(layout.data.style_metrics.as_ptr(), metric_arena);
        assert_eq!(layout.styles().as_ptr(), style_arena);
    }
}

#[test]
fn an_atomic_box_tracks_the_refreshed_parent_baseline() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for mode in [QuantizationMode::Device, QuantizationMode::Logical] {
        let (mut layout, _, _) = paragraph(&mut fcx, &mut lcx, 1.25, true, mode, INITIAL);
        let inline_boxes = layout.inline_boxes().cloned().collect::<Vec<_>>();
        assert!(layout.update_vertical_alignments(&[
            None,
            Some(VerticalAlign::length(11.)),
            Some(VerticalAlign::length(5.)),
            None,
            None
        ]));
        layout.break_all_lines(None);
        let line = layout.lines().next().unwrap();
        let atomic = line
            .items()
            .find_map(|item| match item {
                PositionedLayoutItem::InlineBox(b) if b.id == 91 => Some(b),
                _ => None,
            })
            .unwrap();
        assert_close(atomic.y + atomic.baseline.unwrap(), line.style_baseline(2));
        assert_eq!(
            layout.inline_boxes().cloned().collect::<Vec<_>>(),
            inline_boxes
        );
    }
}

#[test]
fn refreshed_width_rewrap_and_quota_replay_match_fresh_alignment_layouts() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let updated = [
        VerticalAlign::BASELINE,
        VerticalAlign::MIDDLE,
        VerticalAlign::SUPER,
        VerticalAlign::BOTTOM,
        VerticalAlign::TOP,
    ];
    for mode in [QuantizationMode::Device, QuantizationMode::Logical] {
        let (mut layout, _, _) = paragraph(&mut fcx, &mut lcx, 1.25, true, mode, INITIAL);
        let shape = shape_snapshot(&layout);
        assert!(layout.update_vertical_alignments(&updated.map(Some)));
        for width in [35., 75., 35.] {
            layout.break_all_lines(Some(width));
            let (mut expected, _, _) = paragraph(&mut fcx, &mut lcx, 1.25, true, mode, updated);
            expected.break_all_lines(Some(width));
            assert_eq!(positions(&layout), positions(&expected));
            assert_eq!(shape_snapshot(&layout), shape);
        }
        {
            let mut breaker = layout.break_lines();
            assert!(breaker.break_next_with_length(2).is_some());
            let saved = breaker.state().clone();
            assert!(breaker.break_next_with_length(2).is_some());
            breaker.revert_to(saved);
            while breaker.break_next_with_length(2).is_some() {}
            breaker.finish();
        }
        let (mut expected, _, _) = paragraph(&mut fcx, &mut lcx, 1.25, true, mode, updated);
        {
            let mut breaker = expected.break_lines();
            while breaker.break_next_with_length(2).is_some() {}
            breaker.finish();
        }
        assert_eq!(positions(&layout), positions(&expected));
        assert_eq!(shape_snapshot(&layout), shape);
        {
            let mut breaker = layout.break_lines();
            assert!(breaker.break_next_with_length(2).is_some());
            breaker.revert_to(crate::BreakerState::default());
            while breaker.break_next_with_length(2).is_some() {}
            breaker.finish();
        }
        assert_eq!(positions(&layout), positions(&expected));
        assert_eq!(shape_snapshot(&layout), shape);
    }
}
