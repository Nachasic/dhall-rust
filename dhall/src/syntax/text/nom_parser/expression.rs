use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use nom::{branch::alt, bytes::complete::tag,
    character::complete::char,
    combinator::{cut, opt},
    error::context,
    multi::many0,
    sequence::{delimited, preceded, terminated}};
use super::input::Input;
use super::helpers::*;
use super::labels::*;
use super::application::*;
use super::operators::operator_expression;
use super::structure::empty_list_literal;
use super::errors::*;
use crate::syntax::{Expr, ExprKind};
use crate::operations::OpKind;

pub(super) fn let_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("let")(input)?;
    let (mut rest, _) = cut(context("whitespace after `let`", ws1))(rest)?;
    let mut bindings = Vec::new();
    loop {
        let (r, name) = terminated(cut(context("variable name in `let` binding", nonreserved_label)), ws)(rest)?;
        let (r, annot) = opt(|input| {
            let (r, _) = char(':')(input)?;
            let (r, _) = cut(context("whitespace after `:` in `let` type annotation", ws1))(r)?;
            let (r, e) = cut(context("type expression in `let` annotation", expression))(r)?;
            let (r, _) = ws(r)?;
            Ok((r, e))
        })(r)?;
        let (r, _) = cut(context("`=` in `let` binding", char('=')))(r)?;
        let (r, _) = ws(r)?;
        let (r, val) = cut(context("expression after `=` in `let` binding", expression))(r)?;
        let (r, _) = ws(r)?;
        bindings.push((name, annot, val));
        rest = r;
        if let Ok((r, _)) = keyword::<'_>("let")(rest) {
            let (r, _) = cut(context("whitespace after `let`", ws1))(r)?;
            rest = r;
        } else {
            break;
        }
    }
    let (rest, _) = cut(context("`in` keyword after `let` binding", keyword("in")))(rest)?;
    let (rest, _) = cut(context("whitespace after `in`", ws1))(rest)?;
    let (rest, body) = cut(context("body expression after `in`", expression))(rest)?;
    let expr = bindings.into_iter().rev().fold(body, |acc, (name, annot, val)| {
        spanned(input, rest, ExprKind::Let(name, annot, val, acc))
    });
    Ok((rest, expr))
}

pub(super) fn lambda_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = alt((tag("\\"), tag("λ")))(input)?;
    let (rest, _) = cut(context("whitespace before `(` in lambda", ws))(rest)?;
    let (rest, _) = cut(context("`(` after `\\` or `λ`", char('(')))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = cut(context("parameter name in lambda", terminated(nonreserved_label, ws)))(rest)?;
    let (rest, _) = cut(context("`:` after parameter name", char(':')))(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in lambda", ws1))(rest)?;
    let (rest, ty) = cut(context("type annotation in lambda", expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`)` in lambda", char(')')))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`->` or `→` after lambda parameters", alt((tag("->"), tag("→")))))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, body) = cut(context("body expression in lambda", expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Lam(name, ty, body))))
}

pub(super) fn if_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("if")(input)?;
    let (rest, _) = cut(context("whitespace after `if`", ws1))(rest)?;
    let (rest, cond) = cut(context("condition after `if`", expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`then` keyword", keyword("then")))(rest)?;
    let (rest, _) = cut(context("whitespace after `then`", ws1))(rest)?;
    let (rest, t) = cut(context("expression after `then`", expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`else` keyword", keyword("else")))(rest)?;
    let (rest, _) = cut(context("whitespace after `else`", ws1))(rest)?;
    let (rest, f) = cut(context("expression after `else`", expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::BoolIf(cond, t, f)))))
}

pub(super) fn forall_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = alt((tag("forall"), tag("∀")))(input)?;
    let (rest, _) = cut(context("whitespace before `(` in `forall`", ws))(rest)?;
    let (rest, _) = cut(context("`(` after `forall`", char('(')))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = cut(context("variable name in `forall`", terminated(nonreserved_label, ws)))(rest)?;
    let (rest, _) = cut(context("`:` in `forall`", char(':')))(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in `forall`", ws1))(rest)?;
    let (rest, ty) = cut(context("type in `forall`", expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`)` in `forall`", char(')')))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`->` or `→` after `forall`", alt((tag("->"), tag("→")))))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, body) = cut(context("body expression in `forall`", expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Pi(name, ty, body))))
}

pub(super) fn assert_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("assert")(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = cut(context("`:` after `assert`", char(':')))(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in `assert`", ws1))(rest)?;
    let (rest, e) = cut(context("expression after `assert :`", expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Assert(e))))
}

/// `merge x y : T` (with type annotation)
pub(super) fn merge_annot_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = cut(context("whitespace after `merge`", ws1))(rest)?;
    let (rest, x) = cut(context("first argument to `merge`", import_expression))(rest)?;
    let (rest, _) = cut(context("whitespace between `merge` arguments", ws1))(rest)?;
    let (rest, y) = cut(context("second argument to `merge`", import_expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in `merge`", ws1))(rest)?;
    let (rest, ty) = cut(context("type annotation in `merge`", application))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::Merge(x, y, Some(ty))))))
}

/// `toMap x : T` (with type annotation)
pub(super) fn tomap_annot_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = cut(context("whitespace after `toMap`", ws1))(rest)?;
    let (rest, x) = cut(context("argument to `toMap`", import_expression))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = cut(context("whitespace after `:` in `toMap`", ws1))(rest)?;
    let (rest, ty) = cut(context("type annotation in `toMap`", application))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::ToMap(x, Some(ty))))))
}

/// `with` expression: `e with a.b.c = v`
/// ABNF: import-expression 1*(whsp1 with whsp1 with-clause)
pub(super) fn with_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, base) = import_expression(input)?;
    let (mut rest, mut expr) = with_clause(rest, base)?;
    loop {
        match with_clause(rest, expr.clone()) {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Parse a single `with` clause: `whsp1 "with" whsp1 path = value`.
pub(super) fn with_clause(input: Input<'_>, base: Expr) -> ParseResult<'_, Expr> {
    let (rest, _) = ws1(input)?;
    let (rest, _) = keyword("with")(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, first) = any_label_or_some(rest)?;
    let (rest, mut more) = many0(preceded(
        delimited(ws, char('.'), ws),
        any_label_or_some,
    ))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('=')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, val) = operator_expression(rest)?;
    let mut labels = vec![first];
    labels.append(&mut more);
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::With(base, labels, val)))))
}

/// Arrow type: `A -> B` (non-dependent function type)
/// ABNF: operator-expression whsp arrow whsp expression
/// Falls through to annotated-expression if no arrow found.
pub(super) fn arrow_or_annot_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, lhs) = operator_expression(input)?;
    // Try arrow
    let tried_arrow = (|| -> ParseResult<Expr> {
        let (r, _) = ws(rest)?;
        let (r, _) = alt((tag("->"), tag("→")))(r)?;
        let (r, _) = ws(r)?;
        let (r, rhs) = expression(r)?;
        Ok((r, spanned(input, r, ExprKind::Pi("_".into(), lhs.clone(), rhs))))
    })();
    if let Ok((r, e)) = tried_arrow {
        return Ok((r, e));
    }
    // Try annotation
    let (rest, annot) = opt(|input| {
        let (r, _) = ws(input)?;
        let (r, _) = char(':')(r)?;
        let (r, _) = ws1(r)?;
        let (r, ty) = expression(r)?;
        Ok((r, ty))
    })(rest)?;
    match annot {
        Some(ty) => Ok((rest, spanned(input, rest, ExprKind::Annot(lhs, ty)))),
        None => Ok((rest, lhs)),
    }
}

/// Top-level expression parser.
pub(super) fn expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    preceded(ws, alt((
        lambda_expression,
        let_expression,
        if_expression,
        forall_expression,
        with_expression,
        assert_expression,
        merge_annot_expression,
        tomap_annot_expression,
        empty_list_literal,
        arrow_or_annot_expression,
    )))(input)
}

/// Entry point: parse a complete Dhall expression.
pub fn parse_expr(input: &str) -> Result<Expr, String> {
    // Skip shebang lines if present.
    let mut input = input;
    while input.starts_with("#!") {
        input = input.find('\n').map_or("", |i| &input[i + 1..]);
    }
    let source: Rc<str> = Rc::from(input);
    let inp = Input::new(input, &source);
    let mut complete = terminated(expression, ws);
    match complete(inp) {
        Ok((rest, expr)) if rest.is_empty() => Ok(expr),
        Ok((rest, _)) => {
            let consumed = input.len() - rest.len();
            let before = &input[..consumed];
            let line = before.chars().filter(|&c| c == '\n').count() + 1;
            let last_nl = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
            let col = before[last_nl..].chars().count() + 1;

            let line_start = last_nl;
            let line_end = input[consumed..].find('\n').map(|i| consumed + i).unwrap_or(input.len());
            let source_line = &input[line_start..line_end];
            let caret_offset = col - 1;
            let caret = format!("{}^---", " ".repeat(caret_offset));
            let line_num_width = format!("{}", line).len();
            let padding = " ".repeat(line_num_width);

            let remaining = rest.fragment;
            let had_leading_ws = consumed > 0 && input.as_bytes()[consumed - 1].is_ascii_whitespace();
            let hint = diagnose_leftover(remaining, had_leading_ws, before);

            Err(format!(
                " --> {}:{}\n{} |\n{} | {}\n{} | {}\n{} |\n{} = {}",
                line, col, padding, line, source_line, padding, caret, padding, padding, hint
            ))
        }
        Err(e) => {
            let e = match e {
                nom::Err::Error(e) | nom::Err::Failure(e) => e,
                nom::Err::Incomplete(_) => unreachable!("complete parsers"),
            };
            Err(format_verbose_error(input, &e))
        }
    }
}

