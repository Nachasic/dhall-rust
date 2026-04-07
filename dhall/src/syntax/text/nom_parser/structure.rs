use alloc::vec;
use nom::{branch::alt,
    character::complete::char,
    combinator::{cut, opt},
    error::context,
    multi::{many0, separated_list0},
    sequence::{delimited, preceded, terminated}};
use super::input::Input;
use super::helpers::*;
use super::literals::*;
use super::labels::*;
use super::imports::import_expr;
use super::application::application;
use super::expression::expression;
use crate::syntax::{Expr, ExprKind, Label, NumKind, Span, V};

pub(super) fn atom(input: Input<'_>) -> ParseResult<'_, Expr> {
    context("expression", alt((
        // Parenthesized expression
        delimited(
            terminated(char('('), ws),
            expression,
            preceded(ws, char(')')),
        ),
        // Numeric literals (order matters: double before natural)
        map_spanned(double_literal, |n| ExprKind::Num(NumKind::Double(n))),
        map_spanned(integer_literal, |n| ExprKind::Num(NumKind::Integer(n))),
        map_spanned(natural_literal, |n| ExprKind::Num(NumKind::Natural(n))),
        // Text literal
        map_spanned(double_quote_literal, |t| ExprKind::TextLit(t)),
        map_spanned(single_quote_literal, |t| ExprKind::TextLit(t)),
        // Record literal/type: { ... }
        record_literal_or_type,
        // Union type: < ... >
        union_type,
        // List literal: [ ... ] (non-empty only; empty list is at expression level)
        list_literal,
        // Imports (before builtins/variables — `missing`, `env:` look like identifiers)
        import_expr,
        // Builtins and constants (reject if followed by @ — that's invalid)
        builtin_no_index,
        // Variable
        map_spanned(variable, |v| ExprKind::Var(v)),
    )))(input)
}

// ── 7. Records ───────────────────────────────────────────────────────

pub(super) fn record_literal_or_type(input: Input<'_>) -> ParseResult<'_, Expr> {
    use alloc::collections::BTreeMap;
    let (rest, expr) = delimited(
        terminated(char('{'), ws),
        |input| {
            let (rest, _) = opt(terminated(char(','), ws))(input)?;
            // Try empty record literal: = [,]
            if rest.starts_with_char('=') {
                let rest2 = Input { fragment: &rest.fragment[1..], ..rest };
                let (rest2, _) = opt(preceded(ws, char(',')))(rest2)?;
                return Ok((rest2, ExprKind::RecordLit(Default::default())));
            }
            // Try non-empty record
            if let Ok((rest2, first)) = record_entry(rest) {
                let (rest2, _) = ws(rest2)?;
                let (rest2, mut more) = many0(|input| {
                    let (r, _) = char(',')(input)?;
                    let (r, _) = ws(r)?;
                    let (r, e) = record_entry(r)?;
                    let (r, _) = ws(r)?;
                    Ok((r, e))
                })(rest2)?;
                let (rest2, _) = opt(terminated(char(','), ws))(rest2)?;
                let mut entries = vec![first];
                entries.append(&mut more);
                let is_type = entries.iter().all(|(_, sep, _)| *sep == ':');
                if is_type {
                    let mut map = BTreeMap::new();
                    for (l, _, e) in entries {
                        if map.contains_key(&l) {
                            return Err(nom::Err::Failure(nom::error::VerboseError {
                                errors: alloc::vec![(input, nom::error::VerboseErrorKind::Context("Duplicate field in record type"))],
                            }));
                        }
                        map.insert(l, e);
                    }
                    return Ok((rest2, ExprKind::RecordType(map)));
                } else {
                    let mut map = BTreeMap::new();
                    for (l, _, e) in entries {
                        insert_recordlit_entry(&mut map, l, e);
                    }
                    return Ok((rest2, ExprKind::RecordLit(map)));
                }
            }
            // Empty record type {} or { , }
            Ok((rest, ExprKind::RecordType(Default::default())))
        },
        preceded(ws, char('}')),
    )(input)?;
    Ok((rest, spanned(input, rest, expr)))
}

/// Record entry: `name = expr`, `name : type`, `name` (pun), or `name.a.b = expr` (dotted).
pub(super) fn record_entry(input: Input<'_>) -> ParseResult<'_, (Label, char, Expr)> {
    let (rest, first_label) = terminated(any_label_or_some, ws)(input)?;

    // Try dotted field syntax: name.a.b = expr
    if rest.starts_with_char('.') {
        let rest2 = Input { fragment: &rest.fragment[1..], ..rest };
        let (rest2, _) = ws(rest2)?;
        // Collect remaining dot-separated labels
        let (rest2, more_labels) = separated_list0(
            delimited(ws, char('.'), ws),
            any_label_or_some,
        )(rest2)?;
        let (rest2, _) = ws(rest2)?;
        let (rest2, _) = char('=')(rest2)?;
        let (rest2, _) = ws(rest2)?;
        let (rest2, val) = expression(rest2)?;
        // Desugar: { a.b.c = v } → { a = { b = { c = v } } }
        let nested = more_labels.into_iter().rev().fold(val, |inner, l| {
            let map = core::iter::once((l, inner)).collect();
            Expr::new(ExprKind::RecordLit(map), Span::Artificial)
        });
        return Ok((rest2, (first_label, '=', nested)));
    }

    // Try `name = expr` or `name : type`
    if rest.starts_with_char('=') {
        let rest2 = Input { fragment: &rest.fragment[1..], ..rest };
        let (rest2, _) = ws(rest2)?;
        let (rest2, val) = expression(rest2)?;
        return Ok((rest2, (first_label, '=', val)));
    }
    if rest.starts_with_char(':') {
        let rest2 = Input { fragment: &rest.fragment[1..], ..rest };
        let (rest2, _) = ws1(rest2)?;
        let (rest2, val) = expression(rest2)?;
        return Ok((rest2, (first_label, ':', val)));
    }

    // Pun: `{ name }` desugars to `{ name = name }`
    let pun_expr = Expr::new(ExprKind::Var(V(first_label.clone(), 0)), Span::Artificial);
    Ok((rest, (first_label, '=', pun_expr)))
}

// ── 8. Lists ─────────────────────────────────────────────────────────

pub(super) fn list_literal(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, items) = delimited(
        terminated(char('['), ws),
        |input| {
            let (rest, _) = opt(terminated(char(','), ws))(input)?;
            let (rest, first) = expression(rest)?;
            let (rest, _) = ws(rest)?;
            let (rest, mut more) = many0(|input| {
                let (r, _) = char(',')(input)?;
                let (r, _) = ws(r)?;
                let (r, e) = expression(r)?;
                let (r, _) = ws(r)?;
                Ok((r, e))
            })(rest)?;
            let (rest, _) = opt(terminated(char(','), ws))(rest)?;
            let mut items = vec![first];
            items.append(&mut more);
            Ok((rest, items))
        },
        preceded(ws, char(']')),
    )(input)?;
    Ok((rest, spanned(input, rest, ExprKind::NEListLit(items))))
}

// ── 8b. Union types ──────────────────────────────────────────────────

/// Parse a single union type entry: `label` or `label : type`.
pub(super) fn union_type_entry(input: Input<'_>) -> ParseResult<'_, (Label, Option<Expr>)> {
    let (rest, l) = terminated(any_label_or_some, ws)(input)?;
    let (rest, ty) = opt(|input| {
        let (r, _) = char(':')(input)?;
        let (r, _) = ws1(r)?;
        let (r, e) = expression(r)?;
        Ok((r, e))
    })(rest)?;
    Ok((rest, (l, ty)))
}

pub(super) fn union_type(input: Input<'_>) -> ParseResult<'_, Expr> {
    use alloc::collections::BTreeMap;
    let (rest, _) = terminated(char('<'), ws)(input)?;
    let (rest, _) = opt(terminated(char('|'), ws))(rest)?;
    let (rest, entries) = if let Ok((r, first)) = union_type_entry(rest) {
        let (r, _) = ws(r)?;
        let (r, mut more) = many0(preceded(
            terminated(char('|'), ws),
            terminated(union_type_entry, ws),
        ))(r)?;
        let (r, _) = opt(preceded(char('|'), ws))(r)?;
        let mut entries = vec![first];
        entries.append(&mut more);
        (r, entries)
    } else {
        (rest, vec![])
    };
    let mut map = BTreeMap::new();
    for (l, ty) in entries {
        if map.contains_key(&l) {
            return Err(nom::Err::Failure(nom::error::VerboseError {
                errors: alloc::vec![(input, nom::error::VerboseErrorKind::Context("Duplicate variant in union type"))],
            }));
        }
        map.insert(l, ty);
    }
    let (rest, _) = preceded(ws, char('>'))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::UnionType(map))))
}

// ── 8c. Empty list with type ─────────────────────────────────────────

pub(super) fn empty_list_literal(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = terminated(char('['), ws)(input)?;
    let (rest, _) = opt(terminated(char(','), ws))(rest)?;
    let (rest, _) = terminated(char(']'), ws)(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in empty list type", ws1))(rest)?;
    let (rest, ty) = cut(context("type annotation for empty list (e.g. `[] : List T`)", application))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::EmptyListLit(ty))))
}

