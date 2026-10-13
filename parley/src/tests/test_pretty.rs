// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Pretty line breaking (`text-wrap-style: pretty`).
//!
//! The expected lines were measured in Chromium 153.0.8010.12 with the same font (Roboto Regular
//! at 16px, `@font-face` from `parley_dev`) and widths, with `text-wrap: wrap` and
//! `text-wrap: pretty`.

use alloc::string::String;
use alloc::vec::Vec;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    BalanceOptions, FontFamily, IndentOptions, Layout, LayoutContext, TextStyle, WhiteSpaceCollapse,
};

fn layout(
    text: &str,
    font_size: f32,
    white_space_collapse: WhiteSpaceCollapse,
) -> Layout<ColorBrush> {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let style = TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size,
        white_space_collapse,
        ..TextStyle::default()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &style);
    builder.push_text(text);
    builder.build().0
}

/// The words of each line, single-spaced as Chromium's lines are compared.
fn lines(layout: &Layout<ColorBrush>, text: &str) -> Vec<String> {
    layout
        .lines()
        .map(|line| {
            text[line.text_range()]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect()
}

struct Case {
    text: &'static str,
    width: f32,
    white_space_collapse: WhiteSpaceCollapse,
    indent: f32,
    justify: bool,
    /// The device pixel ratio: the layout is in device pixels.
    zoom: f32,
}

impl Case {
    fn new(text: &'static str, width: f32) -> Self {
        Self {
            text,
            width,
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            indent: 0.,
            justify: false,
            zoom: 1.,
        }
    }

    fn break_lines(&self, pretty: bool) -> (Vec<String>, bool) {
        let mut layout = layout(self.text, 16. * self.zoom, self.white_space_collapse);
        if self.indent != 0. {
            layout.set_text_indent(self.indent * self.zoom, IndentOptions::default());
        }
        let width = self.width * self.zoom;
        let applied = if pretty {
            layout.break_all_lines_pretty(
                Some(width),
                BalanceOptions {
                    zoom: self.zoom,
                    justify: self.justify,
                },
            )
        } else {
            layout.break_all_lines(Some(width));
            false
        };
        (lines(&layout, self.text), applied)
    }

    /// Check the greedy and the pretty lines against Chromium's, and whether the score line
    /// breaker applied.
    fn check(&self, greedy: &[&str], pretty: &[&str], applies: bool) {
        assert_eq!(self.break_lines(false).0, greedy, "greedy lines");
        let (lines, applied) = self.break_lines(true);
        assert_eq!(lines, pretty, "pretty lines");
        assert_eq!(applied, applies, "whether the score line breaker applied");
    }
}

#[test]
fn pretty_leaves_one_line_unchanged() {
    let lines = ["Pretty text stays on one line"];
    Case::new("Pretty text stays on one line", 400.).check(&lines, &lines, false);
}

#[test]
fn pretty_avoids_a_short_last_word() {
    Case::new(
        "Balanced headings avoid a lonely last word at the end",
        360.,
    )
    .check(
        &["Balanced headings avoid a lonely last word at the", "end"],
        &["Balanced headings avoid a lonely last word at", "the end"],
        true,
    );
    Case::new(
        "The quick brown fox jumps over the lazy dog and keeps running far away",
        253.,
    )
    .check(
        &[
            "The quick brown fox jumps over the",
            "lazy dog and keeps running far",
            "away",
        ],
        &[
            "The quick brown fox jumps over",
            "the lazy dog and keeps running",
            "far away",
        ],
        true,
    );
}

/// Only the last four lines of a paragraph are rebroken.
#[test]
fn pretty_breaks_the_last_four_lines() {
    Case::new(
        "Chromium optimizes only the last four lines of a paragraph when it lays out text with pretty wrapping, so the earlier lines of this long paragraph keep their greedy breaks and only the end moves",
        160.,
    )
    .check(
        &[
            "Chromium optimizes",
            "only the last four lines",
            "of a paragraph when it",
            "lays out text with",
            "pretty wrapping, so",
            "the earlier lines of this",
            "long paragraph keep",
            "their greedy breaks",
            "and only the end",
            "moves",
        ],
        &[
            "Chromium optimizes",
            "only the last four lines",
            "of a paragraph when it",
            "lays out text with",
            "pretty wrapping, so",
            "the earlier lines of this",
            "long paragraph",
            "keep their greedy",
            "breaks and only the",
            "end moves",
        ],
        true,
    );
}

/// A last line with a break opportunity, or of a third of the width or more, stays greedy.
#[test]
fn pretty_needs_a_single_short_last_word() {
    let lines = [
        "The quick brown fox jumps",
        "over the lazy dog and keeps",
        "running far away",
    ];
    Case::new(
        "The quick brown fox jumps over the lazy dog and keeps running far away",
        200.,
    )
    .check(&lines, &lines, false);
    let lines = [
        "An indented first",
        "line changes the",
        "width that the first",
        "line of the window",
        "is scored against",
    ];
    Case::new(
        "An indented first line changes the width that the first line of the window is scored against",
        140.,
    )
    .check(&lines, &lines, false);
}

/// Each paragraph is optimized on its own.
#[test]
fn pretty_breaks_each_paragraph() {
    let mut case = Case::new(
        "The first paragraph also avoids an orphan at its end\nand the second paragraph after the forced break does so too",
        170.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
    case.check(
        &[
            "The first paragraph also",
            "avoids an orphan at its",
            "end",
            "and the second",
            "paragraph after the",
            "forced break does so",
            "too",
        ],
        &[
            "The first paragraph also",
            "avoids an orphan at",
            "its end",
            "and the second",
            "paragraph after the",
            "forced break does",
            "so too",
        ],
        true,
    );
}

/// The first line's text indent narrows the width it is scored against.
#[test]
fn pretty_scores_against_the_indented_first_line() {
    let mut case = Case::new(
        "An indented first line changes the width that the first line of the window is scored against",
        230.,
    );
    case.indent = 30.;
    case.check(
        &[
            "An indented first line",
            "changes the width that the first",
            "line of the window is scored",
            "against",
        ],
        &[
            "An indented first line",
            "changes the width that the",
            "first line of the window is",
            "scored against",
        ],
        true,
    );
}

/// Justified text has no line penalty.
#[test]
fn pretty_scores_justified_text() {
    let mut case = Case::new(
        "Justified pretty text has no line penalty but still keeps a lonely word off its last line",
        190.,
    );
    case.justify = true;
    case.check(
        &[
            "Justified pretty text has no",
            "line penalty but still keeps",
            "a lonely word off its last",
            "line",
        ],
        &[
            "Justified pretty text has",
            "no line penalty but still",
            "keeps a lonely word off its",
            "last line",
        ],
        true,
    );
}

#[test]
fn pretty_breaks_preserved_spaces() {
    let mut case = Case::new(
        "Preserved  spaces  wrap  as  well  and  pretty  still  keeps  a  lonely  word  off  the  end",
        150.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::Preserve;
    case.check(
        &[
            "Preserved spaces",
            "wrap as well and",
            "pretty still keeps a",
            "lonely word off the",
            "end",
        ],
        &[
            "Preserved spaces",
            "wrap as well and",
            "pretty still keeps",
            "a lonely word off",
            "the end",
        ],
        true,
    );
}

/// `break-spaces`, a preserved tab and an overflowing line keep the lines greedy.
#[test]
fn pretty_leaves_disabled_paragraphs_greedy() {
    let mut case = Case::new(
        "Balanced headings avoid a lonely last word at the end",
        360.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    let lines = ["Balanced headings avoid a lonely last word at the", "end"];
    case.check(&lines, &lines, false);

    // Chromium leaves this text greedy at every width from 150px to 420px, among them those
    // where its last line is "end" (190px to 203px). Parley does not place tabs at tab stops,
    // so only the greedy fallback is compared here.
    let mut case = Case::new(
        "Balanced headings avoid a lonely\tlast word at the end",
        195.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let (greedy, _) = case.break_lines(false);
    assert_eq!(case.break_lines(true), (greedy, false), "a preserved tab");

    let lines = [
        "The",
        "Pneumonoultramicroscopicsilicovolcanoconiosis",
        "word overflows the narrow line at its",
        "end",
    ];
    Case::new(
        "The Pneumonoultramicroscopicsilicovolcanoconiosis word overflows the narrow line at its end",
        260.,
    )
    .check(&lines, &lines, false);
}

/// In device pixels at a device pixel ratio of 2, as Blink lays out with the zoom.
#[test]
fn pretty_breaks_device_pixels() {
    let mut case = Case::new(
        "Pretty text avoids leaving a single short word alone on the last line of the paragraph",
        290.,
    );
    case.zoom = 2.;
    case.check(
        &[
            "Pretty text avoids leaving a single short",
            "word alone on the last line of the",
            "paragraph",
        ],
        &[
            "Pretty text avoids leaving a single",
            "short word alone on the last line of",
            "the paragraph",
        ],
        true,
    );
}
