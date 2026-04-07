use alloc::rc::Rc;
use alloc::string::String;
use core::ops::{Range, RangeFrom, RangeFull, RangeTo};
use nom::IResult;
use crate::syntax::{Span, ParsedSpan};

/// Nom input wrapper that carries a reference to the full source `Rc<str>`
/// alongside the current `&str` slice, enabling span creation via pointer
/// arithmetic.
#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    /// Current unconsumed slice (always a subslice of `*full`).
    pub(super) fragment: &'a str,
    /// Shared reference to the `Rc<str>` that owns the full source text.
    /// We store a raw pointer to avoid carrying the `Rc` by value (which
    /// isn't `Copy`). Safety: the `Rc` is kept alive by `parse_expr`.
    pub(super) full: *const str,
    /// The `Rc<str>` itself, stored once and cloned only when creating spans.
    /// We keep a reference to it so we can clone cheaply.
    pub(super) source: &'a Rc<str>,
}

impl<'a> Input<'a> {
    pub(super) fn new(input: &'a str, source: &'a Rc<str>) -> Self {
        // IMPORTANT: fragment must point into the Rc's allocation, not the original &str,
        // so that pointer arithmetic in offset() works correctly.
        let rc_str: &'a str = &**source;
        debug_assert_eq!(input.len(), rc_str.len());
        Input { fragment: rc_str, full: rc_str as *const str, source }
    }

    /// Byte offset of the current fragment within the full source.
    pub(super) fn offset(&self) -> usize {
        self.fragment.as_ptr() as usize - self.full as *const u8 as usize
    }

    /// Create a `Span::Parsed` covering bytes `[start_input .. self]`
    /// i.e. from where `start_input` pointed to where `self` currently points.
    pub(super) fn span_since(&self, start: Input<'a>) -> Span {
        Span::Parsed(ParsedSpan::new(
            self.source.clone(),
            start.offset(),
            self.offset(),
        ))
    }

    pub(super) fn starts_with_char(&self, c: char) -> bool { self.fragment.starts_with(c) }
    pub(super) fn is_empty(&self) -> bool { self.fragment.is_empty() }
    pub(super) fn len(&self) -> usize { self.fragment.len() }
}

impl<'a> core::fmt::Debug for Input<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.fragment.fmt(f)
    }
}

impl<'a> PartialEq for Input<'a> {
    fn eq(&self, other: &Self) -> bool { self.fragment == other.fragment }
}
impl<'a> Eq for Input<'a> {}

// ── nom trait implementations ────────────────────────────────────────
// All delegate to the inner `&str`, reconstructing `Input` for returned slices.

impl<'a> nom::InputLength for Input<'a> {
    fn input_len(&self) -> usize { self.fragment.len() }
}

impl<'a> nom::Offset for Input<'a> {
    fn offset(&self, second: &Self) -> usize {
        second.fragment.as_ptr() as usize - self.fragment.as_ptr() as usize
    }
}

impl<'a> nom::AsBytes for Input<'a> {
    fn as_bytes(&self) -> &[u8] { self.fragment.as_bytes() }
}

impl<'a> nom::InputIter for Input<'a> {
    type Item = char;
    type Iter = core::str::CharIndices<'a>;
    type IterElem = core::str::Chars<'a>;

    fn iter_indices(&self) -> Self::Iter { self.fragment.char_indices() }
    fn iter_elements(&self) -> Self::IterElem { self.fragment.chars() }
    fn position<P>(&self, predicate: P) -> Option<usize>
    where P: Fn(Self::Item) -> bool {
        self.fragment.char_indices()
            .find(|&(_, c)| predicate(c))
            .map(|(i, _)| i)
    }
    fn slice_index(&self, count: usize) -> Result<usize, nom::Needed> {
        let mut cnt = 0;
        for (index, _) in self.fragment.char_indices() {
            if cnt == count { return Ok(index); }
            cnt += 1;
        }
        if cnt == count { return Ok(self.fragment.len()); }
        Err(nom::Needed::Unknown)
    }
}

impl<'a> nom::InputTake for Input<'a> {
    fn take(&self, count: usize) -> Self {
        Input { fragment: &self.fragment[..count], ..*self }
    }
    fn take_split(&self, count: usize) -> (Self, Self) {
        let (prefix, suffix) = self.fragment.split_at(count);
        (
            Input { fragment: suffix, ..*self },
            Input { fragment: prefix, ..*self },
        )
    }
}

impl<'a> nom::InputTakeAtPosition for Input<'a> {
    type Item = char;

    fn split_at_position<P, E: nom::error::ParseError<Self>>(
        &self, predicate: P,
    ) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
        match self.fragment.find(predicate) {
            Some(i) => Ok((
                Input { fragment: &self.fragment[i..], ..*self },
                Input { fragment: &self.fragment[..i], ..*self },
            )),
            None => Err(nom::Err::Incomplete(nom::Needed::new(1))),
        }
    }

    fn split_at_position1<P, E: nom::error::ParseError<Self>>(
        &self, predicate: P, e: nom::error::ErrorKind,
    ) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
        match self.fragment.find(predicate) {
            Some(0) => Err(nom::Err::Error(E::from_error_kind(*self, e))),
            Some(i) => Ok((
                Input { fragment: &self.fragment[i..], ..*self },
                Input { fragment: &self.fragment[..i], ..*self },
            )),
            None => Err(nom::Err::Incomplete(nom::Needed::new(1))),
        }
    }

    fn split_at_position_complete<P, E: nom::error::ParseError<Self>>(
        &self, predicate: P,
    ) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
        match self.fragment.find(predicate) {
            Some(i) => Ok((
                Input { fragment: &self.fragment[i..], ..*self },
                Input { fragment: &self.fragment[..i], ..*self },
            )),
            None => Ok((
                Input { fragment: &self.fragment[self.fragment.len()..], ..*self },
                Input { fragment: self.fragment, ..*self },
            )),
        }
    }

    fn split_at_position1_complete<P, E: nom::error::ParseError<Self>>(
        &self, predicate: P, e: nom::error::ErrorKind,
    ) -> IResult<Self, Self, E>
    where P: Fn(Self::Item) -> bool {
        match self.fragment.find(predicate) {
            Some(0) => Err(nom::Err::Error(E::from_error_kind(*self, e))),
            Some(i) => Ok((
                Input { fragment: &self.fragment[i..], ..*self },
                Input { fragment: &self.fragment[..i], ..*self },
            )),
            None if self.fragment.is_empty() =>
                Err(nom::Err::Error(E::from_error_kind(*self, e))),
            None => Ok((
                Input { fragment: &self.fragment[self.fragment.len()..], ..*self },
                Input { fragment: self.fragment, ..*self },
            )),
        }
    }
}

impl<'a, 'b> nom::Compare<&'b str> for Input<'a> {
    fn compare(&self, t: &'b str) -> nom::CompareResult {
        nom::Compare::compare(&self.fragment, t)
    }
    fn compare_no_case(&self, t: &'b str) -> nom::CompareResult {
        nom::Compare::compare_no_case(&self.fragment, t)
    }
}

impl<'a> nom::FindToken<char> for Input<'a> {
    fn find_token(&self, token: char) -> bool {
        self.fragment.chars().any(|c| c == token)
    }
}

impl<'a, 'b> nom::FindSubstring<&'b str> for Input<'a> {
    fn find_substring(&self, substr: &'b str) -> Option<usize> {
        self.fragment.find(substr)
    }
}

impl<'a, R: core::str::FromStr> nom::ParseTo<R> for Input<'a> {
    fn parse_to(&self) -> Option<R> { self.fragment.parse().ok() }
}

macro_rules! impl_slice {
    ($range:ty, $index:expr) => {
        impl<'a> nom::Slice<$range> for Input<'a> {
            fn slice(&self, range: $range) -> Self {
                Input { fragment: &self.fragment[$index(range)], ..*self }
            }
        }
    };
}
impl_slice!(Range<usize>,    |r: Range<usize>| r);
impl_slice!(RangeTo<usize>,  |r: RangeTo<usize>| r);
impl_slice!(RangeFrom<usize>,|r: RangeFrom<usize>| r);
impl_slice!(RangeFull,       |r: RangeFull| r);

impl<'a> nom::ExtendInto for Input<'a> {
    type Item = char;
    type Extender = String;
    fn new_builder(&self) -> String { String::new() }
    fn extend_into(&self, acc: &mut String) { acc.push_str(self.fragment); }
}
