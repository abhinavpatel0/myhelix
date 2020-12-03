//! Selections are the primary editing construct. Even a single cursor is defined as an empty
//! single selection range.
//!
//! All positioning is done via `char` offsets into the buffer.
use crate::{Assoc, ChangeSet, Rope, RopeSlice};
use smallvec::{smallvec, SmallVec};
use std::borrow::Cow;

#[inline]
fn abs_difference(x: usize, y: usize) -> usize {
    if x < y {
        y - x
    } else {
        x - y
    }
}

/// A single selection range. Anchor-inclusive, head-exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    // TODO: optimize into u32
    /// The anchor of the range: the side that doesn't move when extending.
    pub anchor: usize,
    /// The head of the range, moved when extending.
    pub head: usize,
} // TODO: might be cheaper to store normalized as from/to and an inverted flag

impl Range {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// Start of the range.
    #[inline]
    #[must_use]
    pub fn from(&self) -> usize {
        std::cmp::min(self.anchor, self.head)
    }

    /// End of the range.
    #[inline]
    #[must_use]
    pub fn to(&self) -> usize {
        std::cmp::max(self.anchor, self.head)
    }

    /// `true` when head and anchor are at the same position.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Check two ranges for overlap.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        // cursor overlap is checked differently
        if self.is_empty() {
            self.from() <= other.to()
        } else {
            self.from() < other.to()
        }
    }

    pub fn contains(&self, pos: usize) -> bool {
        if self.is_empty() {
            return false;
        }

        if self.anchor < self.head {
            self.anchor <= pos && pos < self.head
        } else {
            self.head < pos && pos <= self.anchor
        }
    }

    /// Map a range through a set of changes. Returns a new range representing the same position
    /// after the changes are applied.
    pub fn map(self, changes: &ChangeSet) -> Self {
        let anchor = changes.map_pos(self.anchor, Assoc::After);
        let head = changes.map_pos(self.head, Assoc::After);

        // TODO: possibly unnecessary
        if self.anchor == anchor && self.head == head {
            return self;
        }
        Self { anchor, head }
    }

    /// Extend the range to cover at least `from` `to`.
    #[must_use]
    pub fn extend(&self, from: usize, to: usize) -> Self {
        if from <= self.anchor && to >= self.anchor {
            return Range {
                anchor: from,
                head: to,
            };
        }

        Range {
            anchor: self.anchor,
            head: if abs_difference(from, self.anchor) > abs_difference(to, self.anchor) {
                from
            } else {
                to
            },
        }
    }

    // groupAt

    #[inline]
    pub fn fragment<'a>(&'a self, text: &'a RopeSlice) -> Cow<'a, str> {
        Cow::from(text.slice(self.from()..self.to() + 1))
    }
}

/// A selection consists of one or more selection ranges.
#[derive(Debug, Clone)]
pub struct Selection {
    // TODO: decide how many ranges to inline SmallVec<[Range; 1]>
    ranges: SmallVec<[Range; 1]>,
    primary_index: usize,
}

impl Selection {
    // eq

    #[must_use]
    pub fn primary(&self) -> Range {
        self.ranges[self.primary_index]
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.primary().head
    }

    /// Ensure selection containing only the primary selection.
    pub fn into_single(self) -> Self {
        if self.ranges.len() == 1 {
            self
        } else {
            Self {
                ranges: smallvec![self.ranges[self.primary_index]],
                primary_index: 0,
            }
        }
    }

    // add_range // push
    // replace_range

    /// Map selections over a set of changes. Useful for adjusting the selection position after
    /// applying changes to a document.
    pub fn map(self, changes: &ChangeSet) -> Self {
        if changes.is_empty() {
            return self;
        }

        Self::new(
            self.ranges
                .into_iter()
                .map(|range| range.map(changes))
                .collect(),
            self.primary_index,
        )
    }

    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    #[must_use]
    /// Constructs a selection holding a single range.
    pub fn single(anchor: usize, head: usize) -> Self {
        Self {
            ranges: smallvec![Range { anchor, head }],
            primary_index: 0,
        }
    }

    /// Constructs a selection holding a single cursor.
    pub fn point(pos: usize) -> Self {
        Self::single(pos, pos)
    }

    #[must_use]
    pub fn new(ranges: SmallVec<[Range; 1]>, primary_index: usize) -> Self {
        fn normalize(mut ranges: SmallVec<[Range; 1]>, mut primary_index: usize) -> Selection {
            let primary = ranges[primary_index];
            ranges.sort_unstable_by_key(Range::from);
            primary_index = ranges.iter().position(|&range| range == primary).unwrap();

            let mut result: SmallVec<[Range; 1]> = SmallVec::new();

            // TODO: we could do with one vec by removing elements as we mutate

            for (i, range) in ranges.into_iter().enumerate() {
                // if previous value exists
                if let Some(prev) = result.last_mut() {
                    // and we overlap it
                    if range.overlaps(prev) {
                        let from = prev.from();
                        let to = std::cmp::max(range.to(), prev.to());

                        if i <= primary_index {
                            primary_index -= 1
                        }

                        // merge into previous
                        if range.anchor > range.head {
                            prev.anchor = to;
                            prev.head = from;
                        } else {
                            prev.anchor = from;
                            prev.head = to;
                        }
                        continue;
                    }
                }

                result.push(range)
            }

            Selection {
                ranges: result,
                primary_index,
            }
        }

        // fast path for a single selection (cursor)
        if ranges.len() == 1 {
            return Selection {
                ranges,
                primary_index: 0,
            };
        }

        // TODO: only normalize if needed (any ranges out of order)
        normalize(ranges, primary_index)
    }

    /// Takes a closure and maps each selection over the closure.
    pub fn transform<F>(&self, f: F) -> Self
    where
        F: Fn(Range) -> Range,
    {
        Self::new(
            self.ranges.iter().copied().map(f).collect(),
            self.primary_index,
        )
    }

    pub fn fragments<'a>(&'a self, text: &'a RopeSlice) -> impl Iterator<Item = Cow<str>> + 'a {
        self.ranges.iter().map(move |range| range.fragment(text))
    }
}

// TODO: checkSelection -> check if valid for doc length

// TODO: support to split on capture #N instead of whole match
pub fn split_on_matches(
    text: &RopeSlice,
    selections: &Selection,
    regex: &crate::regex::Regex,
) -> Selection {
    let mut result = SmallVec::with_capacity(selections.ranges().len());

    for sel in selections.ranges() {
        // TODO: can't avoid occasional allocations since Regex can't operate on chunks yet
        let fragment = sel.fragment(&text);

        let mut sel_start = sel.from();
        let sel_end = sel.to();

        let mut start_byte = text.char_to_byte(sel_start);

        let mut start = sel_start;

        for mat in regex.find_iter(&fragment) {
            // TODO: retain range direction

            let end = text.byte_to_char(start_byte + mat.start());
            result.push(Range::new(start, end - 1));
            start = text.byte_to_char(start_byte + mat.end());
        }

        if start <= sel_end {
            result.push(Range::new(start, sel_end));
        }
    }

    // TODO: figure out a new primary index
    Selection::new(result, 0)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_create_normalizes_and_merges() {
        let sel = Selection::new(
            smallvec![
                Range::new(10, 12),
                Range::new(6, 7),
                Range::new(4, 5),
                Range::new(3, 4),
                Range::new(0, 6),
                Range::new(7, 8),
                Range::new(9, 13),
                Range::new(13, 14),
            ],
            0,
        );

        let res = sel
            .ranges
            .into_iter()
            .map(|range| format!("{}/{}", range.anchor, range.head))
            .collect::<Vec<String>>()
            .join(",");

        assert_eq!(res, "0/6,6/7,7/8,9/13,13/14");
    }

    #[test]
    fn test_create_merges_adjacent_points() {
        let sel = Selection::new(
            smallvec![
                Range::new(10, 12),
                Range::new(12, 12),
                Range::new(12, 12),
                Range::new(10, 10),
                Range::new(8, 10),
            ],
            0,
        );

        let res = sel
            .ranges
            .into_iter()
            .map(|range| format!("{}/{}", range.anchor, range.head))
            .collect::<Vec<String>>()
            .join(",");

        assert_eq!(res, "8/10,10/12");
    }

    #[test]
    fn test_contains() {
        let range = Range::new(10, 12);

        assert_eq!(range.contains(9), false);
        assert_eq!(range.contains(10), true);
        assert_eq!(range.contains(11), true);
        assert_eq!(range.contains(12), false);
        assert_eq!(range.contains(13), false);

        let range = Range::new(9, 6);
        assert_eq!(range.contains(9), true);
        assert_eq!(range.contains(7), true);
        assert_eq!(range.contains(6), false);
    }

    #[test]
    fn test_split_on_matches() {
        use crate::regex::Regex;

        let text = Rope::from("abcd efg wrs   xyz 123 456");

        let selections = Selection::new(smallvec![Range::new(0, 8), Range::new(10, 19),], 0);

        let result = split_on_matches(&text.slice(..), &selections, &Regex::new(r"\s+").unwrap());

        assert_eq!(
            result.ranges(),
            &[
                Range::new(0, 3),
                Range::new(5, 7),
                Range::new(10, 11),
                Range::new(15, 17),
                Range::new(19, 19),
            ]
        );

        assert_eq!(
            result.fragments(&text.slice(..)).collect::<Vec<_>>(),
            &["abcd", "efg", "rs", "xyz", "1"]
        );
    }
}
