// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Borrowed visual traversal with each consumer's existing accumulation policy.

use super::line::{AtomIter, Line, LineItem};
use super::run::Run;
use crate::{InlineBox, InlineBoxKind, style::Brush};
use parley_engine::Atom;

#[derive(Clone)]
pub(crate) struct FlowAtom<'a, B: Brush> {
    pub(crate) run: Run<'a, B>,
    pub(crate) atom: Atom<'a>,
    pub(crate) item_index: usize,
    pub(crate) x: f32,
    pub(crate) advance: f32,
    pub(crate) glyph_count: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct FlowBox<'a> {
    pub(crate) item_index: usize,
    pub(crate) box_index: usize,
    pub(crate) inline_box: &'a InlineBox,
    pub(crate) x: f32,
    pub(crate) advance: f32,
    pub(crate) is_rtl: bool,
}

#[derive(Clone)]
pub(crate) enum ContentItem<'a, B: Brush> {
    Atom(FlowAtom<'a, B>),
    InlineBox(FlowBox<'a>),
}

/// Whole visual atoms and boxes, positioned with per-atom `f32` accumulation.
///
/// Glyphless atoms retain their position but reserve no advance. Only in-flow
/// boxes advance the cursor, matching ordinary-span content bounds.
#[derive(Clone)]
pub(crate) struct ContentFlow<'a, B: Brush> {
    line: Line<'a, B>,
    item_index: usize,
    current_run: Option<(Run<'a, B>, AtomIter<'a>)>,
    x: f32,
}

impl<'a, B: Brush> ContentFlow<'a, B> {
    pub(crate) fn new(line: Line<'a, B>) -> Self {
        let x = line.data.metrics.inline_min_coord + line.data.metrics.offset;
        Self {
            line,
            item_index: 0,
            current_run: None,
            x,
        }
    }
}

impl<'a, B: Brush> Iterator for ContentFlow<'a, B> {
    type Item = ContentItem<'a, B>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((run, atoms)) = &mut self.current_run {
                if let Some(atom) = atoms.peek() {
                    let (glyph_count, measured_advance) = atoms.measure(&atom);
                    let advance = if glyph_count == 0 {
                        0.
                    } else {
                        measured_advance
                    };
                    let item = FlowAtom {
                        run: *run,
                        atom,
                        item_index: self.item_index,
                        x: self.x,
                        advance,
                        glyph_count,
                    };
                    self.x += advance;
                    atoms.consume();
                    return Some(ContentItem::Atom(item));
                }
                self.current_run = None;
                self.item_index += 1;
            }

            match self.line.item(self.item_index)? {
                LineItem::Run(run) => {
                    self.current_run = Some((run, AtomIter::new(run)));
                }
                LineItem::InlineBox(inline_box) => {
                    let line_item = &self.line.layout.data.line_items
                        [self.line.data.item_range.start + self.item_index];
                    let advance = if inline_box.kind == InlineBoxKind::InFlow {
                        inline_box.width
                    } else {
                        0.
                    };
                    let item = FlowBox {
                        item_index: self.item_index,
                        box_index: line_item.index,
                        inline_box,
                        x: self.x,
                        advance,
                        is_rtl: line_item.is_rtl(),
                    };
                    // Content bounds never added an out-of-flow box's zero advance.
                    // Keep the operation itself absent, including for signed zero.
                    if inline_box.kind == InlineBoxKind::InFlow {
                        self.x += advance;
                    }
                    self.item_index += 1;
                    return Some(ContentItem::InlineBox(item));
                }
            }
        }
    }
}

/// Box positions with one `Run::advance` addition per preceding text run.
///
/// This preserves the grouping used by empty-span atomic anchors. Out-of-flow
/// boxes reserve zero advance, which is still added to the cursor.
#[derive(Clone)]
pub(crate) struct BoxFlow<'a, B: Brush> {
    line: Line<'a, B>,
    item_index: usize,
    x: f32,
}

impl<'a, B: Brush> BoxFlow<'a, B> {
    pub(crate) fn new(line: Line<'a, B>) -> Self {
        let x = line.data.metrics.inline_min_coord + line.data.metrics.offset;
        Self {
            line,
            item_index: 0,
            x,
        }
    }
}

impl<'a, B: Brush> Iterator for BoxFlow<'a, B> {
    type Item = FlowBox<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.line.item(self.item_index)? {
                LineItem::Run(run) => self.x += run.advance(),
                LineItem::InlineBox(inline_box) => {
                    let line_item = &self.line.layout.data.line_items
                        [self.line.data.item_range.start + self.item_index];
                    let advance = if inline_box.kind == InlineBoxKind::InFlow {
                        inline_box.width
                    } else {
                        0.
                    };
                    let item = FlowBox {
                        item_index: self.item_index,
                        box_index: line_item.index,
                        inline_box,
                        x: self.x,
                        advance,
                        is_rtl: line_item.is_rtl(),
                    };
                    self.x += advance;
                    self.item_index += 1;
                    return Some(item);
                }
            }
            self.item_index += 1;
        }
    }
}
