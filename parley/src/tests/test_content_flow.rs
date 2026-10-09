// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Private content cursors retain whole atoms and the existing distinct positioning policies.

use alloc::vec::Vec;
use core::ops::Range;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::layout::flow::{BoxFlow, ContentFlow, ContentItem};
use crate::{
    Alignment, AlignmentOptions, BaseDirection, FontContext, FontFamily, InlineBox, InlineBoxKind,
    Layout, LayoutContext, LineHeight, PositionedLayoutItem, TextStyle, TreeBuilder, VerticalAlign,
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
) -> Layout<ColorBrush> {
    let mut b = lcx.tree_builder(fcx, 1., false, &style());
    configure(&mut b);
    b.build().0
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

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "expected {expected}, got {actual}"
    );
}

#[derive(Debug, PartialEq)]
enum Content {
    Atom(usize, u32, Range<u32>, Range<usize>, usize, f32, f32, bool),
    Box(usize, usize, u64, InlineBoxKind, f32, f32, bool),
}

fn signature(item: ContentItem<'_, ColorBrush>) -> Content {
    match item {
        ContentItem::Atom(a) => Content::Atom(
            a.item_index,
            a.run.shaped_text_run_index,
            a.atom.shaped_clusters_range(),
            a.run.full_slice().text_byte_range(a.atom.char_range()),
            a.glyph_count,
            a.x,
            a.advance,
            a.run.is_rtl(),
        ),
        ContentItem::InlineBox(b) => Content::Box(
            b.item_index,
            b.box_index,
            b.inline_box.id,
            b.inline_box.kind,
            b.x,
            b.advance,
            b.is_rtl,
        ),
    }
}

fn snapshot(layout: &Layout<ColorBrush>) -> Vec<(Range<usize>, Vec<Content>)> {
    layout
        .lines()
        .map(|line| {
            (
                line.text_range(),
                ContentFlow::new(line).map(signature).collect(),
            )
        })
        .collect()
}

fn box_snapshot(layout: &Layout<ColorBrush>) -> Vec<Vec<(usize, usize, u64, f32, f32, bool)>> {
    layout
        .lines()
        .map(|line| {
            BoxFlow::new(line)
                .map(|b| {
                    (
                        b.item_index,
                        b.box_index,
                        b.inline_box.id,
                        b.x,
                        b.advance,
                        b.is_rtl,
                    )
                })
                .collect()
        })
        .collect()
}

#[test]
fn authored_style_boundaries_do_not_split_real_ligature_or_combining_atoms() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for (first, rest, direction) in [
        ("f", "i", BaseDirection::Ltr),
        ("e", "\u{301}", BaseDirection::Ltr),
        ("ا", "\u{654}", BaseDirection::Rtl),
    ] {
        let mut layout = build(&mut fcx, &mut lcx, |b| {
            b.set_base_direction(direction);
            b.push_style_span(style());
            b.push_text(first);
            b.pop_style_span();
            b.push_style_span(style());
            b.push_text(rest);
            b.pop_style_span();
        });
        layout.break_all_lines(None);
        assert_eq!(layout.data.shaped_text.runs().len(), 1);
        let shaped = layout.data.shaped_text.run_slice(0);
        let expected = shaped.atoms_start().collect::<Vec<_>>();
        assert_eq!(expected.len(), 1, "fixture must shape one whole atom");
        assert_eq!(expected[0].characters().len(), 2);
        assert_ne!(
            expected[0].characters()[0].style_index,
            expected[0].characters()[1].style_index
        );
        let line = layout.lines().next().unwrap();
        let items = ContentFlow::new(line).collect::<Vec<_>>();
        assert_eq!(items.len(), 1);
        let ContentItem::Atom(a) = &items[0] else {
            panic!("expected a whole atom")
        };
        assert_eq!(
            a.atom.shaped_clusters_range(),
            expected[0].shaped_clusters_range()
        );
        assert_eq!(a.atom.characters(), expected[0].characters());
        assert_eq!(
            a.run.full_slice().text_byte_range(a.atom.char_range()),
            0..first.len() + rest.len()
        );
        assert_eq!(a.item_index, 0);
        assert!(a.advance > 0.);
        let positioned = line
            .items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(g) => Some(g),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(positioned.len(), 1);
        assert_eq!(a.glyph_count, positioned[0].glyphs().count());
        if first == "f" {
            assert_eq!(a.glyph_count, 1, "bundled font must actually ligate fi");
        }
        close(a.x, positioned[0].offset());
        close(a.advance, positioned[0].advance());
    }
}

#[test]
fn visual_boxes_keep_original_global_ordinals_and_only_in_flow_width_reserves_space() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
        let rtl = direction == BaseDirection::Rtl;
        let mut layout = build(&mut fcx, &mut lcx, |b| {
            b.set_base_direction(direction);
            b.push_text(if rtl { "ب" } else { "a" });
            for (id, width, kind) in [
                (10, 7.125, InlineBoxKind::InFlow),
                (20, 51.25, InlineBoxKind::OutOfFlow),
                (30, 61.5, InlineBoxKind::CustomOutOfFlow),
                (40, 13.375, InlineBoxKind::InFlow),
            ] {
                b.push_inline_box(inline_box(id, width, kind));
            }
            b.push_text(if rtl { "ت" } else { "b" });
        });
        layout.break_all_lines(None);
        assert_eq!(
            layout.inline_boxes().map(|b| b.id).collect::<Vec<_>>(),
            alloc::vec![10, 20, 30, 40]
        );
        let line = layout.lines().next().unwrap();
        let boxes = ContentFlow::new(line)
            .filter_map(|i| match i {
                ContentItem::InlineBox(b) => Some(b),
                _ => None,
            })
            .collect::<Vec<_>>();
        let expected = if rtl {
            alloc::vec![3, 2, 1, 0]
        } else {
            alloc::vec![0, 1, 2, 3]
        };
        assert_eq!(
            boxes.iter().map(|b| b.box_index).collect::<Vec<_>>(),
            expected
        );
        for b in &boxes {
            assert_eq!(b.inline_box.id, [10, 20, 30, 40][b.box_index]);
            assert_eq!(b.is_rtl, rtl);
            close(
                b.advance,
                match b.inline_box.id {
                    10 => 7.125,
                    40 => 13.375,
                    _ => 0.,
                },
            );
        }
        close(boxes.iter().map(|b| b.advance).sum(), 20.5);
        close(boxes[1].x, boxes[0].x + boxes[0].advance);
        close(boxes[2].x, boxes[1].x);
        close(boxes[3].x, boxes[2].x);
        let public = line
            .items()
            .filter_map(|i| match i {
                PositionedLayoutItem::InlineBox(b) => Some(b),
                _ => None,
            })
            .collect::<Vec<_>>();
        let grouped = BoxFlow::new(line).collect::<Vec<_>>();
        assert_eq!(public.len(), 4);
        for ((a, b), p) in boxes.iter().zip(&grouped).zip(&public) {
            assert_eq!(a.inline_box.id, p.id);
            assert_eq!(a.box_index, b.box_index);
            close(a.x, p.x);
            close(b.x, p.x);
        }
    }
}

#[test]
fn bundled_soft_hyphen_glyph_reserves_its_measured_spacing_advance() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let spaced = TextStyle {
        letter_spacing: 2.,
        ..style()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &spaced);
    builder.push_text("a\u{ad}");
    builder.push_inline_box(inline_box(91, 13.375, InlineBoxKind::InFlow));
    builder.push_text("b");
    let (mut layout, text) = builder.build();
    assert_eq!(text, "a\u{ad}b");
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    let atoms = ContentFlow::new(line)
        .filter_map(|i| match i {
            ContentItem::Atom(a) => Some(a),
            _ => None,
        })
        .collect::<Vec<_>>();
    let shy = atoms
        .iter()
        .find(|a| a.atom.characters()[0].text_byte_start == 1)
        .unwrap();
    assert_eq!(shy.atom.characters().len(), 1);
    assert_eq!(
        shy.glyph_count, 1,
        "bundled fonts retain one glyph for this SHY"
    );
    close(shy.advance, shy.run.line_spacing().atom_advance(&shy.atom));
    assert!(shy.advance > 0., "SHY glyph must retain authored spacing");
    let content_box = ContentFlow::new(line)
        .find_map(|i| match i {
            ContentItem::InlineBox(b) => Some(b),
            _ => None,
        })
        .unwrap();
    close(content_box.x, shy.x + shy.advance);
    let following = atoms
        .iter()
        .find(|a| a.atom.characters()[0].text_byte_start == 3)
        .unwrap();
    close(following.x, content_box.x + 13.375);
    let grouped_box = BoxFlow::new(line).next().unwrap();
    let run = line.runs().next().unwrap();
    close(
        grouped_box.x,
        line.metrics().inline_min_coord + line.metrics().offset + run.advance(),
    );
    close(grouped_box.x, content_box.x);
    let public_box = line
        .items()
        .find_map(|i| match i {
            PositionedLayoutItem::InlineBox(b) => Some(b),
            _ => None,
        })
        .unwrap();
    close(content_box.x, public_box.x);
}

#[test]
fn forced_newlines_are_real_zero_glyph_atoms_and_terminal_line_exhausts() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut layout = build(&mut fcx, &mut lcx, |b| {
        b.push_style_span(TextStyle {
            white_space_collapse: WhiteSpaceCollapse::Preserve,
            ..style()
        });
        b.push_text("a\nb\n");
        b.pop_style_span();
    });
    layout.break_all_lines(None);
    assert_eq!(layout.len(), 3);
    let mut newlines = Vec::new();
    for line in layout.lines() {
        for item in ContentFlow::new(line) {
            if let ContentItem::Atom(a) = item {
                if a.atom.characters()[0].whitespace == parley_engine::shape::Whitespace::Newline {
                    assert_eq!(
                        a.glyph_count, 0,
                        "fixture must exercise the glyphless newline branch"
                    );
                    close(a.advance, 0.);
                    newlines.push(a.atom.characters()[0].text_byte_start);
                }
            }
        }
    }
    assert_eq!(newlines, alloc::vec![1, 3]);
    let terminal = layout.lines().last().unwrap();
    assert_eq!(terminal.text_range(), 4..4);
    assert!(ContentFlow::new(terminal).next().is_none());
    assert!(BoxFlow::new(terminal).next().is_none());
}

fn paragraph(fcx: &mut FontContext, lcx: &mut LayoutContext<ColorBrush>) -> Layout<ColorBrush> {
    build(fcx, lcx, |b| {
        b.push_text("aa bb ");
        b.push_inline_box(inline_box(10, 7.125, InlineBoxKind::InFlow));
        b.push_inline_box(inline_box(20, 51.25, InlineBoxKind::OutOfFlow));
        b.push_inline_box(inline_box(30, 61.5, InlineBoxKind::CustomOutOfFlow));
        b.push_text(" cc dd ee ff");
    })
}

#[test]
fn cloned_cursors_resume_independently_and_remain_exhausted() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut layout = paragraph(&mut fcx, &mut lcx);
    layout.break_all_lines(None);
    let line = layout.lines().next().unwrap();
    let mut cursor = ContentFlow::new(line);
    assert!(cursor.next().is_some());
    assert!(cursor.next().is_some());
    let mut cloned = cursor.clone();
    let rest = cloned.by_ref().map(signature).collect::<Vec<_>>();
    assert!(!rest.is_empty());
    assert_eq!(cursor.by_ref().map(signature).collect::<Vec<_>>(), rest);
    for _ in 0..3 {
        assert!(cursor.next().is_none());
        assert!(cloned.next().is_none());
    }
    let mut boxes = BoxFlow::new(line);
    assert_eq!(boxes.next().unwrap().box_index, 0);
    let mut box_clone = boxes.clone();
    assert_eq!(
        boxes.by_ref().map(|b| b.box_index).collect::<Vec<_>>(),
        alloc::vec![1, 2]
    );
    assert_eq!(
        box_clone.by_ref().map(|b| b.box_index).collect::<Vec<_>>(),
        alloc::vec![1, 2]
    );
    for _ in 0..3 {
        assert!(boxes.next().is_none());
        assert!(box_clone.next().is_none());
    }
}

#[test]
fn alignment_and_justification_recompute_positions_and_match_fresh_layouts() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut reused = paragraph(&mut fcx, &mut lcx);
    reused.break_all_lines(Some(320.));
    let initial = snapshot(&reused);
    let initial_boxes = box_snapshot(&reused);
    assert_eq!(initial.len(), 1);
    for alignment in [Alignment::Center, Alignment::Justify, Alignment::Start] {
        let options = AlignmentOptions {
            last_line_alignment: Some(alignment),
            ..AlignmentOptions::default()
        };
        reused.align(alignment, options);
        let mut fresh = paragraph(&mut fcx, &mut lcx);
        fresh.break_all_lines(Some(320.));
        fresh.align(alignment, options);
        assert_eq!(snapshot(&reused), snapshot(&fresh));
        assert_eq!(box_snapshot(&reused), box_snapshot(&fresh));
        if alignment == Alignment::Start {
            assert_eq!(snapshot(&reused), initial);
            assert_eq!(box_snapshot(&reused), initial_boxes);
        } else {
            assert_ne!(
                snapshot(&reused),
                initial,
                "fixture must expose new positions"
            );
            assert_ne!(box_snapshot(&reused), initial_boxes);
        }
        if alignment == Alignment::Center {
            let shift = reused.lines().next().unwrap().metrics().offset;
            assert!(shift > 0.);
            close(box_snapshot(&reused)[0][0].3 - initial_boxes[0][0].3, shift);
        }
        if alignment == Alignment::Justify {
            let line = reused.lines().next().unwrap();
            let last = ContentFlow::new(line).last().unwrap();
            let ContentItem::Atom(last) = last else {
                panic!("fixture ends in text")
            };
            close(last.x + last.advance, 320.);
        }
    }
}

#[test]
fn rewrapping_a_cloned_layout_matches_fresh_positions_and_restores_narrow_lines() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let original = paragraph(&mut fcx, &mut lcx);
    let shaped = original.data.shaped_text.clone();
    let mut reused = original.clone();
    let mut narrow = None;
    let mut narrow_lines = 0;
    for width in [35., 120., 35.] {
        reused.break_all_lines(Some(width));
        let mut fresh = paragraph(&mut fcx, &mut lcx);
        fresh.break_all_lines(Some(width));
        assert_eq!(snapshot(&reused), snapshot(&fresh));
        assert_eq!(box_snapshot(&reused), box_snapshot(&fresh));
        assert_eq!(reused.data.shaped_text, shaped);
        if width == 35. {
            if let Some(expected) = &narrow {
                assert_eq!(&snapshot(&reused), expected);
            } else {
                narrow_lines = reused.len();
                narrow = Some(snapshot(&reused));
            }
        } else {
            assert!(reused.len() < narrow_lines, "fixture must actually rewrap");
        }
    }
    assert!(
        original.lines().next().is_none(),
        "cloned layout breaking must not mutate original"
    );
}

#[test]
fn width_and_character_break_checkpoint_replay_preserve_content_and_box_positions() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    // CustomOutOfFlow yields control to width-break callers until explicitly consumed.
    // Keep this replay fixture to text and in-flow boxes; the visual-box test separately
    // verifies all three kinds. Every successful step here must commit a visible text line.
    let original = build(&mut fcx, &mut lcx, |b| {
        b.push_text("aa bb ");
        b.push_inline_box(inline_box(10, 7.125, InlineBoxKind::InFlow));
        b.push_text(" cc dd ");
        b.push_inline_box(inline_box(40, 9.375, InlineBoxKind::InFlow));
        b.push_text(" ee ff");
    });
    for quota in [false, true] {
        let mut sequential = original.clone();
        let mut replay = original.clone();
        for (layout, retry) in [(&mut sequential, false), (&mut replay, true)] {
            let mut breaker = layout.break_lines();
            breaker.state_mut().set_layout_max_advance(35.);
            breaker.state_mut().set_line_max_advance(35.);
            if retry {
                assert!(progressing_break(&mut breaker, quota));
                let saved = breaker.state().clone();
                assert!(progressing_break(&mut breaker, quota));
                breaker.revert_to(saved);
            }
            for _ in 0..128 {
                if !progressing_break(&mut breaker, quota) {
                    break;
                }
            }
            assert!(
                breaker.is_done(),
                "replay fixture must finish within 128 lines"
            );
            breaker.finish();
        }
        assert!(
            sequential.len() > 2,
            "fixture must cross multiple saved opportunities"
        );
        assert_eq!(snapshot(&sequential), snapshot(&replay));
        assert_eq!(box_snapshot(&sequential), box_snapshot(&replay));
        let ordinals = box_snapshot(&replay)
            .into_iter()
            .flatten()
            .map(|b| b.1)
            .collect::<Vec<_>>();
        assert_eq!(
            ordinals,
            alloc::vec![0, 1],
            "line replay must neither duplicate nor omit boxes"
        );
    }
}

fn progressing_break(breaker: &mut crate::BreakLines<'_, ColorBrush>, quota: bool) -> bool {
    let before = breaker.committed_y();
    let committed = if quota {
        breaker.break_next_with_length(2).is_some()
    } else {
        breaker.break_next().is_some()
    };
    if committed {
        assert!(
            breaker.committed_y() > before,
            "successful replay step must advance the committed text line"
        );
    }
    committed
}

#[test]
fn fractional_origins_preserve_original_atom_and_run_grouped_f32_arithmetic_bits() {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let fractional = TextStyle {
        font_size: 17.125,
        letter_spacing: 0.3,
        word_spacing: 0.7,
        white_space_collapse: WhiteSpaceCollapse::Preserve,
        ..style()
    };
    let mut b = lcx.tree_builder(&mut fcx, 1., false, &fractional);
    b.push_text("ab\u{ad} cd");
    b.push_inline_box(inline_box(10, 7.3, InlineBoxKind::InFlow));
    b.push_inline_box(inline_box(20, 51.7, InlineBoxKind::OutOfFlow));
    b.push_inline_box(inline_box(30, 61.2, InlineBoxKind::CustomOutOfFlow));
    b.push_style_span(TextStyle {
        font_size: 19.375,
        letter_spacing: 0.35,
        ..fractional.clone()
    });
    b.push_text("ef\u{ad} gh");
    b.pop_style_span();
    b.push_inline_box(inline_box(40, 13.7, InlineBoxKind::InFlow));
    b.push_text("ij\n");
    let (mut layout, text) = b.build();
    layout.break_all_lines(None);
    assert_eq!(
        layout.len(),
        2,
        "preserved final newline adds a terminal line"
    );
    assert!(
        ContentFlow::new(layout.lines().last().unwrap())
            .next()
            .is_none()
    );
    let mut detects_late_origin = false;
    let mut detects_run_regrouping = false;
    let mut detects_wide_accumulation = false;
    for (inline_min_coord, offset) in [
        (0.1, 0.2),
        (4096.125, 0.3),
        (16384.25, 0.7),
        (1_048_576.125, 0.3),
    ] {
        layout.data.lines[0].metrics.inline_min_coord = inline_min_coord;
        layout.data.lines[0].metrics.offset = offset;
        let line = layout.lines().next().unwrap();
        let origin = inline_min_coord + offset;
        let mut atom_x = origin;
        let mut box_x = origin;
        let mut regrouped_x = origin;
        let mut late_offset = 0_f32;
        let mut wide_x = f64::from(origin);
        let mut expected_content = Vec::new();
        let mut expected_boxes = Vec::new();
        let mut glyphless = 0;
        let mut soft_hyphens = 0;
        let mut fractional_advances = 0;
        let mut runs = 0;
        let mut out_of_flow = 0;

        // Retain the two original 10478f6 formulas as an independent oracle:
        // style_spans::bounds added each visible atom to the already combined origin;
        // empty_spans::positions added Run::advance once per run. Neither reserved OOF width.
        // This intentionally avoids either new cursor and does not certify paint/caret equality.
        for item_index in 0..line.len() {
            match line.item(item_index).unwrap() {
                crate::layout::LineItem::Run(run) => {
                    runs += 1;
                    assert!(
                        !run.is_rtl(),
                        "this arithmetic fixture uses logical LTR atoms"
                    );
                    let mut run_visible_advance = 0_f32;
                    for atom in run.line_slice().atoms_start() {
                        let glyph_count: usize = atom
                            .shaped_clusters()
                            .iter()
                            .map(|cluster| usize::from(cluster.glyph_len()))
                            .sum();
                        let text_range = run.full_slice().text_byte_range(atom.char_range());
                        if &text[text_range] == "\u{ad}" {
                            soft_hyphens += 1;
                            assert_eq!(glyph_count, 1, "bundled SHY glyph count is verified");
                        }
                        let advance = if glyph_count == 0 {
                            glyphless += 1;
                            0.
                        } else {
                            // Original AtomIter::measure first assigned atom.advance(), then
                            // added the complete spacing gap before advancing the f32 cursor.
                            let mut advance = atom.advance();
                            if !run.line_spacing().is_zero() {
                                advance += run.line_spacing().gaps(&atom).total();
                            }
                            advance
                        };
                        fractional_advances += usize::from(advance.fract() != 0.);
                        detects_late_origin |= atom_x.to_bits() != (origin + late_offset).to_bits();
                        detects_wide_accumulation |= atom_x.to_bits() != (wide_x as f32).to_bits();
                        expected_content.push((
                            0_u8,
                            u64::from(atom.characters()[0].text_byte_start),
                            item_index,
                            usize::MAX,
                            glyph_count,
                            atom_x.to_bits(),
                            advance.to_bits(),
                        ));
                        atom_x += advance;
                        run_visible_advance += advance;
                        late_offset += advance;
                        wide_x += f64::from(advance);
                    }
                    regrouped_x += run_visible_advance;
                    detects_run_regrouping |= atom_x.to_bits() != regrouped_x.to_bits();
                    box_x += run.advance();
                }
                crate::layout::LineItem::InlineBox(inline_box) => {
                    let box_index =
                        layout.data.line_items[line.data.item_range.start + item_index].index;
                    let advance = if inline_box.kind == InlineBoxKind::InFlow {
                        inline_box.width
                    } else {
                        out_of_flow += 1;
                        0.
                    };
                    expected_content.push((
                        1_u8,
                        inline_box.id,
                        item_index,
                        box_index,
                        0,
                        atom_x.to_bits(),
                        advance.to_bits(),
                    ));
                    expected_boxes.push((
                        item_index,
                        box_index,
                        inline_box.id,
                        box_x.to_bits(),
                        advance.to_bits(),
                    ));
                    box_x += advance;
                    if inline_box.kind == InlineBoxKind::InFlow {
                        atom_x += inline_box.width;
                        regrouped_x += inline_box.width;
                        late_offset += inline_box.width;
                        wide_x += f64::from(inline_box.width);
                    }
                }
            }
        }
        assert!(runs >= 3 && fractional_advances >= 3);
        assert_eq!(
            glyphless, 1,
            "preserved newline must exercise the skipped atom advance"
        );
        assert_eq!(soft_hyphens, 2, "fixture retains both glyph-bearing SHYs");
        assert_eq!(
            out_of_flow, 2,
            "both OOF kinds must exercise zero reservation"
        );
        let actual_content = ContentFlow::new(line)
            .map(|item| match item {
                ContentItem::Atom(a) => (
                    0_u8,
                    u64::from(a.atom.characters()[0].text_byte_start),
                    a.item_index,
                    usize::MAX,
                    a.glyph_count,
                    a.x.to_bits(),
                    a.advance.to_bits(),
                ),
                ContentItem::InlineBox(b) => (
                    1_u8,
                    b.inline_box.id,
                    b.item_index,
                    b.box_index,
                    0,
                    b.x.to_bits(),
                    b.advance.to_bits(),
                ),
            })
            .collect::<Vec<_>>();
        let actual_boxes = BoxFlow::new(line)
            .map(|b| {
                (
                    b.item_index,
                    b.box_index,
                    b.inline_box.id,
                    b.x.to_bits(),
                    b.advance.to_bits(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual_content, expected_content,
            "original per-atom f32 positions at origin {origin}"
        );
        assert_eq!(
            actual_boxes, expected_boxes,
            "original run-grouped f32 box positions at origin {origin}"
        );
    }
    assert!(
        detects_late_origin,
        "fixture must reject adding the origin after content accumulation"
    );
    assert!(
        detects_run_regrouping,
        "fixture must reject replacing atom additions with a run sum"
    );
    assert!(
        detects_wide_accumulation,
        "fixture must reject accumulating the origin/content in f64"
    );
}
