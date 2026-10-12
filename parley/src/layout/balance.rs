// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Balanced line breaking (`text-wrap-style: balance`), as Chromium lays it out.
//!
//! [CSS Text 4 § 6.2][spec] leaves the balancing algorithm to the user agent. This module follows
//! Blink's `LayoutNG` (Chromium 153) so that balanced text breaks where it does in Chromium:
//!
//! 1. Lines are first broken greedily.
//! 2. **Score line breaker** (`ScoreLineBreaker::BalanceBreakPoints`). If the first paragraph
//!    (up to the first forced break) has at most [`MAX_LINES_FOR_BALANCE`] lines, none of which
//!    overflows, and the layout has no preserved tabs, the break opportunities of that paragraph
//!    are scored as in Minikin's optimal line breaker: the square of each line's slack, plus a
//!    line penalty per line and an "orphans" penalty on the last opportunity. The breaks with the
//!    lowest total win, provided they produce as many lines as the greedy breaks; otherwise
//!    this step fails. Lines after the paragraph are broken greedily.
//! 3. **Bisection** (`ParagraphLineBreaker::AttemptParagraphBalancing`), if the score line
//!    breaker did not apply. Only for layouts without forced breaks: the available width is
//!    bisected, in `LayoutUnit`s (1/64 of a layout unit), between 80% of the average line width
//!    and the available width, down to the smallest width that does not add lines. Every line is
//!    then broken against that width, but aligned within the original one.
//!
//! Otherwise lines stay greedy.
//!
//! Sources (Chromium 153.0.8010.12, `third_party/blink/renderer/core/layout/`):
//! `inline/inline_layout_algorithm.cc` (`LineBreakStrategy`), `inline/score_line_breaker.cc`,
//! `inline/line_break_candidate.cc`, `inline/line_breaker.cc` (`AppendCandidates`,
//! `HandleOverflow`), `inline/paragraph_line_breaker.cc`, `inline/line_widths.cc`,
//! `inline/inline_items_builder.cc` (what disables either algorithm) and
//! `block_layout_algorithm.cc` (`kMaxLinesForBalance`).
//!
//! Positions are Parley's (unsnapped) advances, as for greedy breaking. Blink snaps a position
//! to `LayoutUnit` after each inline box edge and sums `LayoutUnit` item widths when it measures
//! lines for the bisection; those sub-1/64 differences are not modelled.
//!
//! [spec]: https://drafts.csswg.org/css-text-4/#text-wrap-style

use alloc::vec::Vec;

#[cfg(feature = "libm")]
#[allow(unused_imports)]
use core_maths::CoreFloat;
use parley_engine::shape::Whitespace;

use crate::layout::{BreakReason, Layout, YieldData};
use crate::style::Brush;
use crate::{InlineBoxKind, WhiteSpaceCollapse};

/// The maximum number of lines that are balanced (Blink's `kMaxLinesForBalance`).
pub const MAX_LINES_FOR_BALANCE: usize = 6;

/// Options for [`Layout::break_all_lines_balanced`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BalanceOptions {
    /// The effective zoom: layout units per CSS pixel (the device pixel ratio when the layout is
    /// in device pixels).
    pub zoom: f32,
    /// Whether the paragraph is justified (`text-align: justify`), which removes the line
    /// penalty.
    pub justify: bool,
}

impl Default for BalanceOptions {
    fn default() -> Self {
        Self {
            zoom: 1.,
            justify: false,
        }
    }
}

/// A position at which a line may end: before the atom starting at `cluster` in the item
/// `item`, or after the inline box `item - 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BreakKey {
    item: usize,
    cluster: u32,
}

impl BreakKey {
    /// The end of the paragraph, which the line breaker reaches without a break opportunity.
    const END: Self = Self {
        item: usize::MAX,
        cluster: u32::MAX,
    };

    pub(crate) fn before_atom(item: usize, cluster: u32) -> Self {
        Self { item, cluster }
    }

    /// `item` is the index of the item after the box.
    pub(crate) fn after_box(item: usize) -> Self {
        Self {
            item,
            cluster: u32::MAX,
        }
    }
}

/// How [`super::BreakLines`] breaks lines for balancing.
#[derive(Clone, Debug)]
pub(crate) enum Directive {
    /// Break nothing within the first paragraph and record its break opportunities.
    Collect,
    /// Fit every line against this advance (including the text indent) instead of the line's.
    Width(f32),
    /// End line `n` at `breaks[n]`; later lines are broken greedily.
    Breaks(Vec<BreakKey>),
}

/// A recorded break opportunity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    key: BreakKey,
    /// The line position if the line continues past the opportunity.
    pos_no_break: f32,
    /// The line position of the end of the content if the line ends here, which excludes
    /// hanging whitespace.
    pos_if_break: f32,
    /// The line's items, clusters and boxes up to the opportunity, which tell apart
    /// opportunities at different positions.
    progress: (usize, u32, usize),
}

/// Balancing state of a [`super::BreakLines`].
#[derive(Clone, Debug)]
pub(crate) struct BalanceState {
    directive: Directive,
    candidates: Vec<Candidate>,
    /// While collecting, the end of the content so far excluding hanging whitespace.
    pub(crate) content_end: f32,
}

impl BalanceState {
    fn new(directive: Directive) -> Self {
        Self {
            directive,
            candidates: Vec::new(),
            content_end: 0.,
        }
    }

    /// The advance that content of line `line` is fit against.
    #[inline(always)]
    pub(crate) fn fit_advance(&self, line: usize, max_advance: f32, line_indent: f32) -> f32 {
        match &self.directive {
            // Blink sets a "nearly max" available width for lines it ends at break points.
            Directive::Collect => f32::MAX,
            Directive::Breaks(breaks) if line < breaks.len() => f32::MAX,
            Directive::Breaks(_) => max_advance,
            Directive::Width(width) => width - line_indent,
        }
    }

    /// Whether line `line` ends at `key`.
    #[inline(always)]
    pub(crate) fn breaks_at(&self, line: usize, key: BreakKey) -> bool {
        match &self.directive {
            Directive::Breaks(breaks) => breaks.get(line) == Some(&key),
            _ => false,
        }
    }

    #[inline(always)]
    pub(crate) fn is_collecting(&self) -> bool {
        matches!(self.directive, Directive::Collect)
    }

    /// Record a break opportunity at line position `x`.
    pub(crate) fn collect(&mut self, key: BreakKey, x: f32, progress: (usize, u32, usize)) {
        if !self.is_collecting() {
            return;
        }
        // An opportunity after an inline box and one before the following atom are the same.
        if self
            .candidates
            .last()
            .is_some_and(|last| last.progress == progress)
        {
            return;
        }
        self.candidates.push(Candidate {
            key,
            pos_no_break: x,
            pos_if_break: self.content_end,
            progress,
        });
    }
}

/// Lengths in Blink's `LayoutUnit`: fixed point with 6 fractional bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LayoutUnit(i32);

impl LayoutUnit {
    const DENOMINATOR: f32 = 64.;

    /// `LayoutUnit::FromFloatRound`.
    fn from_f32_round(value: f32) -> Self {
        Self(saturate((value * Self::DENOMINATOR).round()))
    }

    /// `LayoutUnit::FromFloatCeil`.
    fn from_f32_ceil(value: f32) -> Self {
        Self(saturate((value * Self::DENOMINATOR).ceil()))
    }

    /// `LayoutUnit(float)`, which truncates.
    fn from_f32(value: f32) -> Self {
        Self(saturate((value * Self::DENOMINATOR).trunc()))
    }

    fn to_f32(self) -> f32 {
        self.0 as f32 / Self::DENOMINATOR
    }

    fn clamp_negative_to_zero(self) -> Self {
        Self(self.0.max(0))
    }

    /// `LayoutUnit::AddEpsilon`.
    fn add_epsilon(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl core::ops::Add for LayoutUnit {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl core::ops::Sub for LayoutUnit {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "saturated like Blink's LayoutUnit"
)]
fn saturate(raw: f32) -> i32 {
    if raw.is_nan() {
        0
    } else {
        raw.clamp(i32::MIN as f32, i32::MAX as f32) as i32
    }
}

/// What the greedy breaks of a line tell about balancing.
#[derive(Clone, Copy, Debug)]
struct GreedyLine {
    reason: BreakReason,
    /// The line overflows, which disables both balancing algorithms in Blink.
    overflowed: bool,
    /// The line width as Blink measures it: indent plus content, excluding hanging whitespace.
    width: f32,
    indent: f32,
    text_end: usize,
}

/// Plan balanced breaks for `layout`, whose lines have been broken greedily at `max_advance`.
pub(crate) fn plan<B: Brush>(
    layout: &mut Layout<B>,
    max_advance: f32,
    options: BalanceOptions,
    rebroken: &mut bool,
) -> Option<Directive> {
    let data = &layout.data;
    if !max_advance.is_finite()
        || max_advance >= f32::MAX
        || (data.text_len == 0 && data.inline_boxes.is_empty())
        || data.lines.is_empty()
    {
        return None;
    }
    // Floats are placed by the embedder and need line heights; neither algorithm models them.
    if data
        .inline_boxes
        .iter()
        .any(|b| b.inline_box.kind == InlineBoxKind::CustomOutOfFlow)
    {
        return None;
    }
    let zoom = if options.zoom > 0. { options.zoom } else { 1. };
    let available = LayoutUnit::from_f32_round(max_advance);
    let greedy: Vec<GreedyLine> = data
        .lines
        .iter()
        .map(|line| GreedyLine {
            reason: line.break_reason,
            overflowed: line.overflowed || line.break_reason == BreakReason::Emergency,
            width: line.indent + line.metrics.advance - line.metrics.hanging_advance,
            indent: line.indent,
            text_end: line.text_range.end,
        })
        .collect();

    // Blink appends a forced break (`DidAppendForcedBreak`) for preserved newlines and `<br>`,
    // and a control item for each preserved tab, which disables the score line breaker.
    let mut has_forced_break = false;
    let mut has_preserved_tab = false;
    let mut text_length = 0_usize;
    for character in data.shaped_text.characters() {
        text_length += if character.flags.len_utf8() == 4 {
            2
        } else {
            1
        };
        match character.whitespace {
            Whitespace::Newline => has_forced_break = true,
            Whitespace::Tab => {
                let style = &data.styles[usize::from(character.style_index)];
                if matches!(
                    style.white_space_collapse,
                    WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
                ) {
                    has_preserved_tab = true;
                }
            }
            _ => {}
        }
    }
    // Atomic inlines are U+FFFC in Blink's text content.
    text_length += data
        .inline_boxes
        .iter()
        .filter(|b| b.inline_box.kind == InlineBoxKind::InFlow)
        .count();
    let root_font_size = data.style_metrics.first().map(|m| m.font_size)?;
    let space_advance = data.root_space_advance;

    if !has_preserved_tab
        && let Some(breaks) = score_breaks(
            layout,
            &greedy,
            available,
            root_font_size,
            zoom,
            options,
            rebroken,
        )
    {
        return Some(Directive::Breaks(breaks));
    }
    if has_forced_break || available <= LayoutUnit(0) {
        return None;
    }
    bisect(
        layout,
        &greedy,
        max_advance,
        available,
        text_length,
        space_advance,
        zoom,
        rebroken,
    )
    .map(|width| Directive::Width(width.to_f32()))
}

/// `ScoreLineBreaker::BalanceBreakPoints`: the break of each line of the first paragraph.
fn score_breaks<B: Brush>(
    layout: &mut Layout<B>,
    greedy: &[GreedyLine],
    available: LayoutUnit,
    font_size: f32,
    zoom: f32,
    options: BalanceOptions,
    rebroken: &mut bool,
) -> Option<Vec<BreakKey>> {
    // `OptimalBreakPoints`: the greedy lines of the first paragraph.
    let mut num_lines = 0;
    for line in greedy {
        num_lines += 1;
        // `ShouldDisableScoreLineBreak`: overflow, and `white-space: break-spaces`.
        if line.overflowed {
            return None;
        }
        if matches!(line.reason, BreakReason::Explicit | BreakReason::None) {
            break;
        }
        if num_lines >= MAX_LINES_FOR_BALANCE {
            return None;
        }
    }
    if num_lines <= 1 {
        return None;
    }
    let data = &layout.data;
    let paragraph_end = greedy[num_lines - 1].text_end;
    if data.shaped_text.characters().iter().any(|character| {
        (character.text_byte_start as usize) < paragraph_end
            && data.styles[usize::from(character.style_index)].white_space_collapse
                == WhiteSpaceCollapse::BreakSpaces
    }) {
        return None;
    }
    let first_line_indent = LayoutUnit::from_f32(greedy[0].indent);

    // `ComputeCandidates`: a sentinel at the start, every break opportunity, and the end.
    *rebroken = true;
    let mut breaker = layout.break_lines();
    breaker.set_balance(BalanceState::new(Directive::Collect));
    breaker.state_mut().set_layout_max_advance(f32::MAX);
    breaker.state_mut().set_line_max_advance(f32::MAX);
    let Some(YieldData::LineBreak(end)) = breaker.break_next() else {
        return None;
    };
    let collected = breaker.take_balance()?;
    drop(breaker);

    struct Scored {
        key: BreakKey,
        pos_no_break: f32,
        pos_if_break: f32,
        penalty: f32,
    }
    let mut candidates = Vec::with_capacity(collected.candidates.len() + 2);
    candidates.push(Scored {
        key: BreakKey::END,
        pos_no_break: 0.,
        pos_if_break: 0.,
        penalty: 0.,
    });
    candidates.extend(collected.candidates.iter().map(|c| Scored {
        key: c.key,
        pos_no_break: c.pos_no_break,
        pos_if_break: c.pos_if_break,
        penalty: 0.,
    }));
    candidates.push(Scored {
        key: BreakKey::END,
        pos_no_break: end.advance,
        pos_if_break: collected.content_end,
        penalty: 0.,
    });

    // `Optimize`: not needed with fewer than three break opportunities.
    if candidates.len() < 3 + 2 {
        return None;
    }
    // Increase the penalty of the last opportunity to minimize typographic orphans. (Blink
    // continues to earlier opportunities only through hyphenated ones; Parley does not
    // hyphenate.)
    let last_opportunity = candidates.len() - 2;
    candidates[last_opportunity].penalty += 10000. * zoom;

    // `SetupParameters`, with Minikin's heuristics.
    let width_times_font_size = available.clamp_negative_to_zero().to_f32() * font_size / zoom;
    let line_penalty = if options.justify {
        0.
    } else {
        width_times_font_size * 2. * 2.
    };
    let available_to_fit = |line_index: usize| {
        let mut width = available;
        if line_index == 0 {
            width = width - first_line_indent;
        }
        width.clamp_negative_to_zero().add_epsilon().to_f32()
    };

    // `ComputeScores`.
    const SCORE_INFINITY: f32 = f32::MAX;
    const SCORE_OVERFULL: f32 = 1e12;
    struct Score {
        score: f32,
        prev_index: usize,
        line_index: usize,
    }
    let mut scores: Vec<Score> = Vec::with_capacity(candidates.len());
    scores.push(Score {
        score: 0.,
        prev_index: 0,
        line_index: 0,
    });
    let mut active = 0;
    for end in 1..candidates.len() {
        let end_candidate = &candidates[end];
        let mut best = SCORE_INFINITY;
        let mut best_prev_index = 0;

        let mut last_line_index = scores[active].line_index;
        let mut available_width = available_to_fit(last_line_index);
        let mut start_edge = end_candidate.pos_if_break - available_width;
        let mut best_hope = 0_f32;

        // Blink advances `active` within the loop; the loop's range is fixed at its start.
        let first_start = active;
        for start in first_start..end {
            let start_score = &scores[start];
            let line_index = start_score.line_index;
            if line_index != last_line_index {
                last_line_index = line_index;
                let new_available_width = available_to_fit(line_index);
                if new_available_width != available_width {
                    available_width = new_available_width;
                    start_edge = end_candidate.pos_if_break - available_width;
                    best_hope = 0.;
                }
            }
            let start_score_value = start_score.score;
            if start_score_value + best_hope >= best {
                continue;
            }
            let delta = candidates[start].pos_no_break - start_edge;

            // Blink's last-line exception (a penalty for a hyphen on the last line) does not
            // apply to balancing, so every overfull line scores alike.
            let width_score = if delta < 0. {
                SCORE_OVERFULL
            } else {
                delta * delta / zoom
            };
            if delta < 0. {
                active = start + 1;
            } else {
                best_hope = width_score;
            }
            let score = start_score_value + width_score;
            if score <= best {
                best = score;
                best_prev_index = start;
            }
        }

        scores.push(Score {
            score: best + end_candidate.penalty + line_penalty,
            prev_index: best_prev_index,
            line_index: scores[best_prev_index].line_index + 1,
        });
    }

    // `ComputeBreakPoints`.
    let mut breaks = Vec::new();
    let mut index = scores.len() - 1;
    while index > 0 {
        breaks.push(candidates[index].key);
        index = scores[index].prev_index;
    }
    breaks.reverse();

    // A different number of lines than the greedy breaks falls back.
    (breaks.len() == num_lines).then_some(breaks)
}

/// The outcome of `LineBreakResults::BreakLines`.
#[derive(PartialEq)]
enum Status {
    Finished,
    NotApplicable,
    MaxLinesExceeded,
}

/// `LineBreakResults::BreakLines` over the lines of a layout.
fn status(lines: &[GreedyLine], max_lines: usize) -> Status {
    for (index, line) in lines.iter().enumerate() {
        if line.overflowed {
            return Status::NotApplicable;
        }
        if index + 1 == lines.len() {
            return Status::Finished;
        }
        if index + 1 == max_lines {
            return Status::MaxLinesExceeded;
        }
    }
    Status::Finished
}

/// `ParagraphLineBreaker::AttemptParagraphBalancing`: the available width to break all lines
/// against.
fn bisect<B: Brush>(
    layout: &mut Layout<B>,
    greedy: &[GreedyLine],
    max_advance: f32,
    available: LayoutUnit,
    text_length: usize,
    space_advance: Option<f32>,
    zoom: f32,
    rebroken: &mut bool,
) -> Option<LayoutUnit> {
    // `EstimateNumLines`: skip text that is too long to balance, by the space width.
    if let Some(space_width) = space_advance.filter(|width| *width > 0.) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "Blink converts the quotient to an unsigned integer"
        )]
        let num_line_chars = (available.to_f32() / space_width) as usize;
        if num_line_chars == 0 {
            return None;
        }
        let estimated_num_lines = text_length.div_ceil(num_line_chars);
        if estimated_num_lines > MAX_LINES_FOR_BALANCE * 2 {
            return None;
        }
    }

    if status(greedy, MAX_LINES_FOR_BALANCE) != Status::Finished {
        return None;
    }
    let num_lines = greedy.len();
    if num_lines <= 1 {
        return None;
    }

    let epsilon = LayoutUnit::from_f32_ceil(zoom);
    let line_width_sum = greedy
        .iter()
        .map(|line| LayoutUnit::from_f32_ceil(line.width).clamp_negative_to_zero())
        .fold(LayoutUnit(0), |sum, width| sum + width);
    #[expect(clippy::cast_possible_truncation, reason = "at most six lines")]
    let avg_line_width = LayoutUnit(line_width_sum.0 / num_lines as i32);
    let min_available_width = LayoutUnit::from_f32_round(avg_line_width.to_f32() * 0.8);

    // `BisectAvailableWidth`.
    let mut upper = available;
    let mut lower = min_available_width;
    while lower + epsilon < upper {
        let middle = LayoutUnit((upper + lower).0 / 2);
        *rebroken = true;
        let mut breaker = layout.break_lines();
        breaker.set_balance(BalanceState::new(Directive::Width(middle.to_f32())));
        breaker.break_remaining(max_advance);
        let lines: Vec<GreedyLine> = layout
            .data
            .lines
            .iter()
            .map(|line| GreedyLine {
                reason: line.break_reason,
                overflowed: line.overflowed || line.break_reason == BreakReason::Emergency,
                width: 0.,
                indent: line.indent,
                text_end: line.text_range.end,
            })
            .collect();
        if status(&lines, num_lines) == Status::Finished {
            upper = middle;
        } else {
            lower = middle;
        }
    }
    Some(upper)
}

impl<B: Brush> Layout<B> {
    /// Breaks all lines with the specified maximum advance, balancing them as Chromium (Blink
    /// `LayoutNG`) lays out `text-wrap-style: balance`.
    ///
    /// Blink first scores the break opportunities of the first paragraph (up to the first forced
    /// break) if it has at most [`MAX_LINES_FOR_BALANCE`] lines, and otherwise bisects the
    /// available width of layouts without forced breaks. Neither applies when a line overflows.
    ///
    /// Returns whether the lines were balanced; otherwise they are broken greedily, as by
    /// [`Layout::break_all_lines`].
    pub fn break_all_lines_balanced(
        &mut self,
        max_advance: Option<f32>,
        options: BalanceOptions,
    ) -> bool {
        self.break_all_lines(max_advance);
        let Some(max_advance) = max_advance else {
            return false;
        };
        let mut rebroken = false;
        let Some(directive) = plan(self, max_advance, options, &mut rebroken) else {
            // Planning breaks lines differently to measure them; restore the greedy lines.
            if rebroken {
                self.break_all_lines(Some(max_advance));
            }
            return false;
        };
        let mut breaker = self.break_lines();
        breaker.set_balance(BalanceState::new(directive));
        breaker.break_remaining(max_advance);
        true
    }
}
