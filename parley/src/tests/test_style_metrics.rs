// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests for per-style span box metrics.

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::layout::style_metrics::StyleMetrics;
use crate::{
    AlignmentBaseline, BaselineShift, FontFamily, InlineBox, InlineBoxKind, Layout, LayoutContext,
    LineHeight, QuantizationMode, StyleProperty, TextStyle, VerticalAlign,
};

fn root_style() -> TextStyle<'static, 'static, ColorBrush> {
    TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size: 20.,
        line_height: LineHeight::Absolute(30.),
        ..TextStyle::default()
    }
}

#[test]
fn source_metadata_keeps_ligatures_bidi_and_positioned_glyphs_unchanged() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::<ColorBrush>::new();
    let mut plain = lcx.tree_builder(&mut fcx, 1., false, &root_style());
    plain.push_text("of");
    plain.push_text("fice אבג");
    plain.push_style_modification_span(&[StyleProperty::Brush(ColorBrush::new(
        peniko::Color::WHITE,
    ))]);
    plain.push_text(" XYZ");
    plain.pop_style_span();
    let (mut expected, text) = plain.build();
    expected.break_all_lines(Some(95.));

    let mut tagged = lcx.tree_builder(&mut fcx, 1., false, &root_style());
    tagged.push_text_with_source("of", 0);
    tagged.push_text_with_source("fice אבג", 1);
    tagged.push_style_modification_span(&[StyleProperty::Brush(ColorBrush::new(
        peniko::Color::WHITE,
    ))]);
    tagged.push_text_with_source(" XYZ", 2);
    tagged.pop_style_span();
    let mut actual = Layout::default();
    let (actual_text, sources) = tagged.build_into_with_sources(&mut actual);
    actual.break_all_lines(Some(95.));
    assert_eq!(actual_text, text);
    assert_eq!(sources.len(), 3);
    let signature = |layout: &Layout<ColorBrush>| {
        layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                crate::PositionedLayoutItem::GlyphRun(run) => {
                    let range = run.text_range();
                    assert!(
                        !range.is_empty()
                            && text.is_char_boundary(range.start)
                            && text.is_char_boundary(range.end)
                    );
                    assert!(
                        run.run().text_range().start <= range.start
                            && range.end <= run.run().text_range().end
                    );
                    Some((
                        run.style_index(),
                        range,
                        run.offset(),
                        run.advance(),
                        run.positioned_glyphs()
                            .map(|glyph| (glyph.id, glyph.x, glyph.y, glyph.advance))
                            .collect::<alloc::vec::Vec<_>>(),
                    ))
                }
                _ => None,
            })
            .collect::<alloc::vec::Vec<_>>()
    };
    assert_eq!(signature(&actual), signature(&expected));
}

/// Root style with three spans: `A` (40px) containing `B` (10px, `vertical-align: super`) and a
/// sibling `top` aligned span `C` containing `D`.
fn build() -> Layout<ColorBrush> {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_text("root ");
    builder.push_style_span(TextStyle {
        font_size: 40.,
        line_height: LineHeight::Absolute(50.),
        ..root_style()
    });
    builder.push_style_span(TextStyle {
        font_size: 10.,
        line_height: LineHeight::Absolute(20.),
        vertical_align: VerticalAlign::SUPER,
        ..root_style()
    });
    builder.push_text("B");
    builder.pop_style_span();
    builder.pop_style_span();
    builder.push_style_modification_span(&[StyleProperty::VerticalAlign(VerticalAlign::TOP)]);
    builder
        .push_style_modification_span(&[StyleProperty::VerticalAlign(VerticalAlign::length(3.))]);
    builder.push_text("D");
    builder.pop_style_span();
    builder.pop_style_span();
    let (layout, _) = builder.build();
    layout
}

fn metrics(layout: &Layout<ColorBrush>) -> &[StyleMetrics] {
    &layout.data.style_metrics
}

/// Exercise a genuine outline whose font has no declared x-height, without adding a binary
/// fixture. OS/2 version 1 predates sxHeight; all other Roboto tables stay unchanged.
fn missing_x_height_context() -> crate::FontContext {
    use alloc::sync::Arc;
    use fontique::{Collection, CollectionOptions, SourceCache};
    let mut data =
        include_bytes!("../../../parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf").to_vec();
    let count = u16::from_be_bytes(data[4..6].try_into().unwrap()) as usize;
    let entry = (0..count)
        .map(|i| 12 + i * 16)
        .find(|&offset| &data[offset..offset + 4] == b"OS/2")
        .unwrap();
    let offset = u32::from_be_bytes(data[entry + 8..entry + 12].try_into().unwrap()) as usize;
    data[offset..offset + 2].copy_from_slice(&1_u16.to_be_bytes());
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    collection.register_fonts(peniko::Blob::new(Arc::new(data)), None);
    crate::FontContext {
        collection,
        source_cache: SourceCache::default(),
    }
}

fn missing_x_height_layout(
    fcx: &mut crate::FontContext,
    scale: f32,
    quantize: bool,
    mode: QuantizationMode,
) -> Layout<ColorBrush> {
    let mut lcx = LayoutContext::new();
    let root = TextStyle {
        font_size: 24.,
        line_height: LineHeight::Absolute(20.),
        ..root_style()
    };
    let mut builder = lcx.tree_builder(fcx, scale, quantize, &root);
    builder.set_quantization_mode(mode);
    builder.push_text("Before ");
    builder.push_style_modification_span(&[
        StyleProperty::FontSize(10.),
        StyleProperty::VerticalAlign(VerticalAlign::MIDDLE),
    ]);
    builder.push_text("x");
    builder.pop_style_span();
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    layout
}

#[test]
fn missing_x_height_logical_fallback_is_measured_before_scaling() {
    let mut fcx = missing_x_height_context();
    let at_1 = missing_x_height_layout(&mut fcx, 1., true, QuantizationMode::Logical);
    let expected = metrics(&at_1);
    assert!(expected[0].x_height > 0.);
    assert_ne!(expected[0].x_height, expected[0].ascent * 0.56);
    for scale in [1.25, 2.] {
        let layout = missing_x_height_layout(&mut fcx, scale, true, QuantizationMode::Logical);
        for (actual, expected) in metrics(&layout).iter().zip(expected) {
            assert_eq!(actual.x_height / scale, expected.x_height);
            assert_eq!(actual.baseline_offset / scale, expected.baseline_offset);
        }
    }
    // Span metrics may synthesize the fallback, but public shaped-run metrics retain None.
    assert!(
        at_1.lines()
            .flat_map(|line| line.runs())
            .all(|run| run.font_metrics().x_height.is_none())
    );
}

#[test]
fn missing_x_height_device_and_unquantized_modes_keep_legacy_estimate() {
    let mut fcx = missing_x_height_context();
    for scale in [1., 1.25, 2.] {
        let device = missing_x_height_layout(&mut fcx, scale, true, QuantizationMode::Device);
        for metrics in metrics(&device) {
            assert_eq!(metrics.x_height, (metrics.ascent * 0.56).round());
        }
        let unquantized = missing_x_height_layout(&mut fcx, scale, false, QuantizationMode::Device);
        for metrics in metrics(&unquantized) {
            assert_eq!(metrics.x_height, metrics.ascent * 0.56);
        }
        let logical = missing_x_height_layout(&mut fcx, scale, false, QuantizationMode::Logical);
        let mut expected = unquantized.data.clone();
        expected.quantization_mode = QuantizationMode::Logical;
        assert_eq!(expected, logical.data);
    }
}

#[test]
fn declared_x_height_is_preserved_in_logical_metrics() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for scale in [1., 1.25, 2.] {
        let mut builder = lcx.tree_builder(&mut fcx, scale, true, &root_style());
        builder.set_quantization_mode(QuantizationMode::Logical);
        builder.push_text("x");
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        let declared = layout
            .lines()
            .next()
            .unwrap()
            .runs()
            .next()
            .unwrap()
            .font_metrics()
            .x_height
            .unwrap();
        assert_eq!(metrics(&layout)[0].x_height, declared);
    }
}

#[test]
fn one_metrics_entry_per_style() {
    let layout = build();
    assert_eq!(layout.data.styles.len(), 5);
    assert_eq!(metrics(&layout).len(), 5);
}

#[test]
fn line_height_is_distributed_as_half_leading() {
    let layout = build();
    let root = metrics(&layout)[0];
    assert_eq!(root.line_height, 30.);
    assert!((root.over + root.under - 30.).abs() < 1e-4);
    assert!(root.over > root.ascent && root.under > root.descent);
    // Roboto at 20px.
    assert!(root.ascent > 15. && root.ascent < 20.);
    assert!(root.x_height > 9. && root.x_height < 12.);
    assert_eq!(root.baseline_offset, 0.);
    assert_eq!(root.aligned_subtree, 0);
}

#[test]
fn ancestors_without_text_have_metrics() {
    let layout = build();
    // Span A has no direct text but still has a style entry with its own 40px metrics.
    let a = metrics(&layout)[1];
    assert_eq!(a.line_height, 50.);
    assert!(a.ascent > 30.);
    assert_eq!(a.baseline_offset, 0.);
    assert_eq!(a.aligned_subtree, 0);
}

#[test]
fn super_shifts_relative_to_parent() {
    let layout = build();
    let b = metrics(&layout)[2];
    assert_eq!(layout.data.styles[2].parent, 1);
    // WebKit/Blink constant: a third of the *parent's* font size (40px).
    assert!((b.baseline_offset - 40. / 3.).abs() < 1e-4);
    assert_eq!(b.aligned_subtree, 0);
}

#[test]
fn top_starts_an_aligned_subtree() {
    let layout = build();
    let c = metrics(&layout)[3];
    let d = metrics(&layout)[4];
    assert_eq!(c.baseline_offset, 0.);
    assert_eq!(c.aligned_subtree, 3);
    // Children of a `top` box are relative to it, not to the root.
    assert_eq!(d.baseline_offset, 3.);
    assert_eq!(d.aligned_subtree, 3);
}

#[test]
fn text_top_middle_and_text_bottom() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    for align in [
        VerticalAlign::TEXT_TOP,
        VerticalAlign::MIDDLE,
        VerticalAlign::TEXT_BOTTOM,
    ] {
        builder.push_style_span(TextStyle {
            font_size: 10.,
            line_height: LineHeight::Absolute(12.),
            vertical_align: align,
            ..root_style()
        });
        builder.push_text("x");
        builder.pop_style_span();
    }
    let (layout, _) = builder.build();
    let m = metrics(&layout);
    let root = m[0];
    let (text_top, middle, text_bottom) = (m[1], m[2], m[3]);
    // The box top sits at the parent's content ascent.
    assert!((text_top.baseline_offset + text_top.over - root.ascent).abs() < 1e-4);
    // The box midpoint sits at half the parent's x-height.
    let mid = middle.baseline_offset + (middle.over - middle.under) / 2.;
    assert!((mid - root.x_height / 2.).abs() < 1e-4);
    // The box bottom sits at the parent's content descent.
    assert!((text_bottom.baseline_offset - text_bottom.under + root.descent).abs() < 1e-4);
}

#[test]
fn alignment_baseline_and_baseline_shift_compose() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    for align in [
        VerticalAlign::TEXT_TOP,
        VerticalAlign::new(AlignmentBaseline::TextTop, BaselineShift::Length(-4.)),
        VerticalAlign::new(AlignmentBaseline::TextTop, BaselineShift::Super),
    ] {
        builder.push_style_span(TextStyle {
            font_size: 10.,
            vertical_align: align,
            ..root_style()
        });
        builder.push_text("x");
        builder.pop_style_span();
    }
    let (layout, _) = builder.build();
    let m = metrics(&layout);
    // `vertical-align: text-top -4px` is `text-top` lowered by 4px ...
    assert!((m[2].baseline_offset - (m[1].baseline_offset - 4.)).abs() < 1e-4);
    // ... and `text-top super` is `text-top` raised by a third of the parent font size.
    assert!((m[3].baseline_offset - (m[1].baseline_offset + m[0].font_size / 3.)).abs() < 1e-4);
}

#[test]
fn trailing_line_after_newline_keeps_newline_style() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = TextStyle {
        line_height: LineHeight::Absolute(10.),
        ..root_style()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_style_span(TextStyle {
        font_size: 40.,
        line_height: LineHeight::Absolute(40.),
        ..root_style()
    });
    builder.push_text("x\n");
    builder.pop_style_span();
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let mut lines = layout.lines();
    let first = lines.next().unwrap();
    let last = lines.next().unwrap();
    assert!(lines.next().is_none());
    assert_eq!(first.metrics().line_height, 40.);
    assert_eq!(last.metrics().line_height, 40.);
    assert_eq!(last.metrics().block_min_coord, 40.);
    assert_eq!(layout.height(), 80.);
}

#[test]
fn quantized_shifted_baselines_are_whole_pixels() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    builder.push_text("a");
    for align in [
        VerticalAlign::length(0.5),
        VerticalAlign::MIDDLE,
        VerticalAlign::SUPER,
    ] {
        builder.push_style_span(TextStyle {
            font_size: 13.,
            line_height: LineHeight::Absolute(15.5),
            vertical_align: align,
            ..root_style()
        });
        builder.push_text("b");
        builder.pop_style_span();
    }
    builder.push_style_span(TextStyle {
        vertical_align: VerticalAlign::BOTTOM,
        ..root_style()
    });
    builder.push_style_span(TextStyle {
        font_size: 13.,
        vertical_align: VerticalAlign::length(0.5),
        ..root_style()
    });
    builder.push_text("c");
    builder.pop_style_span();
    builder.pop_style_span();
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    let mut runs = 0;
    for item in line.items() {
        if let crate::PositionedLayoutItem::GlyphRun(run) = item {
            assert_eq!(run.baseline(), run.baseline().round());
            runs += 1;
        }
    }
    assert_eq!(runs, 5);
}

#[test]
fn quantization_does_not_compound_along_ancestors() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    builder.push_text("a");
    for _ in 0..3 {
        builder.push_style_span(TextStyle {
            vertical_align: VerticalAlign::length(0.4),
            ..root_style()
        });
    }
    builder.push_text("b");
    for _ in 0..3 {
        builder.pop_style_span();
    }
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    let mut baselines = line.items().filter_map(|item| match item {
        crate::PositionedLayoutItem::GlyphRun(run) => Some(run.baseline()),
        _ => None,
    });
    let a = baselines.next().unwrap();
    let b = baselines.next().unwrap();
    // 3 × 0.4 = 1.2 rounds to a 1px shift; rounding each level would give none.
    assert_eq!(a - b, 1.);
}

#[test]
fn quantized_metrics_are_whole_pixels() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = TextStyle {
        line_height: LineHeight::MetricsRelative(1.),
        ..root_style()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    builder.push_text("x");
    let (layout, _) = builder.build();
    let root = metrics(&layout)[0];
    assert_eq!(root.ascent.fract(), 0.);
    assert_eq!(root.descent.fract(), 0.);
    assert_eq!(root.over.fract(), 0.);
    assert_eq!(root.line_height.fract(), 0.);
    assert_eq!(root.under.fract(), 0.);
}

#[test]
fn device_quantization_is_the_builder_default() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = TextStyle {
        font_size: 13.,
        line_height: LineHeight::MetricsRelative(1.),
        ..root_style()
    };

    let mut builder = lcx.tree_builder(&mut fcx, 1.25, true, &root);
    builder.push_text("text");
    let (mut default_layout, _) = builder.build();
    default_layout.break_all_lines(None);

    let mut builder = lcx.tree_builder(&mut fcx, 1.25, true, &root);
    builder.set_quantization_mode(QuantizationMode::Device);
    builder.push_text("text");
    let (mut explicit_layout, _) = builder.build();
    explicit_layout.break_all_lines(None);

    assert_eq!(default_layout.data, explicit_layout.data);
}

#[test]
fn logical_quantization_scales_font_metrics_covariantly() {
    let root = TextStyle {
        font_size: 13.,
        line_height: LineHeight::MetricsRelative(1.),
        ..root_style()
    };
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let mut metrics_at_scale = |scale: f32| {
        let mut builder = lcx.tree_builder(&mut fcx, scale, true, &root);
        builder.set_quantization_mode(QuantizationMode::Logical);
        builder.push_text("text");
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        let metrics = metrics(&layout)[0];
        (
            metrics.ascent / scale,
            metrics.descent / scale,
            metrics.over / scale,
            metrics.under / scale,
            metrics.line_height / scale,
            layout.height() / scale,
        )
    };
    let at_1 = metrics_at_scale(1.);
    for scale in [1.25, 2.] {
        let at_scale = metrics_at_scale(scale);
        for (expected, actual) in [
            (at_1.0, at_scale.0),
            (at_1.1, at_scale.1),
            (at_1.2, at_scale.2),
            (at_1.3, at_scale.3),
            (at_1.4, at_scale.4),
            (at_1.5, at_scale.5),
        ] {
            assert!((expected - actual).abs() < 1e-4);
        }
    }
}

#[test]
fn style_content_bounds_use_own_font_metrics_not_descendant_or_atomic_extents() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_style_span(TextStyle {
        font_size: 40.,
        line_height: LineHeight::Absolute(50.),
        ..root_style()
    });
    builder.push_style_span(TextStyle {
        font_size: 10.,
        line_height: LineHeight::Absolute(20.),
        vertical_align: VerticalAlign::SUPER,
        ..root_style()
    });
    builder.push_text("child");
    builder.pop_style_span();
    builder.push_inline_box(InlineBox {
        id: 1,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 20.,
        height: 100.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    builder.pop_style_span();
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);

    let line = layout.lines().next().unwrap();
    let bounds = line.style_content_bounds(1).unwrap();
    let parent_metrics = metrics(&layout)[1];
    assert!((bounds.0 - (line.metrics().baseline - parent_metrics.ascent)).abs() < 1e-4);
    assert!((bounds.1 - (line.metrics().baseline + parent_metrics.descent)).abs() < 1e-4);
    assert!((bounds.1 - bounds.0) < line.metrics().line_height);

    let atomic = line
        .items()
        .find_map(|item| match item {
            crate::PositionedLayoutItem::InlineBox(inline_box) => Some(inline_box),
            _ => None,
        })
        .unwrap();
    assert!(atomic.height > bounds.1 - bounds.0);
    assert!(atomic.y < bounds.0);
    assert!(line.style_content_bounds(3).is_none());
    assert!(line.style_content_bounds(usize::MAX).is_none());
}

#[test]
fn style_content_bounds_follow_top_bottom_and_nested_vertical_alignment() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_style_span(TextStyle {
        font_size: 18.,
        line_height: LineHeight::Absolute(28.),
        vertical_align: VerticalAlign::TOP,
        ..root_style()
    });
    builder.push_text("top");
    builder.push_style_span(TextStyle {
        font_size: 12.,
        line_height: LineHeight::Absolute(18.),
        vertical_align: VerticalAlign::length(3.),
        ..root_style()
    });
    builder.push_text("nested-top");
    builder.pop_style_span();
    builder.pop_style_span();
    builder.push_style_span(TextStyle {
        font_size: 16.,
        line_height: LineHeight::Absolute(24.),
        vertical_align: VerticalAlign::BOTTOM,
        ..root_style()
    });
    builder.push_text("bottom");
    builder.push_style_span(TextStyle {
        font_size: 11.,
        line_height: LineHeight::Absolute(17.),
        vertical_align: VerticalAlign::length(-2.),
        ..root_style()
    });
    builder.push_text("nested-bottom");
    builder.pop_style_span();
    builder.pop_style_span();
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);

    let line = layout.lines().next().unwrap();
    let mut baselines = [None; 5];
    for item in line.items() {
        if let crate::PositionedLayoutItem::GlyphRun(run) = item {
            baselines[usize::from(run.style_index())] = Some(run.baseline());
        }
    }
    for style_index in 1..=4 {
        let (top, bottom) = line.style_content_bounds(style_index).unwrap();
        let baseline = baselines[style_index].unwrap();
        let style_metrics = metrics(&layout)[style_index];
        assert!((top - (baseline - style_metrics.ascent)).abs() < 1e-4);
        assert!((bottom - (baseline + style_metrics.descent)).abs() < 1e-4);
    }

    let (top_start, _) = line.style_content_bounds(1).unwrap();
    let (_, bottom_end) = line.style_content_bounds(3).unwrap();
    // `top`/`bottom` align each *aligned subtree* to the line box. The nested spans therefore
    // make neither parent span's font content edge coincide with the corresponding line edge.
    assert!(top_start > line.metrics().block_min_coord);
    assert!(bottom_end < line.metrics().block_max_coord);
    assert!(baselines[2].unwrap() < baselines[1].unwrap());
    assert!(baselines[4].unwrap() > baselines[3].unwrap());
}

#[test]
fn style_content_bounds_scale_covariantly_with_logical_quantization() {
    let root = TextStyle {
        font_size: 13.,
        line_height: LineHeight::MetricsRelative(1.),
        ..root_style()
    };
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let mut bounds_at_scale = |scale: f32| {
        let mut builder = lcx.tree_builder(&mut fcx, scale, true, &root);
        builder.set_quantization_mode(QuantizationMode::Logical);
        builder.push_style_span(TextStyle {
            font_size: 17.,
            line_height: LineHeight::MetricsRelative(1.),
            vertical_align: VerticalAlign::length(0.5),
            ..root_style()
        });
        builder.push_text("text");
        builder.pop_style_span();
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        let (top, bottom) = layout
            .lines()
            .next()
            .unwrap()
            .style_content_bounds(1)
            .unwrap();
        (top / scale, bottom / scale)
    };

    let at_1 = bounds_at_scale(1.);
    for scale in [1.25, 2.] {
        let at_scale = bounds_at_scale(scale);
        assert!((at_scale.0 - at_1.0).abs() < 1e-4);
        assert!((at_scale.1 - at_1.1).abs() < 1e-4);
    }
}

#[test]
fn logical_quantization_preserves_fractional_authored_geometry() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = TextStyle {
        line_height: LineHeight::Absolute(15.5),
        ..root_style()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    builder.set_quantization_mode(QuantizationMode::Logical);
    builder.push_text("a");
    builder
        .push_style_modification_span(&[StyleProperty::VerticalAlign(VerticalAlign::length(0.5))]);
    builder.push_text("b");
    builder.pop_style_span();
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 0.,
        height: 25.5,
        baseline: None,
        vertical_align: VerticalAlign::TOP,
    });
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);

    assert_eq!(metrics(&layout)[0].line_height, 15.5);
    assert_eq!(metrics(&layout)[1].baseline_offset, 0.5);
    assert_eq!(layout.lines().next().unwrap().metrics().line_height, 25.5);

    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    builder.set_quantization_mode(QuantizationMode::Logical);
    builder.push_text("a\na");
    let (mut lines_layout, _) = builder.build();
    lines_layout.break_all_lines(None);
    let second_line = lines_layout.lines().nth(1).unwrap();
    assert_eq!(second_line.metrics().block_min_coord, 15.5);
}

#[test]
fn quantize_false_ignores_quantization_mode() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = TextStyle {
        font_size: 13.,
        line_height: LineHeight::MetricsRelative(1.),
        ..root_style()
    };

    let mut builder = lcx.tree_builder(&mut fcx, 1.25, false, &root);
    builder.push_text("text");
    let (mut device_layout, _) = builder.build();
    device_layout.break_all_lines(None);

    let mut builder = lcx.tree_builder(&mut fcx, 1.25, false, &root);
    builder.set_quantization_mode(QuantizationMode::Logical);
    builder.push_text("text");
    let (mut logical_layout, _) = builder.build();
    logical_layout.break_all_lines(None);

    let mut expected = device_layout.data.clone();
    expected.quantization_mode = QuantizationMode::Logical;
    assert_eq!(expected, logical_layout.data);
}

#[test]
fn line_with_only_empty_inline_boxes_is_invisible() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut line_height = |height: f32, kind: InlineBoxKind| {
        let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
        builder.push_inline_box(InlineBox {
            id: 0,
            kind,
            index: 0,
            width: 0.,
            height,
            baseline: None,
            vertical_align: VerticalAlign::BASELINE,
        });
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        layout.lines().next().unwrap().metrics().line_height
    };
    // Empty and out-of-flow boxes alone do not make the strut apply.
    assert_eq!(line_height(0., InlineBoxKind::InFlow), 0.);
    assert_eq!(line_height(10., InlineBoxKind::OutOfFlow), 0.);
    // A non-empty in-flow box does.
    assert_eq!(line_height(10., InlineBoxKind::InFlow), 30.);
}

#[test]
fn root_strut_can_be_disabled_without_suppressing_text_styles() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();

    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 0.,
        height: 10.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    let (mut default_layout, _) = builder.build();
    default_layout.break_all_lines(None);
    assert_eq!(
        default_layout.lines().next().unwrap().metrics().line_height,
        30.
    );

    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 0.,
        height: 10.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    let (mut atomic_only_layout, _) = builder.build();
    atomic_only_layout.set_include_root_strut(false);
    atomic_only_layout.break_all_lines(None);
    assert_eq!(
        atomic_only_layout
            .lines()
            .next()
            .unwrap()
            .metrics()
            .line_height,
        10.
    );

    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_text("text");
    let (mut text_layout, _) = builder.build();
    text_layout.set_include_root_strut(false);
    text_layout.break_all_lines(None);
    // Text atoms contribute their own span boxes and all ancestors, including the root.
    assert_eq!(
        text_layout.lines().next().unwrap().metrics().line_height,
        30.
    );
}

#[test]
fn rebuilding_a_layout_restores_the_root_strut_default() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut layout = Layout::new();

    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 0.,
        height: 10.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    builder.build_into(&mut layout);
    layout.set_include_root_strut(false);
    layout.break_all_lines(None);
    assert_eq!(layout.lines().next().unwrap().metrics().line_height, 10.);

    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 0.,
        height: 10.,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    builder.build_into(&mut layout);
    layout.break_all_lines(None);
    assert_eq!(layout.lines().next().unwrap().metrics().line_height, 30.);
}

/// With quantization, an inline box's fractional height must not be rounded up twice (once for
/// the ascent and once for the descent): the line reserves exactly the box's height.
#[test]
fn quantized_inline_box_extents_are_not_rounded_separately() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &root);
    // Baseline splits the box into 40.5 above and 40.5 below the baseline; rounding each
    // separately would reserve 82px for an 81px box.
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 10.,
        height: 81.,
        baseline: Some(40.5),
        vertical_align: VerticalAlign::BASELINE,
    });
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    assert_eq!(line.metrics().line_height, 81.);
}

/// A box whose reserved space is negative (e.g. an atomic inline with a large negative
/// `margin-top`) is still content: the strut applies and the line keeps its height.
#[test]
fn line_with_negative_height_inline_box_is_not_invisible() {
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let root = root_style();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root);
    builder.push_inline_box(InlineBox {
        id: 0,
        kind: InlineBoxKind::InFlow,
        index: 0,
        width: 10.,
        height: -300.,
        baseline: Some(-305.),
        vertical_align: VerticalAlign::BASELINE,
    });
    let (mut layout, _) = builder.build();
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    assert_eq!(line.metrics().line_height, 30.);
    let inline_box = line
        .items()
        .find_map(|item| match item {
            crate::PositionedLayoutItem::InlineBox(inline_box) => Some(inline_box),
            _ => None,
        })
        .unwrap();
    // The box's own baseline coincides with the line's baseline.
    assert_eq!(
        inline_box.y + inline_box.baseline.unwrap(),
        line.metrics().baseline
    );
}

#[test]
fn fallback_font_glyphs_only_size_the_line_for_normal_line_height() {
    // CSS Inline 3 § 4.1: with a non-`normal` `line-height`, a span box's layout bounds derive
    // solely from its first available font, ignoring glyphs from fallback fonts.
    let mut fcx = create_font_context();
    let kufi = fcx.collection.family_id("Noto Kufi Arabic").unwrap();
    fcx.collection
        .set_fallbacks(fontique::Script::from_bytes(*b"Arab"), [kufi].into_iter());
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let roboto = [parlance::FontFamilyName::Named("Roboto".into())];
    let mut line_height = |text: &str, line_height: LineHeight| {
        let mut builder = lcx.tree_builder(
            &mut fcx,
            1.,
            false,
            &TextStyle {
                font_family: FontFamily::from(&roboto[..]),
                font_size: 20.,
                line_height,
                ..TextStyle::default()
            },
        );
        builder.push_text(text);
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        layout.lines().next().unwrap().metrics().line_height
    };

    // Arabic falls back to Noto Kufi Arabic, whose ascent/descent differ from Roboto's.
    assert_eq!(line_height("x", LineHeight::Absolute(30.)), 30.);
    assert_eq!(line_height("\u{0628}", LineHeight::Absolute(30.)), 30.);
    assert_eq!(line_height("x", LineHeight::FontSizeRelative(1.5)), 30.);
    assert_eq!(
        line_height("\u{0628}", LineHeight::FontSizeRelative(1.5)),
        30.
    );

    let normal = line_height("x", LineHeight::NORMAL);
    let normal_fallback = line_height("\u{0628}", LineHeight::NORMAL);
    assert!(
        normal_fallback > normal,
        "fallback glyphs should grow a `normal` line: {normal_fallback} vs {normal}"
    );
}

#[test]
fn breaking_after_reverting_to_a_default_state_does_not_panic() {
    // `BreakerState::default()` is public, so the line metrics it carries must be a valid
    // (empty) line rather than one that indexing the root aligned subtree would panic on.
    let mut fcx = create_font_context();
    let mut lcx: LayoutContext<ColorBrush> = LayoutContext::new();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &root_style());
    builder.push_text("one two three four");
    let (mut layout, _) = builder.build();

    let mut breaker = layout.break_lines();
    breaker.break_next_with_length(4).unwrap();
    breaker.revert_to(crate::layout::BreakerState::default());
    while breaker.break_next_with_length(4).is_some() {}
    breaker.finish();

    assert!(layout.len() >= 4);
    for line in layout.lines() {
        assert!(line.metrics().line_height > 0.);
    }
}
