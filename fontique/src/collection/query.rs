// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Query support.

use crate::{Charmap, CharmapIndex};

use super::super::{Collection, SourceCache, matching::match_fonts};

use alloc::sync::Arc;
use alloc::vec::Vec;
use parlance::Script;
use smallvec::SmallVec;

use super::{
    super::{Attributes, Blob, FallbackKey, FamilyId, FamilyInfo, GenericFamily, Synthesis},
    Inner,
};

#[derive(Clone, Default)]
pub(super) struct QueryState {
    families: Vec<CachedFamily>,
    fallback_families: Vec<CachedFamily>,
}

impl QueryState {
    fn clear(&mut self) {
        self.families.clear();
        self.fallback_families.clear();
    }
}

/// State for font selection.
///
/// Instances of this can be obtained from [`Collection::query`].
pub struct Query<'a> {
    collection: &'a mut Inner,
    state: &'a mut QueryState,
    source_cache: &'a mut SourceCache,
    attributes: Attributes,
    fallbacks: Option<FallbackKey>,
}

impl<'a> Query<'a> {
    pub(super) fn new(collection: &'a mut Collection, source_cache: &'a mut SourceCache) -> Self {
        collection.query_state.clear();
        Self {
            collection: &mut collection.inner,
            state: &mut collection.query_state,
            source_cache,
            attributes: Attributes::default(),
            fallbacks: None,
        }
    }

    /// Sets the ordered sequence of families to match against.
    pub fn set_families<'f, I>(&mut self, families: I)
    where
        I: IntoIterator,
        I::Item: Into<QueryFamily<'f>>,
    {
        self.state.families.clear();
        for family in families {
            let family = family.into();
            match family {
                QueryFamily::Named(name) => {
                    if let Some(id) = self.collection.family_id(name) {
                        self.state.families.push(CachedFamily::new(id));
                    }
                }
                QueryFamily::Id(id) => {
                    self.state.families.push(CachedFamily::new(id));
                }
                QueryFamily::Generic(generic) => {
                    for id in self.collection.generic_families(generic) {
                        self.state.families.push(CachedFamily::new(id));
                    }
                }
            }
        }
    }

    /// Sets the primary attributes to match against.
    pub fn set_attributes(&mut self, attributes: Attributes) {
        if self.attributes != attributes {
            for family in &mut self.state.families {
                family.clear_fonts();
            }
            for family in &mut self.state.fallback_families {
                family.clear_fonts();
            }
            self.attributes = attributes;
        }
    }

    /// Sets the script and locale for fallback fonts.
    pub fn set_fallbacks(&mut self, key: impl Into<FallbackKey>) {
        let key = key.into();
        if self.fallbacks != Some(key) {
            self.state.fallback_families.clear();
            self.state.fallback_families.extend(
                self.collection
                    .fallback_families(key)
                    .map(CachedFamily::new),
            );
            // HACK: always add a Han font to the fallback list to capture
            // punctuation in the common script
            // See <https://github.com/linebender/parley/issues/597>
            self.state.fallback_families.extend(
                self.collection
                    .fallback_families(FallbackKey::new(Script::from_bytes(*b"Hani"), None))
                    .map(CachedFamily::new),
            );
            self.fallbacks = Some(key);
        }
    }

    /// Invokes the given callback with all fonts that match the current
    /// settings.
    ///
    /// Return [`QueryStatus::Stop`] to end iterating over the matching
    /// fonts or [`QueryStatus::Continue`] to continue iterating.
    pub fn matches_with<'q>(&'q mut self, mut f: impl FnMut(&'q QueryFont) -> QueryStatus) {
        for family in self
            .state
            .families
            .iter_mut()
            .chain(self.state.fallback_families.iter_mut())
        {
            match &mut family.family {
                Entry::Error => continue,
                Entry::Ok(..) => {}
                status @ Entry::Vacant => {
                    if let Some(info) = self.collection.family(family.id) {
                        *status = Entry::Ok(info);
                    } else {
                        *status = Entry::Error;
                        continue;
                    }
                }
            }
            let Entry::Ok(family_info) = &family.family else {
                continue;
            };
            let bucket = load_bucket(
                family_info,
                self.attributes,
                &mut family.best,
                self.source_cache,
            );
            let mut yielded_default = false;
            for font in bucket {
                if font.family.1 == family_info.default_font_index() {
                    yielded_default = true;
                }
                if f(font) == QueryStatus::Stop {
                    return;
                }
            }
            if yielded_default {
                continue;
            }
            if let Some(font) = load_font(
                family_info,
                self.attributes,
                &mut family.default,
                self.source_cache,
            ) && f(font) == QueryStatus::Stop
            {
                return;
            }
        }
    }

    /// Returns the [first available font] for the current settings: the first
    /// font that [`matches_with`](Self::matches_with) would yield whose
    /// `unicode-range` (see [`FontInfoOverride::unicode_range`]) includes U+0020
    /// SPACE. As in Chromium, when no font's range includes it, the first font.
    ///
    /// [first available font]: https://drafts.csswg.org/css-fonts-4/#first-available-font
    /// [`FontInfoOverride::unicode_range`]: crate::FontInfoOverride::unicode_range
    pub fn first_available_font(&mut self) -> Option<QueryFont> {
        let mut first = None;
        let mut found = None;
        self.matches_with(|font| {
            if first.is_none() {
                first = Some(font);
            }
            if font.covers(' ' as u32) {
                found = Some(font.clone());
                QueryStatus::Stop
            } else {
                QueryStatus::Continue
            }
        });
        found.or_else(|| first.cloned())
    }
}

impl Drop for Query<'_> {
    fn drop(&mut self) {
        self.state.clear();
    }
}

/// Determines whether a font query operation will continue.
///
/// See [`Query::matches_with`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum QueryStatus {
    /// Query should continue with the next font.
    Continue,
    /// Query should stop.
    Stop,
}

/// Family descriptor for a font query.
///
/// This allows [`Query::set_families`] to
/// take a variety of family types.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum QueryFamily<'a> {
    /// A named font family.
    Named(&'a str),
    /// An identifier for a font family.
    Id(FamilyId),
    /// A generic font family.
    Generic(GenericFamily),
}

impl<'a> From<&'a str> for QueryFamily<'a> {
    fn from(value: &'a str) -> Self {
        Self::Named(value)
    }
}

impl From<FamilyId> for QueryFamily<'static> {
    fn from(value: FamilyId) -> Self {
        Self::Id(value)
    }
}

impl From<GenericFamily> for QueryFamily<'static> {
    fn from(value: GenericFamily) -> Self {
        Self::Generic(value)
    }
}

/// Candidate font generated by a [`Query`].
#[derive(Clone, Debug)]
pub struct QueryFont {
    /// Family identifier and index of the font in the family font list.
    pub family: (FamilyId, usize),
    /// Blob containing the font data.
    pub blob: Blob<u8>,
    /// Index of a font in a font collection (`ttc`) file.
    pub index: u32,
    /// Synthesis suggestions for this font based on the requested attributes.
    pub synthesis: Synthesis,
    /// Data used for constructing a character map for this font.
    pub charmap_index: CharmapIndex,
    /// The inclusive code point ranges the font is used for, if it was
    /// registered with any (CSS `@font-face` `unicode-range`).
    pub unicode_range: Option<Arc<[(u32, u32)]>>,
}

impl QueryFont {
    /// Attempts to construct a [Charmap] for this font. It maps only the
    /// characters in the font's [`unicode_range`](Self::unicode_range), so
    /// that a face is a candidate for a character only if its range includes
    /// it, as in CSS.
    pub fn charmap(&self) -> Option<Charmap<'_>> {
        self.charmap_index
            .charmap(self.blob.as_ref())
            .map(|charmap| charmap.with_unicode_range(self.unicode_range.as_deref()))
    }

    /// Returns `true` if the font may be used for `codepoint`: it has no
    /// [`unicode_range`](Self::unicode_range), or the range includes it.
    pub fn covers(&self, codepoint: u32) -> bool {
        crate::font::range_covers(self.unicode_range.as_deref(), codepoint)
    }
}

/// Loads every font in `family` matching `attributes` (all coverage variants of the best match).
fn load_bucket<'a>(
    family: &FamilyInfo,
    attributes: Attributes,
    bucket: &'a mut Entry<SmallVec<[QueryFont; 1]>>,
    source_cache: &mut SourceCache,
) -> &'a [QueryFont] {
    match bucket {
        Entry::Error => &[],
        Entry::Ok(fonts) => fonts,
        status @ Entry::Vacant => {
            *status = Entry::Error;
            let mut indices: SmallVec<[usize; 4]> = match_fonts(
                family.fonts(),
                attributes.width,
                attributes.style,
                attributes.weight,
                true,
            )
            .collect();
            // Faces registered with a `unicode-range` come first, the last
            // registered first: CSS Fonts 4 checks the `@font-face` rules of a
            // family in reverse order of definition, and Chromium does
            // (CSSSegmentedFontFace::GetFontData walks its faces in reverse).
            indices.sort_by_key(|&index| match family.fonts()[index].unicode_range() {
                Some(_) => (0, usize::MAX - index),
                None => (1, index),
            });
            let mut fonts: SmallVec<[QueryFont; 1]> = SmallVec::new();
            for index in indices {
                let Some(font_info) = family.fonts().get(index) else {
                    continue;
                };
                let Some(blob) = font_info.load(Some(source_cache)) else {
                    continue;
                };
                fonts.push(QueryFont {
                    family: (family.id(), index),
                    blob: blob.clone(),
                    index: font_info.index(),
                    synthesis: font_info.synthesis(
                        attributes.width,
                        attributes.style,
                        attributes.weight,
                    ),
                    charmap_index: font_info.charmap_index(),
                    unicode_range: font_info.shared_unicode_range(),
                });
            }
            if fonts.is_empty() {
                return &[];
            }
            *status = Entry::Ok(fonts);
            let Entry::Ok(fonts) = status else {
                unreachable!()
            };
            fonts
        }
    }
}

fn load_font<'a>(
    family: &FamilyInfo,
    attributes: Attributes,
    font: &'a mut Entry<QueryFont>,
    source_cache: &mut SourceCache,
) -> Option<&'a QueryFont> {
    match font {
        Entry::Error => None,
        Entry::Ok(font) => Some(font),
        status @ Entry::Vacant => {
            // Set to error in case we fail. This simplifies
            // the following code.
            *status = Entry::Error;
            let family_index = family.default_font_index();
            let font_info = family.fonts().get(family_index)?;
            let blob = font_info.load(Some(source_cache))?;
            let blob_index = font_info.index();
            let synthesis =
                font_info.synthesis(attributes.width, attributes.style, attributes.weight);
            *status = Entry::Ok(QueryFont {
                family: (family.id(), family_index),
                blob,
                index: blob_index,
                synthesis,
                charmap_index: font_info.charmap_index(),
                unicode_range: font_info.shared_unicode_range(),
            });
            if let Entry::Ok(font) = status {
                Some(font)
            } else {
                None
            }
        }
    }
}

#[derive(Clone)]
struct CachedFamily {
    id: FamilyId,
    family: Entry<FamilyInfo>,
    best: Entry<SmallVec<[QueryFont; 1]>>,
    default: Entry<QueryFont>,
}

impl CachedFamily {
    fn new(id: FamilyId) -> Self {
        Self {
            id,
            family: Entry::Vacant,
            best: Entry::Vacant,
            default: Entry::Vacant,
        }
    }

    fn clear_fonts(&mut self) {
        self.best = Entry::Vacant;
        self.default = Entry::Vacant;
    }
}

#[derive(Clone)]
enum Entry<T> {
    Ok(T),
    Vacant,
    Error,
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::{Collection, CollectionOptions, FontInfoOverride};

    const ROBOTO: &[u8] =
        include_bytes!("../../../parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf");

    const CYRILLIC: &[(u32, u32)] = &[(0x0301, 0x0301), (0x0400, 0x045F), (0x2116, 0x2116)];
    const LATIN_EXT: &[(u32, u32)] = &[(0x0100, 0x02BA), (0x2020, 0x2020)];
    const LATIN: &[(u32, u32)] = &[(0x0000, 0x00FF), (0x2000, 0x206F)];

    /// A family of one font registered once per `ranges`, in that order, as
    /// fontsource's `@font-face` rules declare its subsets.
    fn family(ranges: &[Option<&[(u32, u32)]>]) -> Collection {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        for range in ranges {
            collection.register_fonts(
                Blob::new(Arc::new(ROBOTO.to_vec())),
                Some(FontInfoOverride {
                    family_name: Some("Subsets"),
                    unicode_range: *range,
                    ..Default::default()
                }),
            );
        }
        collection
    }

    fn query_ranges(collection: &mut Collection) -> Vec<Option<Vec<(u32, u32)>>> {
        let mut source_cache = SourceCache::default();
        let mut query = collection.query(&mut source_cache);
        query.set_families(["Subsets"]);
        let mut ranges = Vec::new();
        query.matches_with(|font| {
            ranges.push(font.unicode_range.as_deref().map(<[_]>::to_vec));
            QueryStatus::Continue
        });
        ranges
    }

    fn first_available_range(collection: &mut Collection) -> Option<Vec<(u32, u32)>> {
        let mut source_cache = SourceCache::default();
        let mut query = collection.query(&mut source_cache);
        query.set_families(["Subsets"]);
        query
            .first_available_font()
            .and_then(|font| font.unicode_range.as_deref().map(<[_]>::to_vec))
    }

    /// The first font that maps `ch`, as text shaping selects a font.
    fn font_for(collection: &mut Collection, ch: char) -> Option<Vec<(u32, u32)>> {
        let mut source_cache = SourceCache::default();
        let mut query = collection.query(&mut source_cache);
        query.set_families(["Subsets"]);
        let mut found = None;
        query.matches_with(|font| {
            if font.charmap().and_then(|charmap| charmap.map(ch)).is_some() {
                found = Some(font.unicode_range.as_deref().map(<[_]>::to_vec));
                QueryStatus::Stop
            } else {
                QueryStatus::Continue
            }
        });
        found.flatten()
    }

    /// Faces with a `unicode-range` are offered last-registered first, as
    /// CSS Fonts 4 and Chromium order the `@font-face` rules of a family.
    #[test]
    fn ranged_faces_are_offered_in_reverse_registration_order() {
        let mut collection = family(&[Some(CYRILLIC), Some(LATIN_EXT), Some(LATIN)]);
        assert_eq!(
            query_ranges(&mut collection),
            [
                Some(LATIN.to_vec()),
                Some(LATIN_EXT.to_vec()),
                Some(CYRILLIC.to_vec())
            ]
        );
    }

    /// Without ranges, registration order is kept.
    #[test]
    fn faces_without_ranges_keep_registration_order() {
        let mut collection = family(&[None, Some(LATIN), None]);
        assert_eq!(
            query_ranges(&mut collection),
            [Some(LATIN.to_vec()), None, None]
        );
    }

    /// A face maps only the characters in its range, although the font maps
    /// more: Roboto has `0`, `a` and `Ж` in every face here.
    #[test]
    fn a_face_is_a_candidate_only_for_characters_in_its_range() {
        let mut collection = family(&[Some(CYRILLIC), Some(LATIN_EXT), Some(LATIN)]);
        assert_eq!(font_for(&mut collection, '0'), Some(LATIN.to_vec()));
        assert_eq!(font_for(&mut collection, 'a'), Some(LATIN.to_vec()));
        assert_eq!(font_for(&mut collection, 'Ж'), Some(CYRILLIC.to_vec()));
        assert_eq!(font_for(&mut collection, 'Ł'), Some(LATIN_EXT.to_vec()));
        // U+2020 is in the latin and latin-ext ranges: the last defined wins.
        assert_eq!(font_for(&mut collection, '\u{2020}'), Some(LATIN.to_vec()));
        // No range includes U+4E00.
        assert_eq!(font_for(&mut collection, '\u{4E00}'), None);
    }

    /// The first available font is the first face whose range includes
    /// U+0020, whatever the registration order; else the first face.
    #[test]
    fn first_available_font_has_space_in_its_range() {
        for order in [
            [CYRILLIC, LATIN_EXT, LATIN],
            [LATIN, CYRILLIC, LATIN_EXT],
            [LATIN_EXT, LATIN, CYRILLIC],
        ] {
            let mut collection = family(&order.map(Some));
            assert_eq!(first_available_range(&mut collection), Some(LATIN.to_vec()));
        }
        let mut collection = family(&[Some(LATIN_EXT), Some(CYRILLIC)]);
        assert_eq!(
            first_available_range(&mut collection),
            Some(CYRILLIC.to_vec())
        );
    }
}
