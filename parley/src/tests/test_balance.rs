// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Balanced line breaking (`text-wrap-style: balance`).
//!
//! The expected lines were measured in Chromium 153.0.8010.12 with the same font (Roboto Regular
//! at 16px, `@font-face` from `parley_dev`) and widths, with `text-wrap: wrap` and
//! `text-wrap: balance`.

use alloc::string::String;
use alloc::vec::Vec;

use super::utils::ColorBrush;
use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    Alignment, AlignmentOptions, BalanceOptions, FontFamily, IndentOptions, Layout, LayoutContext,
    TextStyle, WhiteSpaceCollapse,
};

fn layout(text: &str, white_space_collapse: WhiteSpaceCollapse) -> Layout<ColorBrush> {
    let mut fcx = create_font_context();
    let mut lcx = LayoutContext::new();
    let style = TextStyle {
        font_family: FontFamily::from(FONT_FAMILY_LIST),
        font_size: 16.,
        white_space_collapse,
        ..TextStyle::default()
    };
    let mut builder = lcx.tree_builder(&mut fcx, 1., true, &style);
    builder.push_text(text);
    builder.build().0
}

fn lines(layout: &Layout<ColorBrush>, text: &str) -> Vec<String> {
    layout
        .lines()
        .map(|line| String::from(text[line.text_range()].trim()))
        .filter(|line| !line.is_empty())
        .collect()
}

struct Case {
    text: &'static str,
    width: f32,
    white_space_collapse: WhiteSpaceCollapse,
    indent: f32,
    justify: bool,
}

impl Case {
    fn new(text: &'static str, width: f32) -> Self {
        Self {
            text,
            width,
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            indent: 0.,
            justify: false,
        }
    }

    fn break_lines(&self, balance: bool) -> (Vec<String>, bool) {
        let mut layout = layout(self.text, self.white_space_collapse);
        if self.indent != 0. {
            layout.set_text_indent(self.indent, IndentOptions::default());
        }
        let balanced = if balance {
            layout.break_all_lines_balanced(
                Some(self.width),
                BalanceOptions {
                    zoom: 1.,
                    justify: self.justify,
                },
            )
        } else {
            layout.break_all_lines(Some(self.width));
            false
        };
        (lines(&layout, self.text), balanced)
    }

    /// Check the greedy and the balanced lines against Chromium's, and whether balancing applied.
    fn check(&self, greedy: &[&str], balanced: &[&str], applies: bool) {
        assert_eq!(self.break_lines(false).0, greedy, "greedy lines");
        let (lines, applied) = self.break_lines(true);
        assert_eq!(lines, balanced, "balanced lines");
        assert_eq!(applied, applies, "whether balancing applied");
    }
}

#[test]
fn balance_leaves_one_line_unchanged() {
    Case::new("Balanced text stays on one line", 400.).check(
        &["Balanced text stays on one line"],
        &["Balanced text stays on one line"],
        false,
    );
}

#[test]
fn balance_scores_three_lines() {
    Case::new(
        "The quick brown fox jumps over the lazy dog and keeps running far away",
        220.,
    )
    .check(
        &[
            "The quick brown fox jumps",
            "over the lazy dog and keeps",
            "running far away",
        ],
        &[
            "The quick brown fox jumps",
            "over the lazy dog and",
            "keeps running far away",
        ],
        true,
    );
}

#[test]
fn balance_scores_two_lines() {
    Case::new("Balanced headings avoid a lonely last word", 260.).check(
        &["Balanced headings avoid a lonely", "last word"],
        &["Balanced headings", "avoid a lonely last word"],
        true,
    );
}

/// With fewer than three break opportunities the score line breaker does not apply, and the
/// available width is bisected instead.
#[test]
fn balance_bisects_with_two_opportunities() {
    Case::new("Internationalization standardization procedures", 330.).check(
        &["Internationalization standardization", "procedures"],
        &["Internationalization", "standardization procedures"],
        true,
    );
}

/// Chromium balances up to six lines.
#[test]
fn balance_scores_six_lines() {
    Case::new(
        "Chromium balances paragraphs of up to six lines and leaves longer ones greedy, so this one which wraps onto six lines is balanced",
        200.,
    )
    .check(
        &[
            "Chromium balances",
            "paragraphs of up to six",
            "lines and leaves longer",
            "ones greedy, so this one",
            "which wraps onto six lines",
            "is balanced",
        ],
        &[
            "Chromium balances",
            "paragraphs of up to six",
            "lines and leaves longer",
            "ones greedy, so this",
            "one which wraps onto",
            "six lines is balanced",
        ],
        true,
    );
    Case::new(
        "A paragraph that wraps onto six lines at this width is still balanced by Chromium because six lines is the limit it accepts for this",
        190.,
    )
    .check(
        &[
            "A paragraph that wraps",
            "onto six lines at this width",
            "is still balanced by",
            "Chromium because six",
            "lines is the limit it accepts",
            "for this",
        ],
        &[
            "A paragraph that wraps",
            "onto six lines at this",
            "width is still balanced",
            "by Chromium because",
            "six lines is the limit",
            "it accepts for this",
        ],
        true,
    );
}

/// Chromium balances at most six lines; longer paragraphs stay greedy.
#[test]
fn balance_leaves_seven_lines_greedy() {
    let greedy = [
        "Chromium balances",
        "paragraphs of up to six",
        "lines and leaves longer",
        "ones greedy, so this one",
        "which wraps onto exactly",
        "six lines at this width is",
        "balanced as well",
    ];
    Case::new(
        "Chromium balances paragraphs of up to six lines and leaves longer ones greedy, so this one which wraps onto exactly six lines at this width is balanced as well",
        200.,
    )
    .check(&greedy, &greedy, false);
}

/// Only the paragraph before the first forced break is balanced, by the score line breaker.
#[test]
fn balance_scores_the_first_paragraph() {
    let mut case = Case::new(
        "The first paragraph is balanced before the forced break\nand the second paragraph after it is broken greedily as usual",
        240.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
    let greedy = [
        "The first paragraph is balanced",
        "before the forced break",
        "and the second paragraph after it",
        "is broken greedily as usual",
    ];
    case.check(&greedy, &greedy, true);

    let mut case = Case::new(
        "A first paragraph that balances its two lines nicely\nthen a greedy second paragraph that wraps as it always did before",
        300.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
    case.check(
        &[
            "A first paragraph that balances its two",
            "lines nicely",
            "then a greedy second paragraph that",
            "wraps as it always did before",
        ],
        &[
            "A first paragraph that",
            "balances its two lines nicely",
            "then a greedy second paragraph that",
            "wraps as it always did before",
        ],
        true,
    );
}

/// `white-space: break-spaces` disables the score line breaker; the width is bisected.
#[test]
fn balance_bisects_break_spaces() {
    let mut case = Case::new(
        "Break spaces disable the score breaker so the width is bisected",
        260.,
    );
    case.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    let lines = [
        "Break spaces disable the score",
        "breaker so the width is bisected",
    ];
    case.check(&lines, &lines, true);
}

/// A line that overflows disables both algorithms.
#[test]
fn balance_leaves_overflow_greedy() {
    let greedy = [
        "A",
        "Pneumonoultramicroscopicsilicovolcanoconiosis",
        "word overflows the line",
    ];
    Case::new(
        "A Pneumonoultramicroscopicsilicovolcanoconiosis word overflows the line",
        200.,
    )
    .check(&greedy, &greedy, false);
}

/// The first line's text indent narrows the width it is scored against.
#[test]
fn balance_scores_against_the_indented_first_line() {
    let mut case = Case::new(
        "An indented first line changes the widths that the balancing scores against",
        230.,
    );
    case.indent = 24.;
    let greedy = [
        "An indented first line",
        "changes the widths that the",
        "balancing scores against",
    ];
    case.check(&greedy, &greedy, true);
}

/// Justified text has no line penalty.
#[test]
fn balance_scores_justified_text() {
    let mut case = Case::new(
        "Justified balanced text has no line penalty but is balanced all the same here",
        230.,
    );
    case.justify = true;
    case.check(
        &[
            "Justified balanced text has no",
            "line penalty but is balanced all",
            "the same here",
        ],
        &[
            "Justified balanced text",
            "has no line penalty but is",
            "balanced all the same here",
        ],
        true,
    );
}

/// Lines are broken against the balanced width but aligned within the available width.
#[test]
fn balance_aligns_within_the_available_width() {
    let text = "Internationalization standardization procedures";
    let mut layout = layout(text, WhiteSpaceCollapse::Collapse);
    assert!(layout.break_all_lines_balanced(Some(330.), BalanceOptions::default()));
    layout.align(Alignment::End, AlignmentOptions::default());
    for line in layout.lines() {
        let metrics = line.metrics();
        let end = metrics.offset + metrics.advance - metrics.hanging_advance;
        assert!((end - 330.).abs() < 0.01, "line ends at {end}");
    }
}
