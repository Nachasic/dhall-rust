use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use nom::{branch::alt, bytes::complete::{tag, take_while1},
    character::complete::{char, digit1, one_of},
    combinator::{cut, map, map_res, opt, recognize, value},
    error::context,
    multi::many0,
    sequence::{delimited, preceded, terminated, tuple}};
use super::input::Input;
use super::helpers::*;
use super::expression::expression;
use crate::syntax::{Expr, InterpolatedText, InterpolatedTextContents, NaiveDouble};

pub(super) fn natural_literal(input: Input<'_>) -> ParseResult<'_, u64> {
    alt((
        // Hex: 0x...
        map_res(
            preceded(tag("0x"), take_while1(|c: char| c.is_ascii_hexdigit())),
            |s: Input<'_>| u64::from_str_radix(s.fragment, 16),
        ),
        // Decimal (reject leading zeros like 042)
        decimal_natural,
    ))(input)
}

pub(super) fn decimal_natural(input: Input<'_>) -> ParseResult<'_, u64> {
    let (rest, s) = digit1(input)?;
    if s.fragment.len() > 1 && s.fragment.starts_with('0') {
        Err(tag_err(input))
    } else {
        s.fragment.parse::<u64>()
            .map(|n| (rest, n))
            .map_err(|_| tag_err(input))
    }
}

pub(super) fn integer_literal(input: Input<'_>) -> ParseResult<'_, i64> {
    let (rest, sign) = one_of("+-")(input)?;
    let (rest, n) = natural_literal(rest)?;
    let val = if sign == '-' { -(n as i64) } else { n as i64 };
    Ok((rest, val))
}

pub(super) fn double_literal(input: Input<'_>) -> ParseResult<'_, NaiveDouble> {
    alt((
        value(NaiveDouble::from(f64::NAN), tag("NaN")),
        value(NaiveDouble::from(f64::INFINITY), tag("Infinity")),
        value(NaiveDouble::from(f64::NEG_INFINITY), tag("-Infinity")),
        // With dot: 1.0, 1.0e5
        map_res(
            recognize(tuple((
                opt(one_of("+-")),
                digit1,
                tag("."),
                digit1,
                opt(recognize(tuple((one_of("eE"), opt(one_of("+-")), digit1)))),
            ))),
            |s: Input<'_>| s.fragment.parse::<f64>()
                .map_err(|e| format!("{}", e))
                .and_then(|f| if f.is_infinite() { Err("out of range".to_owned()) } else { Ok(NaiveDouble::from(f)) }),
        ),
        // Without dot: 1e4, -1E+5
        map_res(
            recognize(tuple((
                opt(one_of("+-")),
                digit1,
                one_of("eE"),
                opt(one_of("+-")),
                digit1,
            ))),
            |s: Input<'_>| s.fragment.parse::<f64>()
                .map_err(|e| format!("{}", e))
                .and_then(|f| if f.is_infinite() { Err("out of range".to_owned()) } else { Ok(NaiveDouble::from(f)) }),
        ),
    ))(input)
}

/// Check if a Unicode codepoint is a non-character (per Dhall spec).
pub(super) fn is_noncharacter(n: u32) -> bool {
    // Non-characters: 0xNFFFE and 0xNFFFF for each plane 0-16
    (n & 0xFFFE) == 0xFFFE
}

/// Double-quoted string escape sequence.
pub(super) fn double_quote_escaped(input: Input<'_>) -> ParseResult<'_, String> {
    preceded(char('\\'), alt((
        value("\"".to_owned(), char('"')),
        value("$".to_owned(), char('$')),
        value("\\".to_owned(), char('\\')),
        value("/".to_owned(), char('/')),
        value("\u{0008}".to_owned(), char('b')),
        value("\u{000C}".to_owned(), char('f')),
        value("\n".to_owned(), char('n')),
        value("\r".to_owned(), char('r')),
        value("\t".to_owned(), char('t')),
        // Unicode escape: \uXXXX or \u{XXXXX}
        preceded(char('u'), alt((
            // \u{XXXXX}
            map_res(
                delimited(char('{'), take_while1(|c: char| c.is_ascii_hexdigit()), char('}')),
                |s: Input<'_>| u32::from_str_radix(s.fragment, 16)
                    .map_err(|e| format!("{}", e))
                    .and_then(|n| if is_noncharacter(n) { Err("non-character".to_owned()) } else { Ok(n) })
                    .and_then(|n| char::from_u32(n).ok_or_else(|| "invalid codepoint".to_owned()))
                    .map(|c| c.to_string()),
            ),
            // \uXXXX (exactly 4 hex digits)
            map_res(
                recognize(tuple((
                    one_of("0123456789abcdefABCDEF"),
                    one_of("0123456789abcdefABCDEF"),
                    one_of("0123456789abcdefABCDEF"),
                    one_of("0123456789abcdefABCDEF"),
                ))),
                |s: Input<'_>| u32::from_str_radix(s.fragment, 16)
                    .map_err(|e| format!("{}", e))
                    .and_then(|n| if is_noncharacter(n) { Err("non-character".to_owned()) } else { Ok(n) })
                    .and_then(|n| char::from_u32(n).ok_or_else(|| "invalid codepoint".to_owned()))
                    .map(|c| c.to_string()),
            ),
        ))),
    )))(input)
}

/// A chunk of a double-quoted string: text, escape, or interpolation.
pub(super) fn double_quote_chunk(input: Input<'_>) -> ParseResult<'_, InterpolatedTextContents<Expr>> {
    alt((
        // Interpolation: ${expr}
        map(
            delimited(tag("${"), expression, preceded(ws, char('}'))),
            InterpolatedTextContents::Expr,
        ),
        // Escape sequence
        map(double_quote_escaped, InterpolatedTextContents::Text),
        // Plain text (no ", \, or ${ )
        map(
            take_while1(|c: char| c != '"' && c != '\\' && c != '$'),
            |s: Input<'_>| InterpolatedTextContents::Text(s.fragment.to_owned()),
        ),
        // A lone $ that isn't followed by {
        map(char('$'), |_| InterpolatedTextContents::Text("$".to_owned())),
    ))(input)
}

/// Double-quoted string literal with escapes and interpolation.
pub(super) fn double_quote_literal(input: Input<'_>) -> ParseResult<'_, InterpolatedText<Expr>> {
    delimited(
        char('"'),
        map(many0(double_quote_chunk), |chunks| chunks.into_iter().collect()),
        char('"'),
    )(input)
}

/// A chunk of a single-quoted (multi-line) string.
pub(super) fn single_quote_chunk(input: Input<'_>) -> ParseResult<'_, InterpolatedTextContents<Expr>> {
    alt((
        // Escaped sequences specific to multi-line strings
        value(InterpolatedTextContents::Text("''".to_owned()), tag("'''")),
        value(InterpolatedTextContents::Text("${".to_owned()), tag("''${")),
        // Interpolation
        map(
            delimited(tag("${"), expression, preceded(ws, char('}'))),
            InterpolatedTextContents::Expr,
        ),
        // Plain text: anything that isn't '' or ${
        map(
            take_while1(|c: char| c != '\'' && c != '$'),
            |s: Input<'_>| InterpolatedTextContents::Text(s.fragment.to_owned()),
        ),
        // A lone ' that isn't followed by another '
        map(
            terminated(char('\''), nom::combinator::not(char('\''))),
            |_| InterpolatedTextContents::Text("'".to_owned()),
        ),
        // A lone $ that isn't followed by {
        map(
            terminated(char('$'), nom::combinator::not(char('{'))),
            |_| InterpolatedTextContents::Text("$".to_owned()),
        ),
    ))(input)
}

/// Multi-line (single-quoted) string literal with indent stripping.
pub(super) fn single_quote_literal(input: Input<'_>) -> ParseResult<'_, InterpolatedText<Expr>> {
    let (rest, _) = tag("''")(input)?;
    // Must be followed by newline (the opening '' must be on its own line-end)
    let (rest, _) = cut(context("newline after opening `''` (multi-line strings require a newline after `''`)", alt((tag("\r\n"), tag("\n")))))(rest)?;
    let (rest, chunks) = many0(single_quote_chunk)(rest)?;
    let (rest, _) = tag("''")(rest)?;

    // Build lines by splitting on newlines within Text chunks.
    let mut lines: Vec<Vec<InterpolatedTextContents<Expr>>> = vec![vec![]];
    for chunk in chunks {
        match chunk {
            InterpolatedTextContents::Text(ref s) => {
                // Split text on newlines to form lines.
                let mut parts = s.split('\n');
                if let Some(first) = parts.next() {
                    if !first.is_empty() {
                        lines.last_mut().unwrap().push(
                            InterpolatedTextContents::Text(first.to_owned()),
                        );
                    }
                    for part in parts {
                        lines.push(vec![]);
                        if !part.is_empty() {
                            lines.last_mut().unwrap().push(
                                InterpolatedTextContents::Text(part.to_owned()),
                            );
                        }
                    }
                }
            }
            expr => lines.last_mut().unwrap().push(expr),
        }
    }

    // Compute minimum indent from non-empty lines.
    let min_indent = lines.iter().filter_map(|line| {
        match line.first() {
            Some(InterpolatedTextContents::Text(s)) if !s.is_empty() || line.len() > 1 => {
                Some(s.len() - s.trim_start_matches(|c: char| c == ' ' || c == '\t').len())
            }
            Some(InterpolatedTextContents::Expr(_)) => Some(0),
            _ => None, // empty line, skip
        }
    }).min().unwrap_or(0);

    // Strip indent and reassemble.
    let result: InterpolatedText<Expr> = itertools::Itertools::intersperse(
        lines.into_iter().map(|mut line| {
            // Strip indent from the first text chunk of each line.
            if min_indent > 0 {
                if let Some(InterpolatedTextContents::Text(s)) = line.first_mut() {
                    if s.len() >= min_indent {
                        *s = s[min_indent..].to_owned();
                    }
                }
            }
            line.into_iter().collect::<InterpolatedText<Expr>>()
        }),
        InterpolatedText::from("\n".to_owned()),
    )
    .flat_map(InterpolatedText::into_iter)
    .collect();

    Ok((rest, result))
}

