use alloc::vec;
use nom::{branch::alt, bytes::complete::tag,
    character::complete::char,
    combinator::{cut, opt},
    error::context,
    multi::many0,
    sequence::{delimited, preceded, terminated}};
use super::input::Input;
use super::helpers::*;
use super::labels::*;
use super::imports::import_expr;
use super::structure::atom;
use super::expression::expression;
use crate::syntax::{Expr, ExprKind};
use crate::operations::OpKind;

/// Field access and projection: `e.x`, `e.{ x, y }`, `e.(T)`
pub(super) fn selector_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    use alloc::collections::BTreeSet;
    let (mut rest, mut expr) = atom(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = char('.')(r)?;
            let (r, _) = ws(r)?;
            let (r, sel) = alt((
                // .{ x, y } — projection (with optional leading comma)
                |r| -> ParseResult<Expr> {
                    let (r, _) = terminated(char('{'), ws)(r)?;
                    let (r, has_leading) = opt(terminated(char(','), ws))(r)?;
                    let (r, ls) = if let Ok((r2, first)) = any_label_or_some(r) {
                        let (r2, _) = ws(r2)?;
                        let (r2, mut more) = many0(|input| {
                            let (r, _) = char(',')(input)?;
                            let (r, _) = ws(r)?;
                            let (r, l) = any_label_or_some(r)?;
                            let (r, _) = ws(r)?;
                            Ok((r, l))
                        })(r2)?;
                        let (r2, _) = opt(terminated(char(','), ws))(r2)?;
                        let mut ls = vec![first];
                        ls.append(&mut more);
                        (r2, ls)
                    } else if has_leading.is_some() {
                        return Err(nom::Err::Failure(nom::error::VerboseError {
                            errors: alloc::vec![(r, nom::error::VerboseErrorKind::Context("field name in projection (duplicate commas are not allowed)"))],
                        }));
                    } else {
                        (r, vec![])
                    };
                    let (r, _) = char('}')(r)?;
                    let mut set = BTreeSet::new();
                    for l in ls {
                        if !set.insert(l) {
                            return Err(nom::Err::Failure(nom::error::VerboseError {
                                errors: alloc::vec![(input, nom::error::VerboseErrorKind::Context("Duplicate field in projection"))],
                            }));
                        }
                    }
                    let sp = expr.span().union(&rest.span_since(input));
                    Ok((r, Expr::new(ExprKind::Op(OpKind::Projection(expr.clone(), set)), sp)))
                },
                // .(T) — projection by expression
                |r| {
                    let (r, e) = delimited(
                        terminated(char('('), ws),
                        expression,
                        preceded(ws, char(')')),
                    )(r)?;
                    let sp = expr.span().union(&rest.span_since(input));
                    Ok((r, Expr::new(ExprKind::Op(OpKind::ProjectionByExpr(expr.clone(), e)), sp)))
                },
                // .field — field access
                |r| {
                    let (r, l) = label(r)?;
                    let sp = expr.span().union(&rest.span_since(input));
                    Ok((r, Expr::new(ExprKind::Op(OpKind::Field(expr.clone(), l)), sp)))
                },
            ))(r)?;
            Ok((r, sel))
        })();
        match tried {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(nom::Err::Failure(e)) => return Err(nom::Err::Failure(e)),
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Completion: `T::r`
pub(super) fn completion_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut expr) = selector_expression(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = tag("::")(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = selector_expression(r)?;
            Ok((r, { let sp = expr.span().union(&r.span_since(input)); Expr::new(ExprKind::Op(OpKind::Completion(expr.clone(), rhs)), sp) }))
        })();
        match tried {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Keyword-prefixed application: `Some e`, `merge x y`, `toMap x`
pub(super) fn first_application(input: Input<'_>) -> ParseResult<'_, Expr> {
    alt((
        // Some e (mandatory whitespace after Some)
        some_application,
        // merge x y (without type annotation)
        merge_application,
        // toMap x (without type annotation)
        tomap_application,
        import_expression,
    ))(input)
}

pub(super) fn some_application(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("Some")(input)?;
    let (rest, _) = cut(context("whitespace after `Some`", ws1))(rest)?;
    let (rest, e) = cut(context("argument to `Some`", import_expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::SomeLit(e))))
}

pub(super) fn merge_application(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = cut(context("whitespace after `merge`", ws1))(rest)?;
    let (rest, x) = cut(context("first argument to `merge`", import_expression))(rest)?;
    let (rest, _) = cut(context("whitespace between `merge` arguments", ws1))(rest)?;
    let (rest, y) = cut(context("second argument to `merge`", import_expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::Merge(x, y, None)))))
}

pub(super) fn tomap_application(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = cut(context("whitespace after `toMap`", ws1))(rest)?;
    let (rest, x) = cut(context("argument to `toMap`", import_expression))(rest)?;
    Ok((rest, spanned(input, rest, ExprKind::Op(OpKind::ToMap(x, None)))))
}

/// import-expression = import / completion-expression
pub(super) fn import_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    alt((import_expr, completion_expression))(input)
}

/// Function application: `f a b` = `App(App(f, a), b)`
/// ABNF: first-application-expression *(whsp1 import-expression)
pub(super) fn application(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut expr) = first_application(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws1(rest)?;
            let (r, arg) = import_expression(r)?;
            Ok((r, arg))
        })();
        match tried {
            Ok((r, arg)) => {
                { let sp = expr.span().union(&arg.span()); expr = Expr::new(ExprKind::Op(OpKind::App(expr, arg)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

