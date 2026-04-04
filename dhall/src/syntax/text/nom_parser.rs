//! Dhall parser built on `nom` — a `no_std`-compatible alternative to the
//! `pest`-based parser in `parser.rs`.
//!
//! # Structure
//!
//! The parser follows the [Dhall ABNF grammar](https://github.com/dhall-lang/dhall-lang/blob/master/standard/dhall.abnf)
//! and produces the same `Expr` AST as the `pest` parser.
//!
//! Productions are organized bottom-up:
//! 1. Whitespace and comments
//! 2. Literals (numbers, text)
//! 3. Labels and variables
//! 4. Imports
//! 5. Operators (precedence climbing)
//! 6. Expressions (let, lambda, if, etc.)
//!
//! # Status
//!
//! This is a scaffold. Only a subset of the grammar is implemented.
//! The goal is to pass the dhall-lang spec tests incrementally.

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, digit1, multispace0, one_of},
    combinator::{map, map_res, opt, recognize, value},
    multi::{many0, separated_list0},
    sequence::{delimited, pair, preceded, terminated, tuple},
    IResult,
};

use crate::syntax::{
    Expr, ExprKind, InterpolatedText, InterpolatedTextContents, Label,
    NaiveDouble, Span, UnspannedExpr, V,
};

// ── Helpers ──────────────────────────────────────────────────────────

type ParseResult<'a, T> = IResult<&'a str, T>;

fn mkexpr(kind: UnspannedExpr) -> Expr {
    Expr::new(kind, Span::Artificial)
}

// ── 1. Whitespace and comments ───────────────────────────────────────

/// Skip whitespace and line comments (-- to end of line).
fn ws(input: &str) -> ParseResult<()> {
    let mut rest = input;
    loop {
        let (r, _) = multispace0(rest)?;
        rest = r;
        if let Ok((r, _)) = tag::<_, _, nom::error::Error<&str>>("--")(rest) {
            let (r, _) = take_while(|c: char| c != '\n')(r)?;
            rest = r;
        } else {
            break;
        }
    }
    Ok((rest, ()))
}

/// Wrap a parser to consume trailing whitespace.
fn lexeme<'a, O>(
    inner: impl FnMut(&'a str) -> ParseResult<'a, O>,
) -> impl FnMut(&'a str) -> ParseResult<'a, O> {
    terminated(inner, ws)
}

// ── 2. Literals ──────────────────────────────────────────────────────

fn natural_literal(input: &str) -> ParseResult<u64> {
    alt((
        // Hex: 0x...
        map_res(
            preceded(tag("0x"), take_while1(|c: char| c.is_ascii_hexdigit())),
            |s: &str| u64::from_str_radix(s, 16),
        ),
        // Decimal
        map_res(digit1, |s: &str| s.parse::<u64>()),
    ))(input)
}

fn integer_literal(input: &str) -> ParseResult<i64> {
    map_res(
        recognize(pair(one_of("+-"), digit1)),
        |s: &str| s.parse::<i64>(),
    )(input)
}

fn double_literal(input: &str) -> ParseResult<NaiveDouble> {
    alt((
        value(NaiveDouble::from(f64::NAN), tag("NaN")),
        value(NaiveDouble::from(f64::INFINITY), tag("Infinity")),
        value(NaiveDouble::from(f64::NEG_INFINITY), tag("-Infinity")),
        map_res(
            recognize(tuple((
                opt(one_of("+-")),
                digit1,
                tag("."),
                digit1,
                opt(recognize(tuple((one_of("eE"), opt(one_of("+-")), digit1)))),
            ))),
            |s: &str| s.parse::<f64>().map(NaiveDouble::from),
        ),
    ))(input)
}

/// Double-quoted string literal (simplified — no interpolation yet).
fn double_quote_literal(input: &str) -> ParseResult<String> {
    // TODO: escape sequences, interpolation
    delimited(
        char('"'),
        map(
            take_while(|c: char| c != '"' && c != '\\'),
            |s: &str| s.to_owned(),
        ),
        char('"'),
    )(input)
}

// ── 3. Labels and variables ──────────────────────────────────────────

/// Reserved words that cannot be used as labels.
const RESERVED: &[&str] = &[
    "if", "then", "else", "let", "in", "using", "missing", "as",
    "Infinity", "NaN", "merge", "Some", "toMap", "assert", "forall",
    "with",
];

fn is_label_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/'
}

fn simple_label(input: &str) -> ParseResult<Label> {
    let (rest, name) = recognize(pair(
        take_while1(is_label_start),
        take_while(is_label_char),
    ))(input)?;

    if RESERVED.contains(&name) {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }

    Ok((rest, Label::from(name)))
}

fn backtick_label(input: &str) -> ParseResult<Label> {
    delimited(
        char('`'),
        map(take_while1(|c: char| c != '`'), Label::from),
        char('`'),
    )(input)
}

fn label(input: &str) -> ParseResult<Label> {
    lexeme(alt((backtick_label, simple_label)))(input)
}

fn variable(input: &str) -> ParseResult<V> {
    let (rest, l) = label(input)?;
    // Optional @n index
    let (rest, idx) = opt(preceded(
        lexeme(char('@')),
        lexeme(natural_literal),
    ))(rest)?;
    Ok((rest, V(l, idx.unwrap_or(0) as usize)))
}

// ── 4. Builtins ──────────────────────────────────────────────────────

fn builtin(input: &str) -> ParseResult<UnspannedExpr> {
    let (rest, name) = recognize(pair(
        take_while1(is_label_start),
        take_while(is_label_char),
    ))(input)?;

    let expr = match name {
        "True" => ExprKind::Num(crate::syntax::NumKind::Bool(true)),
        "False" => ExprKind::Num(crate::syntax::NumKind::Bool(false)),
        "Type" => ExprKind::Const(crate::syntax::Const::Type),
        "Kind" => ExprKind::Const(crate::syntax::Const::Kind),
        "Sort" => ExprKind::Const(crate::syntax::Const::Sort),
        _ => match crate::builtins::Builtin::parse(name) {
            Some(b) => ExprKind::Builtin(b),
            None => return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            ))),
        },
    };
    Ok((rest, expr))
}

// ── 5. Atoms (primitive expressions) ─────────────────────────────────

fn atom(input: &str) -> ParseResult<Expr> {
    lexeme(alt((
        // Parenthesized expression
        delimited(
            lexeme(char('(')),
            expression,
            lexeme(char(')')),
        ),
        // Numeric literals (order matters: double before natural)
        map(double_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Double(n)))),
        map(integer_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Integer(n)))),
        map(natural_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Natural(n)))),
        // Text literal
        map(double_quote_literal, |s| {
            mkexpr(ExprKind::TextLit(InterpolatedText::from(s)))
        }),
        // Record literal/type: { ... }
        record_literal_or_type,
        // List literal: [ ... ]
        list_literal,
        // Builtins and constants (must come before variable)
        map(builtin, mkexpr),
        // Variable
        map(variable, |v| mkexpr(ExprKind::Var(v))),
    )))(input)
}

// ── 6. Records ───────────────────────────────────────────────────────

fn record_literal_or_type(input: &str) -> ParseResult<Expr> {
    delimited(
        lexeme(char('{')),
        alt((
            // { = } — empty record literal
            map(lexeme(char('=')), |_| mkexpr(ExprKind::RecordLit(Default::default()))),
            // { field = expr, ... } or { field : type, ... }
            map(
                separated_list0(lexeme(char(',')), record_entry),
                |entries| {
                    if entries.is_empty() {
                        return mkexpr(ExprKind::RecordType(Default::default()));
                    }
                    // Determine if this is a literal or type by the separator used
                    let is_type = entries.iter().all(|(_, sep, _)| *sep == ':');
                    let map = entries.into_iter().map(|(l, _, e)| (l, e)).collect();
                    if is_type {
                        mkexpr(ExprKind::RecordType(map))
                    } else {
                        mkexpr(ExprKind::RecordLit(map))
                    }
                },
            ),
        )),
        lexeme(char('}')),
    )(input)
}

fn record_entry(input: &str) -> ParseResult<(Label, char, Expr)> {
    tuple((
        label,
        lexeme(alt((char('='), char(':')))),
        expression,
    ))(input)
}

// ── 7. Lists ─────────────────────────────────────────────────────────

fn list_literal(input: &str) -> ParseResult<Expr> {
    delimited(
        lexeme(char('[')),
        map(
            separated_list0(lexeme(char(',')), expression),
            |items| {
                if items.is_empty() {
                    // Empty list needs a type annotation — this will be
                    // handled at a higher level. For now, produce NEListLit
                    // which will fail if truly empty.
                    mkexpr(ExprKind::NEListLit(items))
                } else {
                    mkexpr(ExprKind::NEListLit(items))
                }
            },
        ),
        lexeme(char(']')),
    )(input)
}

// ── 8. Application ───────────────────────────────────────────────────

/// Function application: `f a b` = `App(App(f, a), b)`
fn application(input: &str) -> ParseResult<Expr> {
    let (rest, first) = atom(input)?;
    let (rest, args) = many0(atom)(rest)?;
    Ok((
        rest,
        args.into_iter().fold(first, |acc, arg| {
            mkexpr(ExprKind::Op(crate::operations::OpKind::App(acc, arg)))
        }),
    ))
}

// ── 9. Operators (simplified precedence) ─────────────────────────────
//
// Full implementation needs a precedence climber for all 13 Dhall operators.
// For now, only + and ++ are handled as a proof of concept.

fn operator_expression(input: &str) -> ParseResult<Expr> {
    let (rest, first) = application(input)?;
    let (rest, pairs) = many0(pair(
        lexeme(alt((
            value(crate::operations::BinOp::NaturalPlus, tag("+")),
            value(crate::operations::BinOp::TextAppend, tag("++")),
            value(crate::operations::BinOp::ListAppend, tag("#")),
        ))),
        application,
    ))(rest)?;
    Ok((
        rest,
        pairs.into_iter().fold(first, |acc, (op, rhs)| {
            mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(op, acc, rhs)))
        }),
    ))
}

// ── 10. Top-level expressions ────────────────────────────────────────

fn let_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = lexeme(tag("let"))(input)?;
    let (rest, name) = label(rest)?;
    // Optional type annotation
    let (rest, annot) = opt(preceded(lexeme(char(':')), expression))(rest)?;
    let (rest, _) = lexeme(char('='))(rest)?;
    let (rest, val) = expression(rest)?;
    let (rest, _) = lexeme(tag("in"))(rest)?;
    let (rest, body) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Let(name, annot, val, body))))
}

fn lambda_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = lexeme(alt((tag("\\"), tag("λ"))))(input)?;
    let (rest, _) = lexeme(char('('))(rest)?;
    let (rest, name) = label(rest)?;
    let (rest, _) = lexeme(char(':'))(rest)?;
    let (rest, ty) = expression(rest)?;
    let (rest, _) = lexeme(char(')'))(rest)?;
    let (rest, _) = lexeme(alt((tag("->"), tag("→"))))(rest)?;
    let (rest, body) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Lam(name, ty, body))))
}

fn if_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = lexeme(tag("if"))(input)?;
    let (rest, cond) = expression(rest)?;
    let (rest, _) = lexeme(tag("then"))(rest)?;
    let (rest, t) = expression(rest)?;
    let (rest, _) = lexeme(tag("else"))(rest)?;
    let (rest, f) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::BoolIf(cond, t, f)))))
}

fn forall_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = lexeme(alt((tag("forall"), tag("∀"))))(input)?;
    let (rest, _) = lexeme(char('('))(rest)?;
    let (rest, name) = label(rest)?;
    let (rest, _) = lexeme(char(':'))(rest)?;
    let (rest, ty) = expression(rest)?;
    let (rest, _) = lexeme(char(')'))(rest)?;
    let (rest, _) = lexeme(alt((tag("->"), tag("→"))))(rest)?;
    let (rest, body) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Pi(name, ty, body))))
}

fn annot_expression(input: &str) -> ParseResult<Expr> {
    let (rest, e) = operator_expression(input)?;
    let (rest, annot) = opt(preceded(lexeme(char(':')), expression))(rest)?;
    match annot {
        Some(ty) => Ok((rest, mkexpr(ExprKind::Annot(e, ty)))),
        None => Ok((rest, e)),
    }
}

/// Top-level expression parser.
pub fn expression(input: &str) -> ParseResult<Expr> {
    preceded(ws, alt((
        lambda_expression,
        let_expression,
        if_expression,
        forall_expression,
        annot_expression,
    )))(input)
}

/// Entry point: parse a complete Dhall expression.
pub fn parse_expr(input: &str) -> Result<Expr, String> {
    match expression(input.trim()) {
        Ok(("", expr)) => Ok(expr),
        Ok((rest, _)) => Err(format!("Unexpected trailing input: {:?}", rest)),
        Err(e) => Err(format!("Parse error: {:?}", e)),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_natural() {
        let e = parse_expr("42").unwrap();
        assert_eq!(e.to_string(), "42");
    }

    #[test]
    fn test_integer() {
        let e = parse_expr("+5").unwrap();
        assert_eq!(e.to_string(), "+5");
    }

    #[test]
    fn test_bool() {
        let e = parse_expr("True").unwrap();
        assert_eq!(e.to_string(), "True");
    }

    #[test]
    fn test_variable() {
        let e = parse_expr("x").unwrap();
        assert_eq!(e.to_string(), "x");
    }

    #[test]
    fn test_addition() {
        let e = parse_expr("1 + 2").unwrap();
        assert_eq!(e.to_string(), "1 + 2");
    }

    #[test]
    fn test_application() {
        let e = parse_expr("f x y").unwrap();
        assert_eq!(e.to_string(), "f x y");
    }

    #[test]
    fn test_lambda() {
        let e = parse_expr("\\(x : Natural) -> x").unwrap();
        assert_eq!(e.to_string(), "λ(x : Natural) → x");
    }

    #[test]
    fn test_let() {
        let e = parse_expr("let x = 1 in x").unwrap();
        assert_eq!(e.to_string(), "let x = 1 in x");
    }

    #[test]
    fn test_record_literal() {
        let e = parse_expr("{ x = 1, y = 2 }").unwrap();
        let s = e.to_string();
        assert!(s.contains("x") && s.contains("1") && s.contains("y") && s.contains("2"), "got: {}", s);
    }

    #[test]
    fn test_record_type() {
        let e = parse_expr("{ x : Natural, y : Natural }").unwrap();
        let s = e.to_string();
        assert!(s.contains("x") && s.contains("Natural"), "got: {}", s);
    }

    #[test]
    fn test_string() {
        let e = parse_expr(r#""hello""#).unwrap();
        assert_eq!(e.to_string(), "\"hello\"");
    }

    #[test]
    fn test_list() {
        let e = parse_expr("[1, 2, 3]").unwrap();
        let s = e.to_string();
        assert!(s.contains('1') && s.contains('2') && s.contains('3'), "got: {}", s);
    }

    #[test]
    fn test_if() {
        let e = parse_expr("if True then 1 else 0").unwrap();
        let s = e.to_string();
        assert!(s.contains("if") && s.contains("True"), "got: {}", s);
    }

    #[test]
    fn test_nested_let() {
        let e = parse_expr("let x = 1 in let y = 2 in x + y").unwrap();
        let s = e.to_string();
        assert!(s.contains("let") && s.contains("x") && s.contains("y"), "got: {}", s);
    }

    #[test]
    fn test_line_comment() {
        let e = parse_expr("1 -- this is a comment\n+ 2").unwrap();
        assert_eq!(e.to_string(), "1 + 2");
    }

    #[test]
    fn test_line_comment_at_end() {
        let e = parse_expr("42 -- trailing comment").unwrap();
        assert_eq!(e.to_string(), "42");
    }
}
