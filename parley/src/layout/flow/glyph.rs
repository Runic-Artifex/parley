// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Positioned glyph traversal with the existing per-style accumulation policy.

use super::super::line::{
    AtomIter, GlyphRun, Line, LineItem, PositionedInlineBox, PositionedLayoutItem,
};
use crate::{InlineBoxKind, style::Brush};
use core::ops::Range;

#[derive(Clone)]
pub(crate) struct GlyphFlow<'a, B: Brush> {
    line: Line<'a, B>,
    item_index: usize,
    /// Iterator over the visual atoms of the run at `item_index`, positioned at the first atom
    /// not yet yielded as part of a glyph run. `None` before entering a run.
    atoms: Option<AtomIter<'a>>,
    offset: f32,
}

impl<'a, B: Brush> GlyphFlow<'a, B> {
    pub(super) fn new(line: Line<'a, B>) -> Self {
        Self {
            line,
            item_index: 0,
            atoms: None,
            offset: 0.,
        }
    }
}

impl<'a, B: Brush> Iterator for GlyphFlow<'a, B> {
    type Item = PositionedLayoutItem<'a, B>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self.line.item(self.item_index)?;
            match item {
                LineItem::InlineBox(inline_box) => {
                    let x = self.offset
                        + self.line.data.metrics.inline_min_coord
                        + self.line.data.metrics.offset;
                    let box_index = self.line.layout.data.line_items
                        [self.line.data.item_range.start + self.item_index]
                        .index;
                    let y = self.line.inline_box_top(box_index);

                    self.item_index += 1;
                    if inline_box.kind == InlineBoxKind::InFlow {
                        self.offset += inline_box.width;
                    }
                    return Some(PositionedLayoutItem::InlineBox(PositionedInlineBox {
                        x,
                        y,
                        width: inline_box.width,
                        height: inline_box.height,
                        baseline: inline_box.baseline,
                        id: inline_box.id,
                        kind: inline_box.kind,
                    }));
                }
                LineItem::Run(run) => {
                    // TODO: style indices are taken from the atom's first character, matching
                    // `Cluster::style_index`. We could get the style index from `parley_engine`'s
                    // `ShapedCluster` instead, which would be somewhat finer-grained.
                    let atoms = self.atoms.get_or_insert_with(|| AtomIter::new(run));

                    // Styles are uniform within an atom, so glyph runs always end on atom
                    // boundaries and the iterator only needs to resume at atom granularity.
                    // Atoms without glyphs don't take part.
                    let mut advance = 0.0;
                    let mut glyph_count = 0;
                    // The style index and shaped cluster range of the glyph run being accumulated.
                    let mut glyph_run: Option<(u16, Range<u32>)> = None;
                    while let Some(atom) = atoms.peek() {
                        let (atom_glyph_count, atom_advance) = atoms.measure(&atom);
                        if atom_glyph_count != 0 {
                            let atom_style_index = atom.characters()[0].style_index;
                            let atom_clusters = atom.shaped_clusters_range();
                            match &mut glyph_run {
                                Some((style_index, shaped_clusters)) => {
                                    if *style_index != atom_style_index {
                                        break;
                                    }
                                    // In case the run is RTL, atoms are visited in reverse logical
                                    // order, so just grow both sides of the range.
                                    shaped_clusters.start =
                                        shaped_clusters.start.min(atom_clusters.start);
                                    shaped_clusters.end =
                                        shaped_clusters.end.max(atom_clusters.end);
                                }
                                None => glyph_run = Some((atom_style_index, atom_clusters)),
                            }
                            glyph_count += atom_glyph_count;
                            advance += atom_advance;
                        }
                        atoms.consume();
                    }

                    if let Some((style_index, shaped_clusters)) = glyph_run {
                        debug_assert_eq!(
                            run.glyphs_in(shaped_clusters.clone()).count(),
                            glyph_count,
                            "The calculated shaped cluster range of this glyph run must hold exactly the number of glyphs we counted above."
                        );
                        let offset = self.offset;
                        self.offset += advance;
                        return Some(PositionedLayoutItem::GlyphRun(GlyphRun {
                            run,
                            style_index,
                            shaped_clusters,
                            offset: offset
                                + self.line.data.metrics.inline_min_coord
                                + self.line.data.metrics.offset,
                            baseline: self.line.style_baseline(style_index),
                            advance,
                        }));
                    }
                    // Any atoms left have no glyphs and thus nothing to yield.
                    self.atoms = None;
                    self.item_index += 1;
                }
            }
        }
    }
}
