// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit commits cover immutable shaped atoms; these tests do not assign edge geometry.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;
use parlance::BidiLevel;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::layout::line_commit::{
    CommitItem, append_line_items, legacy_commit_items, reorder_line_items,
};
use crate::layout::{LayoutItem, LayoutItemKind, LineItemData};
use crate::{
    Cluster, FontFamily, InlineBox, InlineBoxKind, Layout, LayoutContext, LineHeight, TextStyle,
    TreeBuilder, VerticalAlign, WhiteSpaceCollapse,
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

fn build(configure: impl FnOnce(&mut TreeBuilder<'_, ColorBrush>)) -> (Layout<ColorBrush>, String) {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let mut builder = lcx.tree_builder(&mut fcx, 1., false, &style());
    builder.set_style_span_tracking(true);
    configure(&mut builder);
    let (mut layout, text) = builder.build();
    layout.break_all_lines(None);
    (layout, text)
}

fn inline_box(width: f32) -> InlineBox {
    InlineBox {
        id: 42,
        index: 0,
        kind: InlineBoxKind::OutOfFlow,
        width,
        height: 12.,
        baseline: Some(9.),
        vertical_align: VerticalAlign::BASELINE,
    }
}

fn text_item(run_index: usize, shaped_cluster_range: Range<u32>) -> CommitItem {
    CommitItem::TextRun {
        run_index,
        shaped_cluster_range,
    }
}

fn box_item(box_index: usize, level: u8) -> CommitItem {
    CommitItem::InlineBox {
        box_index,
        bidi_level: BidiLevel::new(level),
    }
}

fn atoms(layout: &Layout<ColorBrush>, run: u32) -> Vec<(Range<u32>, Range<usize>)> {
    let slice = layout.data.shaped_text.run_slice(run);
    let mut cursor = slice.atoms_start();
    let mut result = Vec::new();
    while let Some(atom) = cursor.next() {
        result.push((
            atom.shaped_clusters_range(),
            slice.text_byte_range(atom.char_range()),
        ));
    }
    result
}

// Keep the real breaker's line metadata and shaped storage. Only replace its committed
// coverage for read-side assertions; metrics are intentionally not inspected afterwards.
fn install(layout: &mut Layout<ColorBrush>, items: Vec<LineItemData>, bytes: Range<usize>) {
    assert_eq!(layout.len(), 1);
    layout.data.lines[0].item_range = 0..items.len();
    layout.data.lines[0].text_range = bytes;
    layout.data.line_items = items;
}

fn assert_navigation(
    layout: &Layout<ColorBrush>,
    logical: &[Range<usize>],
    visual: &[Range<usize>],
) {
    for (expected, is_visual) in [(logical, false), (visual, true)] {
        let mut current = Cluster::from_byte_index(layout, expected[0].start);
        for (index, range) in expected.iter().enumerate() {
            let cluster = current.expect("every expected cluster is visited exactly once");
            assert_eq!(cluster.text_range(), *range);
            let path = cluster.path();
            assert_eq!(path.cluster(layout).unwrap().path(), path);
            assert_eq!(path.run(layout).unwrap().index(), path.run_index());
            assert_eq!(
                path.cluster(layout).unwrap().text_range(),
                cluster.text_range()
            );
            for byte in range.clone() {
                assert_eq!(Cluster::from_byte_index(layout, byte).unwrap().path(), path);
            }
            let previous = if is_visual {
                cluster.previous_visual()
            } else {
                cluster.previous_logical()
            };
            assert_eq!(
                previous.map(|cluster| cluster.text_range()),
                index.checked_sub(1).map(|index| expected[index].clone())
            );
            current = if is_visual {
                cluster.next_visual()
            } else {
                cluster.next_logical()
            };
        }
        assert!(current.is_none(), "no replay beyond the final cluster");

        let mut current = Cluster::from_byte_index(layout, expected.last().unwrap().start);
        for range in expected.iter().rev() {
            let cluster = current.expect("reverse traversal covers every cluster once");
            assert_eq!(cluster.text_range(), *range);
            current = if is_visual {
                cluster.previous_visual()
            } else {
                cluster.previous_logical()
            };
        }
        assert!(current.is_none(), "no replay before the first cluster");
    }
}

#[test]
fn repeated_original_run_slices_partition_bytes_and_keep_duplicate_box_ordinals() {
    let (mut layout, text) = build(|b| {
        b.push_text("  é");
        b.push_inline_box(inline_box(11.));
        b.push_inline_box(inline_box(17.));
        b.push_style_span(style());
        b.push_text("abc  ");
        b.pop_style_span();
    });
    assert_eq!(text, "éabc");
    assert_eq!(layout.data.shaped_text.runs().len(), 2);
    let shaped_before = layout.data.shaped_text.clone();
    let boxes = layout.inline_boxes().collect::<Vec<_>>();
    assert_eq!(boxes[0].id, boxes[1].id);
    assert_eq!(boxes[0].index, boxes[1].index);
    assert_eq!(boxes[0].index, 2);
    let prefix = atoms(&layout, 0);
    assert_eq!(
        prefix.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [0..2]
    );
    let a = atoms(&layout, 1);
    assert_eq!(
        a.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [2..3, 3..4, 4..5]
    );
    // Authored boxes create a shaping seam. Slice the same original second run
    // twice after both boxes, preserving their coincident normalized byte site.
    let ranges = [
        prefix[0].0.clone(),
        a[0].0.clone(),
        a[1].0.start..a[2].0.end,
    ];
    let mut output = Vec::new();
    let summary = append_line_items(
        &layout,
        &mut output,
        [
            text_item(0, ranges[0].clone()),
            box_item(0, 0),
            box_item(1, 0),
            text_item(1, ranges[1].clone()),
            text_item(1, ranges[2].clone()),
        ],
    );
    assert_eq!(summary.text_range, 0..5);
    assert_eq!(summary.last_item_kind, LayoutItemKind::TextRun);
    assert!(summary.committed_text_run);
    assert!(!summary.needs_reorder);
    assert_eq!(
        output.iter().map(|item| item.index).collect::<Vec<_>>(),
        [0, 0, 1, 1, 1]
    );
    let text_items = output
        .iter()
        .filter(|item| item.kind == LayoutItemKind::TextRun)
        .collect::<Vec<_>>();
    assert_eq!(
        text_items
            .iter()
            .map(|item| item.text_range.clone())
            .collect::<Vec<_>>(),
        [0..2, 2..3, 3..5]
    );
    assert_eq!(
        text_items
            .iter()
            .map(|item| item.shaped_cluster_range.clone())
            .collect::<Vec<_>>(),
        ranges
    );
    assert_eq!(
        ranges[1..]
            .iter()
            .flat_map(|range| range.clone())
            .collect::<Vec<_>>(),
        layout.data.shaped_text.runs()[1]
            .shaped_clusters_range
            .clone()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        ranges[0],
        layout.data.shaped_text.runs()[0].shaped_clusters_range
    );
    assert_eq!(output[1].text_range, 0..0);
    assert_eq!(output[2].shaped_cluster_range, 0..0);
    assert_eq!(
        layout.data.inline_boxes[output[1].index].inline_box.width,
        11.
    );
    assert_eq!(
        layout.data.inline_boxes[output[2].index].inline_box.width,
        17.
    );
    install(&mut layout, output, summary.text_range);
    assert_eq!(
        layout.data.shaped_text, shaped_before,
        "committing does not reshape or duplicate storage"
    );
    let runs = layout.get(0).unwrap().runs().collect::<Vec<_>>();
    assert_eq!(
        runs.iter().map(|run| run.index()).collect::<Vec<_>>(),
        [0, 3, 4]
    );
    assert_eq!(
        runs.iter()
            .map(|run| run.cluster_range())
            .collect::<Vec<_>>(),
        [0..1, 0..1, 1..3]
    );
    assert_eq!(
        runs.iter()
            .map(|run| run.shaped_text_run_index)
            .collect::<Vec<_>>(),
        [0, 1, 1]
    );
    let mut wrong_slice = runs[1].clusters().next().unwrap().path();
    wrong_slice.run_index = 4;
    assert!(
        wrong_slice.cluster(&layout).is_none(),
        "a path cannot escape its committed slice"
    );
    assert_navigation(
        &layout,
        &[0..2, 2..3, 3..4, 4..5],
        &[0..2, 2..3, 3..4, 4..5],
    );
}

#[test]
fn rtl_slices_reorder_without_changing_original_run_identity_or_direction() {
    let (mut layout, text) = build(|b| b.push_text("ابتث"));
    assert_eq!(text.len(), 8);
    assert_eq!(layout.data.shaped_text.runs().len(), 1);
    assert_eq!(
        layout.data.shaped_text.runs()[0].bidi_level,
        BidiLevel::new(1)
    );
    let a = atoms(&layout, 0);
    assert_eq!(
        a.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [0..2, 2..4, 4..6, 6..8]
    );
    let ranges = [a[0].0.clone(), a[1].0.clone(), a[2].0.start..a[3].0.end];
    let mut output = Vec::new();
    let summary = append_line_items(
        &layout,
        &mut output,
        ranges.iter().map(|range| text_item(0, range.clone())),
    );
    assert!(summary.needs_reorder);
    assert_eq!(summary.text_range, 0..8);
    assert!(
        output
            .iter()
            .all(|item| item.bidi_level == BidiLevel::new(1))
    );
    reorder_line_items(&mut output);
    assert_eq!(
        output
            .iter()
            .map(|item| item.text_range.clone())
            .collect::<Vec<_>>(),
        [4..8, 2..4, 0..2]
    );
    assert_eq!(
        output
            .iter()
            .map(|item| item.shaped_cluster_range.clone())
            .collect::<Vec<_>>(),
        [ranges[2].clone(), ranges[1].clone(), ranges[0].clone()]
    );
    assert!(output.iter().all(|item| item.index == 0));
    install(&mut layout, output, summary.text_range);
    let runs = layout.get(0).unwrap().runs().collect::<Vec<_>>();
    assert_eq!(
        runs.iter().map(|run| run.index()).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(
        runs.iter()
            .map(|run| run.cluster_range())
            .collect::<Vec<_>>(),
        [2..4, 1..2, 0..1]
    );
    assert!(
        runs.iter()
            .all(|run| run.is_rtl() && run.shaped_text_run_index == 0)
    );
    assert_navigation(
        &layout,
        &[0..2, 2..4, 4..6, 6..8],
        &[6..8, 4..6, 2..4, 0..2],
    );
}

#[test]
fn tracked_fi_and_combining_spans_commit_whole_atoms_once() {
    let (mut layout, text) = build(|b| {
        b.push_text("xf");
        b.push_style_span(style());
        b.push_text("ia");
        b.push_style_span(style());
        b.push_text("\u{301}");
        b.pop_style_span();
        b.pop_style_span();
        b.push_text("y");
    });
    assert_eq!(text, "xfia\u{301}y");
    assert_eq!(layout.data.shaped_text.runs().len(), 1);
    assert_eq!(layout.data.style_spans.len(), 2);
    let a = atoms(&layout, 0);
    assert_eq!(
        a.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [0..1, 1..3, 3..6, 6..7]
    );
    let original = layout.get(0).unwrap().runs().next().unwrap();
    let glyphs_before = original
        .glyphs_in(original.line_slice().shaped_clusters_range())
        .map(|g| g.id)
        .collect::<Vec<_>>();
    let clusters = original.clusters().collect::<Vec<_>>();
    assert!(
        clusters[1].is_ligature_start(),
        "fixture proves an unsplit fi ligature"
    );
    assert!(clusters[2].is_ligature_continuation());
    assert_eq!(
        clusters[3].text_range(),
        3..6,
        "combining sequence is one grapheme"
    );
    let mut output = Vec::new();
    let summary = append_line_items(
        &layout,
        &mut output,
        a.iter().map(|atom| text_item(0, atom.0.clone())),
    );
    assert_eq!(
        output
            .iter()
            .map(|item| item.text_range.clone())
            .collect::<Vec<_>>(),
        [0..1, 1..3, 3..6, 6..7]
    );
    install(&mut layout, output, summary.text_range);
    let glyphs_after = layout
        .get(0)
        .unwrap()
        .runs()
        .flat_map(|run| {
            run.glyphs_in(run.line_slice().shaped_clusters_range())
                .map(|g| g.id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        glyphs_after, glyphs_before,
        "each whole atom emits its existing glyphs once"
    );
    assert_eq!(
        layout
            .get(0)
            .unwrap()
            .runs()
            .map(|run| run.cluster_range())
            .collect::<Vec<_>>(),
        [0..1, 1..3, 3..4, 4..5]
    );
    assert_navigation(
        &layout,
        &[0..1, 1..2, 2..3, 3..6, 6..7],
        &[0..1, 1..2, 2..3, 3..6, 6..7],
    );
}

#[test]
fn leading_and_trailing_boxes_preserve_item_based_endpoint_flags() {
    let (mut layout, _) = build(|b| {
        b.push_inline_box(inline_box(11.));
        b.push_text("ab");
        b.push_inline_box(inline_box(17.));
    });
    assert_eq!(layout.data.shaped_text.runs().len(), 1);
    let a = atoms(&layout, 0);
    let mut output = Vec::new();
    let summary = append_line_items(
        &layout,
        &mut output,
        [
            box_item(0, 0),
            text_item(0, a[0].0.clone()),
            text_item(0, a[1].0.clone()),
            box_item(1, 0),
        ],
    );
    assert_eq!(summary.last_item_kind, LayoutItemKind::InlineBox);
    install(&mut layout, output, summary.text_range);
    let first = Cluster::from_byte_index(&layout, 0).unwrap();
    let last = Cluster::from_byte_index(&layout, 1).unwrap();
    // Endpoint flags currently count boxes as line items. Keep that existing public
    // behavior explicit; cluster navigation still skips the endpoint boxes.
    assert!(!first.is_start_of_line());
    assert!(!last.is_end_of_line());
    assert_navigation(&layout, &[0..1, 1..2], &[0..1, 1..2]);
}

#[test]
fn legacy_adapter_narrows_only_outer_runs_and_text_uses_original_bidi() {
    let (layout, text) = build(|b| {
        b.push_text("ab");
        b.push_style_span_with_shaping_boundaries(style(), true, true);
        b.push_text("cd");
        b.pop_style_span();
        b.push_text("ef");
        b.push_inline_box(inline_box(11.));
    });
    assert_eq!(text, "abcdef");
    let runs = layout.data.shaped_text.runs();
    assert_eq!(runs.len(), 3);
    let items = [
        LayoutItem {
            kind: LayoutItemKind::TextRun,
            index: 0,
            bidi_level: BidiLevel::new(3),
        },
        LayoutItem {
            kind: LayoutItemKind::TextRun,
            index: 1,
            bidi_level: BidiLevel::new(3),
        },
        LayoutItem {
            kind: LayoutItemKind::InlineBox,
            index: 0,
            bidi_level: BidiLevel::new(2),
        },
        LayoutItem {
            kind: LayoutItemKind::TextRun,
            index: 2,
            bidi_level: BidiLevel::new(3),
        },
    ];
    let mut output = Vec::new();
    let summary = append_line_items(
        &layout,
        &mut output,
        legacy_commit_items(
            &layout,
            &items,
            runs[0].shaped_clusters_range.start + 1..runs[2].shaped_clusters_range.end - 1,
        ),
    );
    assert_eq!(summary.text_range, 1..5);
    assert!(
        !summary.needs_reorder,
        "box levels alone retain the legacy reorder trigger"
    );
    assert_eq!(
        output
            .iter()
            .map(|item| item.text_range.clone())
            .collect::<Vec<_>>(),
        [1..2, 2..4, 0..0, 4..5]
    );
    assert_eq!(
        output[1].shaped_cluster_range,
        runs[1].shaped_clusters_range
    );
    assert_eq!(output[2].index, 0);
    assert_eq!(output[2].bidi_level, BidiLevel::new(2));
    assert!(
        output
            .iter()
            .filter(|item| item.kind == LayoutItemKind::TextRun)
            .all(|item| item.bidi_level == BidiLevel::new(0)),
        "adapter text levels cannot override shaped direction"
    );

    output.clear();
    let summary = append_line_items(
        &layout,
        &mut output,
        legacy_commit_items(
            &layout,
            &items,
            runs[0].shaped_clusters_range.end..runs[2].shaped_clusters_range.end,
        ),
    );
    assert_eq!(summary.text_range, 2..6);
    assert_eq!(
        output.iter().map(|item| item.index).collect::<Vec<_>>(),
        [1, 0, 2],
        "fully consumed preceding run is not replayed"
    );
}

#[test]
fn legacy_empty_and_consumed_commits_keep_sentinel_and_last_emitted_kind() {
    let (layout, _) = build(|b| {
        b.push_text("abc");
        b.push_inline_box(inline_box(11.));
    });
    let range = layout.data.shaped_text.runs()[0]
        .shaped_clusters_range
        .clone();
    let mut output = Vec::new();
    let run = LayoutItem {
        kind: LayoutItemKind::TextRun,
        index: 0,
        bidi_level: BidiLevel::new(0),
    };
    for items in [Vec::new(), alloc::vec![run.clone()]] {
        let summary = append_line_items(
            &layout,
            &mut output,
            legacy_commit_items(&layout, &items, range.end..range.end),
        );
        assert!(output.is_empty());
        assert_eq!(summary.text_range, usize::MAX..0);
        assert_eq!(summary.last_item_kind, LayoutItemKind::TextRun);
        assert!(!summary.committed_text_run);
        assert!(!summary.needs_reorder);
    }
    let items = [
        LayoutItem {
            kind: LayoutItemKind::InlineBox,
            index: 0,
            bidi_level: BidiLevel::new(1),
        },
        run,
    ];
    let summary = append_line_items(
        &layout,
        &mut output,
        legacy_commit_items(&layout, &items, range.end..range.end),
    );
    assert_eq!(output.len(), 1);
    assert_eq!(summary.text_range, usize::MAX..0);
    assert_eq!(summary.last_item_kind, LayoutItemKind::InlineBox);
    assert!(!summary.committed_text_run);
    assert!(!summary.needs_reorder);

    // Explicit empty slices are valid coverage, unlike legacy consumed-run replay.
    let summary = append_line_items(
        &layout,
        &mut output,
        [text_item(0, range.start + 1..range.start + 1)],
    );
    assert_eq!(
        output.len(),
        2,
        "append preserves preceding committed items"
    );
    assert_eq!(summary.text_range, 1..1);
    assert_eq!(summary.last_item_kind, LayoutItemKind::TextRun);
    assert!(summary.committed_text_run);
    assert_eq!(
        output[1].shaped_cluster_range,
        range.start + 1..range.start + 1
    );
    let summary = append_line_items(&layout, &mut output, [text_item(0, range.end..range.end)]);
    assert_eq!(output.len(), 3);
    assert_eq!(summary.text_range, 3..3);
    assert!(summary.committed_text_run);
    assert_eq!(output[2].text_range, 3..3);
}

#[test]
fn explicit_commits_reject_ranges_crossing_original_runs_or_shaped_bounds() {
    let (layout, _) = build(|b| {
        b.push_text("ab");
        b.push_style_span_with_shaping_boundaries(style(), true, false);
        b.push_text("cd");
        b.pop_style_span();
    });
    let runs = layout.data.shaped_text.runs();
    assert_eq!(runs.len(), 2);
    for range in [
        runs[0].shaped_clusters_range.start..runs[1].shaped_clusters_range.end,
        runs[1].shaped_clusters_range.end..runs[1].shaped_clusters_range.end + 1,
        runs[0].shaped_clusters_range.end..runs[0].shaped_clusters_range.start,
    ] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            append_line_items(&layout, &mut Vec::new(), [text_item(0, range)])
        }));
        assert!(
            result.is_err(),
            "explicit ranges must be in one original run"
        );
    }
}

#[test]
fn explicit_empty_slice_rejects_a_proven_interior_combining_atom_endpoint() {
    let (layout, text) = build(|b| {
        b.push_text("x");
        b.push_style_span(style());
        b.push_text("\u{301}");
        b.pop_style_span();
        b.push_text("y");
    });
    assert_eq!(text, "x\u{301}y");
    assert_eq!(layout.data.shaped_text.runs().len(), 1);
    let a = atoms(&layout, 0);
    assert_eq!(
        a.iter().map(|a| a.1.clone()).collect::<Vec<_>>(),
        [0..3, 3..4]
    );
    // Unlike a + acute, x + acute has no precomposed form. Prove that the
    // bundled font emits separate shaped clusters within one whole grapheme.
    let interior = a[0].0.start + 1;
    assert!(
        interior < a[0].0.end,
        "fixture must contain an interior shaped cluster"
    );
    let cluster = layout.data.shaped_text.shaped_clusters()[interior as usize];
    assert!(!cluster.is_grapheme_start());
    assert_eq!(cluster.chars_range().start, 1);
    let mut output = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        append_line_items(&layout, &mut output, [text_item(0, interior..interior)])
    }));
    let panic = result.expect_err("empty slices must still have whole-atom endpoints");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert!(message.unwrap().contains("whole-atom boundaries"));
    assert!(
        output.is_empty(),
        "invalid coverage must not append an item"
    );
}
