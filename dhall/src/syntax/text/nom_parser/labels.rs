use nom::{branch::alt, bytes::complete::{take_while, take_while1},
    character::complete::char, combinator::{map, opt, recognize},
    sequence::{delimited, pair, preceded}};
use super::input::Input;
use super::helpers::*;
use super::literals::natural_literal;
use crate::syntax::{Expr, ExprKind, Label, NumKind, UnspannedExpr, V, Const};

/// Reserved words that cannot be used as labels.
pub(super) const RESERVED: &[&str] = &[
    "if", "then", "else", "let", "in", "using", "missing", "as",
    "Infinity", "NaN", "merge", "Some", "toMap", "assert", "forall",
    "with",
];

/// Check if a name is a builtin or constant (True, False, Type, Kind, Sort, or Builtin::parse).
pub(super) fn is_builtin_name(name: &str) -> bool {
    matches!(name, "True" | "False" | "Type" | "Kind" | "Sort")
        || crate::builtins::Builtin::parse(name).is_some()
}

pub(super) fn is_label_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

pub(super) fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/'
}

pub(super) fn simple_label(input: Input<'_>) -> ParseResult<'_, Label> {
    let (rest, name) = recognize(pair(
        take_while1(is_label_start),
        take_while(is_label_char),
    ))(input)?;

    if RESERVED.contains(&name.fragment) {
        return Err(tag_err(input));
    }

    Ok((rest, Label::from(name.fragment)))
}

/// A nonreserved-label: rejects both keywords AND builtins (unless backtick-quoted).
pub(super) fn nonreserved_label(input: Input<'_>) -> ParseResult<'_, Label> {
    if let Ok(r) = backtick_label(input) {
        return Ok(r);
    }
    let (rest, l) = simple_label(input)?;
    if is_builtin_name(l.as_ref()) {
        return Err(tag_err(input));
    }
    Ok((rest, l))
}

pub(super) fn backtick_label(input: Input<'_>) -> ParseResult<'_, Label> {
    delimited(
        char('`'),
        map(take_while(|c: char| c != '`'), |s: Input<'_>| Label::from(s.fragment)),
        char('`'),
    )(input)
}

pub(super) fn label(input: Input<'_>) -> ParseResult<'_, Label> {
    alt((backtick_label, simple_label))(input)
}

/// any-label-or-some: allows all labels plus the keyword `Some`.
pub(super) fn any_label_or_some(input: Input<'_>) -> ParseResult<'_, Label> {
    alt((
        label,
        map(keyword("Some"), |_| Label::from("Some")),
    ))(input)
}

pub(super) fn variable(input: Input<'_>) -> ParseResult<'_, V> {
    let (rest, l) = nonreserved_label(input)?;
    let (rest, idx) = opt(preceded(
        delimited(ws, char('@'), ws),
        natural_literal,
    ))(rest)?;
    Ok((rest, V(l, idx.unwrap_or(0) as usize)))
}

// ── 4. Builtins ──────────────────────────────────────────────────────

pub(super) fn builtin(input: Input<'_>) -> ParseResult<'_, UnspannedExpr> {
    let (rest, name) = recognize(pair(
        take_while1(is_label_start),
        take_while(is_label_char),
    ))(input)?;

    let expr = match name.fragment {
        "True" => ExprKind::Num(NumKind::Bool(true)),
        "False" => ExprKind::Num(NumKind::Bool(false)),
        "Type" => ExprKind::Const(Const::Type),
        "Kind" => ExprKind::Const(Const::Kind),
        "Sort" => ExprKind::Const(Const::Sort),
        _ => match crate::builtins::Builtin::parse(name.fragment) {
            Some(b) => ExprKind::Builtin(b),
            None => return Err(tag_err(input)),
        },
    };
    Ok((rest, expr))
}

pub(super) fn builtin_no_index(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, b) = builtin(input)?;
    if rest.starts_with_char('@') {
        Err(tag_err(input))
    } else {
        Ok((rest, spanned(input, rest, b)))
    }
}

