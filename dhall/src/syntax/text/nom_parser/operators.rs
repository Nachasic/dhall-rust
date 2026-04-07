use nom::{branch::alt, bytes::complete::tag,
    character::complete::char,
    combinator::value};
use super::input::Input;
use super::helpers::*;
use super::application::application;
use crate::syntax::{Expr, ExprKind};
use crate::operations::{BinOp, BinOp::*, OpKind};

// Lowest precedence at the top, highest at the bottom.
// All operators are left-associative.
// Each level parses its operator and delegates to the next level for operands.

/// Helper: build a left-associative binary operator parser for one precedence level.
macro_rules! binop_level {
    // Single operator — no alt() needed
    ($name:ident, $next:ident, $op_tag:expr => $op_variant:expr) => {
        fn $name(input: Input<'_>) -> ParseResult<'_, Expr> {
            let (mut rest, mut lhs) = $next(input)?;
            loop {
                let tried = (|| -> ParseResult<(crate::operations::BinOp, Expr)> {
                    let (r, _) = ws(rest)?;
                    let (r, _) = tag($op_tag)(r)?;
                    let (r, _) = ws(r)?;
                    let (r, rhs) = $next(r)?;
                    Ok((r, ($op_variant, rhs)))
                })();
                match tried {
                    Ok((r, (op, rhs))) => {
                        { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)), sp); }
                        rest = r;
                    }
                    Err(_) => break,
                }
            }
            Ok((rest, lhs))
        }
    };
    // Multiple operators — use alt()
    ($name:ident, $next:ident, $( $op_tag:expr => $op_variant:expr ),+ $(,)?) => {
        fn $name(input: Input<'_>) -> ParseResult<'_, Expr> {
            let (mut rest, mut lhs) = $next(input)?;
            loop {
                let tried = (|| -> ParseResult<(crate::operations::BinOp, Expr)> {
                    let (r, _) = ws(rest)?;
                    let (r, op) = alt((
                        $( value($op_variant, tag($op_tag)) ),+
                    ))(r)?;
                    let (r, _) = ws(r)?;
                    let (r, rhs) = $next(r)?;
                    Ok((r, (op, rhs)))
                })();
                match tried {
                    Ok((r, (op, rhs))) => {
                        { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)), sp); }
                        rest = r;
                    }
                    Err(_) => break,
                }
            }
            Ok((rest, lhs))
        }
    };
}

/// Match `==` but not `===`.
pub(super) fn op_bool_eq(input: Input<'_>) -> ParseResult<'_, BinOp> {
    let (rest, _) = tag("==")(input)?;
    if rest.starts_with_char('=') {
        Err(tag_err(input))
    } else {
        Ok((rest, BoolEQ))
    }
}

// Ordering matters: longer tokens must come first to avoid prefix matches.
binop_level!(equiv_expr,                   import_alt_expr,    "===" => Equivalence, "≡" => Equivalence);

/// `?` requires mandatory whitespace after to disambiguate `http://a/a?a`
/// ABNF: or-expression *(whsp "?" whsp1 or-expression)
pub(super) fn import_alt_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = or_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = char('?')(r)?;
            let (r, _) = ws1(r)?;
            let (r, rhs) = or_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(ImportAlt, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

binop_level!(or_expr,                      text_append_expr,   "||" => BoolOr);
binop_level!(text_append_expr,             plus_expr,          "++" => TextAppend);
binop_level!(list_append_expr,             and_expr,           "#" => ListAppend);

/// `+` requires mandatory whitespace after to disambiguate `f +2`
/// ABNF: text-append-expression *(whsp "+" whsp1 text-append-expression)
pub(super) fn plus_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = list_append_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = char('+')(r)?;
            // Reject ++ (that's text append)
            if r.starts_with_char('+') {
                return Err(tag_err(rest));
            }
            let (r, _) = ws1(r)?;
            let (r, rhs) = list_append_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(NaturalPlus, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}
binop_level!(and_expr,                     combine_expr,       "&&" => BoolAnd);
binop_level!(times_expr,                   bool_eq_expr,       "*" => NaturalTimes);
binop_level!(ne_expr,                      application,        "!=" => BoolNE);

// combine, prefer, combine_types need hand-written parsers because
// /\ vs // vs //\\ are ambiguous prefixes.

pub(super) fn combine_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = prefer_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = alt((tag("∧"), tag("/\\")))(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = prefer_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(RecursiveRecordMerge, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// Match `//` but not `//\\`
pub(super) fn op_prefer_ascii(input: Input<'_>) -> ParseResult<'_, ()> {
    let (rest, _) = tag("//")(input)?;
    if rest.starts_with_char('\\') {
        Err(tag_err(input))
    } else {
        Ok((rest, ()))
    }
}

pub(super) fn prefer_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = combine_types_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = alt((value((), tag("⫽")), op_prefer_ascii))(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = combine_types_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(RightBiasedRecordMerge, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

pub(super) fn combine_types_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = times_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = alt((tag("⩓"), tag("//\\\\")))(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = times_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(RecursiveRecordTypeMerge, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// `==` level needs special handling to not consume `===`.
pub(super) fn bool_eq_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = ne_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<(crate::operations::BinOp, Expr)> {
            let (r, _) = ws(rest)?;
            let (r, op) = op_bool_eq(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = ne_expr(r)?;
            Ok((r, (op, rhs)))
        })();
        match tried {
            Ok((r, (op, rhs))) => {
                { let sp = lhs.span().union(&rhs.span()); lhs = Expr::new(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)), sp); }
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

pub(super) fn operator_expression(input: Input<'_>) -> ParseResult<'_, Expr> {
    equiv_expr(input)
}

