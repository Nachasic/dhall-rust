use nom::IResult;
use nom::bytes::complete::{tag, take_while};
use nom::character::complete::multispace0;
use super::input::Input;
use crate::operations::{BinOp, OpKind};
use crate::syntax::{Expr, ExprKind, Label, NumKind, Span, UnspannedExpr};

// ── Helpers ──────────────────────────────────────────────────────────

pub(super) type InputVerboseError<'a> = nom::error::VerboseError<Input<'a>>;
pub(super) type ParseResult<'a, T> = IResult<Input<'a>, T, InputVerboseError<'a>>;

/// Create an error at the given input position.
pub(super) fn make_err(input: Input<'_>, kind: nom::error::ErrorKind) -> nom::Err<InputVerboseError<'_>> {
    nom::Err::Error(nom::error::VerboseError {
        errors: alloc::vec![(input, nom::error::VerboseErrorKind::Nom(kind))],
    })
}

pub(super) fn tag_err(input: Input<'_>) -> nom::Err<InputVerboseError<'_>> {
    make_err(input, nom::error::ErrorKind::Tag)
}

/// Error type for the public API.
pub type ParseError = String;

/// Create a spanned expression. `before` is the input at the start of the
/// production, `after` is the input after it was consumed.
pub(super) fn spanned(before: Input<'_>, after: Input<'_>, kind: UnspannedExpr) -> Expr {
    Expr::new(kind, after.span_since(before))
}

/// Like `map`, but wraps the result in a spanned `Expr`.
pub(super) fn map_spanned<'a, O, F, G>(
    mut parser: F,
    f: G,
) -> impl FnMut(Input<'a>) -> ParseResult<'a, Expr>
where
    F: FnMut(Input<'a>) -> ParseResult<'a, O>,
    G: Fn(O) -> UnspannedExpr,
{
    move |input: Input<'a>| {
        let (rest, val) = parser(input)?;
        Ok((rest, spanned(input, rest, f(val))))
    }
}

/// Parse a keyword, ensuring it's not a prefix of a longer identifier.
pub(super) fn keyword<'a>(kw: &'static str) -> impl FnMut(Input<'a>) -> ParseResult<'a, Input<'a>> {
    move |input: Input<'a>| {
        let (rest, matched) = tag(kw)(input)?;
        if rest.fragment.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
            Err(tag_err(input))
        } else {
            Ok((rest, matched))
        }
    }
}

/// Insert a record literal entry, merging duplicates with `∧`.
pub(super) fn insert_recordlit_entry(map: &mut alloc::collections::BTreeMap<Label, Expr>, l: Label, e: Expr) {
    use alloc::collections::btree_map::Entry;
    match map.entry(l) {
        Entry::Vacant(entry) => { entry.insert(e); }
        Entry::Occupied(mut entry) => {
            let other = entry.insert(Expr::new(ExprKind::Num(NumKind::Bool(false)), Span::Artificial));
            let span = Span::DuplicateRecordFieldsSugar(
                Box::new(other.span()),
                Box::new(e.span()),
            );
            entry.insert(Expr::new(
                ExprKind::Op(OpKind::BinOp(
                    BinOp::RecursiveRecordMerge, other, e,
                )),
                span,
            ));
        }
    }
}

// ── 1. Whitespace and comments ───────────────────────────────────────

/// Skip whitespace and line comments (-- to end of line)
/// and block comments ({- ... -}, which can nest).
pub(super) fn ws(input: Input<'_>) -> ParseResult<'_, ()> {
    let mut rest = input;
    loop {
        let (r, _) = multispace0(rest)?;
        rest = r;
        if rest.fragment.starts_with("--") {
            rest = Input { fragment: &rest.fragment[2..], ..rest };
            let (r, _) = take_while(|c: char| c != '\n')(rest)?;
            rest = r;
        } else if rest.fragment.starts_with("{-") {
            rest = block_comment(Input { fragment: &rest.fragment[2..], ..rest })?;
        } else {
            break;
        }
    }
    Ok((rest, ()))
}

/// Consume the body of a block comment (after the opening `{-`).
/// Handles nesting: each `{-` inside must be matched by a `-}`.
pub(super) fn block_comment<'a>(input: Input<'a>) -> Result<Input<'a>, nom::Err<InputVerboseError<'a>>> {
    let mut rest = input;
    loop {
        match rest.fragment.find("{-").map(|i| (i, true)).into_iter()
            .chain(rest.fragment.find("-}").map(|i| (i, false)))
            .min_by_key(|(i, _)| *i)
        {
            Some((i, true)) => {
                rest = block_comment(Input { fragment: &rest.fragment[i + 2..], ..rest })?;
            }
            Some((i, false)) => {
                return Ok(Input { fragment: &rest.fragment[i + 2..], ..rest });
            }
            None => {
                return Err(tag_err(input));
            }
        }
    }
}

/// Mandatory whitespace (at least one space/tab/newline/comment).
pub(super) fn ws1(input: Input<'_>) -> ParseResult<'_, ()> {
    let start = input;
    let (rest, _) = ws(input)?;
    if rest.len() == start.len() {
        Err(make_err(input, nom::error::ErrorKind::Space))
    } else {
        Ok((rest, ()))
    }
}

