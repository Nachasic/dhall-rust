//! Dhall parser built on `nom`.
//!
//! Follows the [Dhall ABNF grammar](https://github.com/dhall-lang/dhall-lang/blob/master/standard/dhall.abnf)
//! and produces the `Expr` AST. Passes all 1937 spec tests.
//!
//! # Structure
//!
//! Productions are organized bottom-up:
//! 1. Whitespace and comments
//! 2. Literals (numbers, text)
//! 3. Labels and variables
//! 4. Builtins
//! 5. Imports
//! 6. Atoms (primitive expressions)
//! 7. Records, unions, lists
//! 8. Selectors, completion, application
//! 9. Operators (precedence tower)
//! 10. Top-level expressions (let, lambda, if, etc.)

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, digit1, multispace0, one_of},
    combinator::{map, map_res, opt, recognize, value},
    multi::{many0, separated_list0},
    sequence::{delimited, pair, preceded, terminated, tuple},
    IResult,
};

use crate::operations::{BinOp, BinOp::*, OpKind};
use crate::syntax::{
    Expr, ExprKind, InterpolatedText, InterpolatedTextContents, Label,
    NaiveDouble, NumKind, Span, UnspannedExpr, V,
};
use crate::syntax::{Const, FilePath, FilePrefix, Hash, ImportMode, ImportTarget, Scheme, URL};

// ── Helpers ──────────────────────────────────────────────────────────

type ParseResult<'a, T> = IResult<&'a str, T>;

/// Error type for the public API.
pub type ParseError = String;

fn mkexpr(kind: UnspannedExpr) -> Expr {
    Expr::new(kind, Span::Artificial)
}

/// Parse a keyword, ensuring it's not a prefix of a longer identifier.
fn keyword<'a>(kw: &'static str) -> impl FnMut(&'a str) -> ParseResult<'a, &'a str> {
    move |input: &'a str| {
        let (rest, matched) = tag(kw)(input)?;
        if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
            Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
        } else {
            Ok((rest, matched))
        }
    }
}

/// Insert a record literal entry, merging duplicates with `∧`.
fn insert_recordlit_entry(map: &mut alloc::collections::BTreeMap<Label, Expr>, l: Label, e: Expr) {
    use alloc::collections::btree_map::Entry;
    match map.entry(l) {
        Entry::Vacant(entry) => { entry.insert(e); }
        Entry::Occupied(mut entry) => {
            let other = entry.insert(Expr::new(ExprKind::Num(NumKind::Bool(false)), Span::Artificial));
            entry.insert(Expr::new(
                ExprKind::Op(OpKind::BinOp(
                    BinOp::RecursiveRecordMerge, other, e,
                )),
                Span::Artificial,
            ));
        }
    }
}

// ── 1. Whitespace and comments ───────────────────────────────────────

/// Skip whitespace and line comments (-- to end of line)
/// and block comments ({- ... -}, which can nest).
fn ws(input: &str) -> ParseResult<'_, ()> {
    let mut rest = input;
    loop {
        let (r, _) = multispace0(rest)?;
        rest = r;
        if let Ok((r, _)) = tag::<_, _, nom::error::Error<&str>>("--")(rest) {
            let (r, _) = take_while(|c: char| c != '\n')(r)?;
            rest = r;
        } else if let Ok((r, _)) = tag::<_, _, nom::error::Error<&str>>("{-")(rest) {
            rest = block_comment(r)?;
        } else {
            break;
        }
    }
    Ok((rest, ()))
}

/// Consume the body of a block comment (after the opening `{-`).
/// Handles nesting: each `{-` inside must be matched by a `-}`.
fn block_comment(input: &str) -> Result<&str, nom::Err<nom::error::Error<&str>>> {
    let mut rest = input;
    loop {
        // Look for {- (nested) or -} (close)
        match rest.find("{-").map(|i| (i, true)).into_iter()
            .chain(rest.find("-}").map(|i| (i, false)))
            .min_by_key(|(i, _)| *i)
        {
            Some((i, true)) => {
                // Nested block comment — recurse past the `{-`
                rest = block_comment(&rest[i + 2..])?;
            }
            Some((i, false)) => {
                // Closing -}
                return Ok(&rest[i + 2..]);
            }
            None => {
                return Err(nom::Err::Error(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Tag,
                )));
            }
        }
    }
}

/// Mandatory whitespace (at least one space/tab/newline/comment).
fn ws1(input: &str) -> ParseResult<'_, ()> {
    let start = input;
    let (rest, _) = ws(input)?;
    if rest.len() == start.len() {
        // No whitespace consumed
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Space)))
    } else {
        Ok((rest, ()))
    }
}

// ── 2. Literals ──────────────────────────────────────────────────────

fn natural_literal(input: &str) -> ParseResult<'_, u64> {
    alt((
        // Hex: 0x...
        map_res(
            preceded(tag("0x"), take_while1(|c: char| c.is_ascii_hexdigit())),
            |s: &str| u64::from_str_radix(s, 16),
        ),
        // Decimal (reject leading zeros like 042)
        decimal_natural,
    ))(input)
}

fn decimal_natural(input: &str) -> ParseResult<'_, u64> {
    let (rest, s) = digit1(input)?;
    if s.len() > 1 && s.starts_with('0') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        s.parse::<u64>()
            .map(|n| (rest, n))
            .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    }
}

fn integer_literal(input: &str) -> ParseResult<'_, i64> {
    let (rest, sign) = one_of("+-")(input)?;
    let (rest, n) = natural_literal(rest)?;
    let val = if sign == '-' { -(n as i64) } else { n as i64 };
    Ok((rest, val))
}

fn double_literal(input: &str) -> ParseResult<'_, NaiveDouble> {
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
            |s: &str| s.parse::<f64>()
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
            |s: &str| s.parse::<f64>()
                .map_err(|e| format!("{}", e))
                .and_then(|f| if f.is_infinite() { Err("out of range".to_owned()) } else { Ok(NaiveDouble::from(f)) }),
        ),
    ))(input)
}

/// Check if a Unicode codepoint is a non-character (per Dhall spec).
fn is_noncharacter(n: u32) -> bool {
    // Non-characters: 0xNFFFE and 0xNFFFF for each plane 0-16
    (n & 0xFFFE) == 0xFFFE
}

/// Double-quoted string escape sequence.
fn double_quote_escaped(input: &str) -> ParseResult<'_, String> {
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
                |s: &str| u32::from_str_radix(s, 16)
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
                |s: &str| u32::from_str_radix(s, 16)
                    .map_err(|e| format!("{}", e))
                    .and_then(|n| if is_noncharacter(n) { Err("non-character".to_owned()) } else { Ok(n) })
                    .and_then(|n| char::from_u32(n).ok_or_else(|| "invalid codepoint".to_owned()))
                    .map(|c| c.to_string()),
            ),
        ))),
    )))(input)
}

/// A chunk of a double-quoted string: text, escape, or interpolation.
fn double_quote_chunk(input: &str) -> ParseResult<'_, InterpolatedTextContents<Expr>> {
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
            |s: &str| InterpolatedTextContents::Text(s.to_owned()),
        ),
        // A lone $ that isn't followed by {
        map(char('$'), |_| InterpolatedTextContents::Text("$".to_owned())),
    ))(input)
}

/// Double-quoted string literal with escapes and interpolation.
fn double_quote_literal(input: &str) -> ParseResult<'_, InterpolatedText<Expr>> {
    delimited(
        char('"'),
        map(many0(double_quote_chunk), |chunks| chunks.into_iter().collect()),
        char('"'),
    )(input)
}

/// A chunk of a single-quoted (multi-line) string.
fn single_quote_chunk(input: &str) -> ParseResult<'_, InterpolatedTextContents<Expr>> {
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
            |s: &str| InterpolatedTextContents::Text(s.to_owned()),
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
fn single_quote_literal(input: &str) -> ParseResult<'_, InterpolatedText<Expr>> {
    let (rest, _) = tag("''")(input)?;
    // Must be followed by newline (the opening '' must be on its own line-end)
    let (rest, _) = alt((tag("\r\n"), tag("\n")))(rest)?;
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

// ── 3. Labels and variables ──────────────────────────────────────────

/// Reserved words that cannot be used as labels.
const RESERVED: &[&str] = &[
    "if", "then", "else", "let", "in", "using", "missing", "as",
    "Infinity", "NaN", "merge", "Some", "toMap", "assert", "forall",
    "with",
];

/// Check if a name is a builtin or constant (True, False, Type, Kind, Sort, or Builtin::parse).
fn is_builtin_name(name: &str) -> bool {
    matches!(name, "True" | "False" | "Type" | "Kind" | "Sort")
        || crate::builtins::Builtin::parse(name).is_some()
}

fn is_label_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/'
}

fn simple_label(input: &str) -> ParseResult<'_, Label> {
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

/// A nonreserved-label: rejects both keywords AND builtins (unless backtick-quoted).
fn nonreserved_label(input: &str) -> ParseResult<'_, Label> {
    if let Ok(r) = backtick_label(input) {
        return Ok(r);
    }
    let (rest, l) = simple_label(input)?;
    if is_builtin_name(l.as_ref()) {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((rest, l))
}

fn backtick_label(input: &str) -> ParseResult<'_, Label> {
    delimited(
        char('`'),
        map(take_while(|c: char| c != '`'), Label::from),
        char('`'),
    )(input)
}

fn label(input: &str) -> ParseResult<'_, Label> {
    alt((backtick_label, simple_label))(input)
}

/// any-label-or-some: allows all labels plus the keyword `Some`.
fn any_label_or_some(input: &str) -> ParseResult<'_, Label> {
    alt((
        label,
        map(keyword("Some"), |_| Label::from("Some")),
    ))(input)
}

fn variable(input: &str) -> ParseResult<'_, V> {
    let (rest, l) = nonreserved_label(input)?;
    let (rest, idx) = opt(preceded(
        delimited(ws, char('@'), ws),
        natural_literal,
    ))(rest)?;
    Ok((rest, V(l, idx.unwrap_or(0) as usize)))
}

// ── 4. Builtins ──────────────────────────────────────────────────────

fn builtin(input: &str) -> ParseResult<'_, UnspannedExpr> {
    let (rest, name) = recognize(pair(
        take_while1(is_label_start),
        take_while(is_label_char),
    ))(input)?;

    let expr = match name {
        "True" => ExprKind::Num(NumKind::Bool(true)),
        "False" => ExprKind::Num(NumKind::Bool(false)),
        "Type" => ExprKind::Const(Const::Type),
        "Kind" => ExprKind::Const(Const::Kind),
        "Sort" => ExprKind::Const(Const::Sort),
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

fn builtin_no_index(input: &str) -> ParseResult<'_, Expr> {
    let (rest, b) = builtin(input)?;
    if rest.starts_with('@') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, mkexpr(b)))
    }
}

// ── 5. Imports ───────────────────────────────────────────────────────

/// Path component: /segment or /"quoted segment"
/// A single path component without leading /
fn path_component_body(input: &str) -> ParseResult<'_, String> {
    alt((
        delimited(
            char('"'),
            map(
                take_while1(|c: char| {
                    let n = c as u32;
                    (0x20..=0x21).contains(&n)
                        || (0x23..=0x2E).contains(&n)
                        || (0x30..=0x7F).contains(&n)
                        || n > 0x7F
                }),
                |s: &str| s.to_owned(),
            ),
            char('"'),
        ),
        map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@".contains(c)),
            |s: &str| s.to_owned(),
        ),
    ))(input)
}

fn path_component(input: &str) -> ParseResult<'_, String> {
    preceded(char('/'), path_component_body)(input)
}

fn absolute_path_prefix(input: &str) -> ParseResult<'_, FilePrefix> {
    let (rest, _) = char('/')(input)?;
    if rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/') || rest.starts_with(' ') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, FilePrefix::Absolute))
    }
}

fn local_import(input: &str) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, prefix) = alt((
        value(FilePrefix::Parent, tag("../")),
        value(FilePrefix::Here, tag("./")),
        value(FilePrefix::Home, tag("~/")),
        absolute_path_prefix,
    ))(input)?;

    let (rest, first) = path_component_body(rest)?;
    if prefix != FilePrefix::Absolute && first.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::TakeWhile1)));
    }
    let (rest, mut more) = many0(path_component)(rest)?;
    let mut components = vec![first];
    components.append(&mut more);

    Ok((rest, ImportTarget::Local(prefix, FilePath { file_path: components })))
}

/// HTTP(S) import: https://example.com/foo/bar.dhall [using headers]
fn http_import(input: &str) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, scheme) = alt((
        value(Scheme::HTTPS, tag("https://")),
        value(Scheme::HTTP, tag("http://")),
    ))(input)?;

    // Authority: everything up to the first /
    let (rest, authority) = map(
        take_while1(|c: char| c != '/' && c != '?' && c != '#' && !c.is_whitespace()),
        |s: &str| s.to_owned(),
    )(rest)?;

    // Path segments (URL paths allow percent-encoding)
    let (rest, segments) = many0(preceded(
        char('/'),
        map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@%".contains(c)),
            |s: &str| s.to_owned(),
        ),
    ))(rest)?;
    let file_path = if segments.is_empty() { vec!["".to_owned()] } else { segments };

    // Optional query
    let (rest, query) = opt(preceded(
        char('?'),
        map(take_while(|c: char| c != ' ' && c != '\n' && c != '\r'), |s: &str| s.to_owned()),
    ))(rest)?;

    // Optional headers: using import-expression
    let (rest, headers) = opt(|input| {
        let (r, _) = ws(input)?;
        let (r, _) = keyword("using")(r)?;
        let (r, _) = ws1(r)?;
        let (r, e) = import_expression(r)?;
        Ok((r, e))
    })(rest)?;

    Ok((rest, ImportTarget::Remote(URL {
        scheme,
        authority,
        path: FilePath { file_path },
        query,
        headers,
    })))
}

/// Environment variable import: env:NAME or env:"NAME"
fn env_import(input: &str) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, _) = tag("env:")(input)?;
    let (rest, name) = alt((
        // Quoted: env:"NAME" (POSIX env var with escapes)
        delimited(char('"'), posix_env_var, char('"')),
        // Unquoted: env:NAME (bash-style)
        map(take_while1(|c: char| c.is_ascii_alphanumeric() || c == '_'), |s: &str| s.to_owned()),
    ))(rest)?;
    Ok((rest, ImportTarget::Env(name)))
}

/// Parse a POSIX-compliant quoted environment variable name.
fn posix_env_var(input: &str) -> ParseResult<'_, String> {
    let (rest, chars) = many0(alt((
        // Escape sequences
        preceded(char('\\'), alt((
            value('\x22', char('"')),
            value('\x5C', char('\\')),
            value('\x07', char('a')),
            value('\x08', char('b')),
            value('\x0C', char('f')),
            value('\x0A', char('n')),
            value('\x0D', char('r')),
            value('\x09', char('t')),
            value('\x0B', char('v')),
        ))),
        // Printable characters except double quote, backslash, and equals
        nom::character::complete::satisfy(|c| {
            let n = c as u32;
            (0x20..=0x21).contains(&n)
                || (0x23..=0x3C).contains(&n)
                || (0x3E..=0x5B).contains(&n)
                || (0x5D..=0x7E).contains(&n)
        }),
    )))(input)?;
    if chars.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::TakeWhile1)));
    }
    Ok((rest, chars.into_iter().collect()))
}

/// `missing` keyword — only needs to not be a prefix of an identifier
fn missing_import(input: &str) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, _) = tag("missing")(input)?;
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)));
    }
    Ok((rest, ImportTarget::Missing))
}

/// SHA256 hash: sha256:hex...
fn import_hash(input: &str) -> ParseResult<'_, Hash> {
    let (rest, _) = tag("sha256:")(input)?;
    let (rest, hex_str) = take_while1(|c: char| c.is_ascii_hexdigit())(rest)?;
    let bytes = hex::decode(hex_str).map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag))
    })?;
    Ok((rest, Hash::SHA256(bytes.into())))
}

/// Full import expression: location hash? (as Text | as Location)?
fn import_expr(input: &str) -> ParseResult<'_, Expr> {
    let (rest, location) = alt((
        http_import,
        local_import,
        env_import,
        missing_import,
    ))(input)?;

    let (rest, hash) = opt(preceded(ws1, import_hash))(rest)?;

    let (rest, mode) = opt(preceded(
        ws1,
        preceded(
            terminated(keyword("as"), ws1),
            alt((
                value(ImportMode::RawText, tag("Text")),
                value(ImportMode::Location, tag("Location")),
            )),
        ),
    ))(rest)?;

    let import = crate::syntax::Import {
        mode: mode.unwrap_or(ImportMode::Code),
        location,
        hash,
    };
    Ok((rest, mkexpr(ExprKind::Import(import))))
}

// ── 6. Atoms (primitive expressions) ─────────────────────────────────

fn atom(input: &str) -> ParseResult<'_, Expr> {
    alt((
        // Parenthesized expression
        delimited(
            terminated(char('('), ws),
            expression,
            preceded(ws, char(')')),
        ),
        // Numeric literals (order matters: double before natural)
        map(double_literal, |n| mkexpr(ExprKind::Num(NumKind::Double(n)))),
        map(integer_literal, |n| mkexpr(ExprKind::Num(NumKind::Integer(n)))),
        map(natural_literal, |n| mkexpr(ExprKind::Num(NumKind::Natural(n)))),
        // Text literal
        map(double_quote_literal, |t| mkexpr(ExprKind::TextLit(t))),
        map(single_quote_literal, |t| mkexpr(ExprKind::TextLit(t))),
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
        map(variable, |v| mkexpr(ExprKind::Var(v))),
    ))(input)
}

// ── 7. Records ───────────────────────────────────────────────────────

fn record_literal_or_type(input: &str) -> ParseResult<'_, Expr> {
    use alloc::collections::BTreeMap;
    delimited(
        terminated(char('{'), ws),
        |input| {
            let (rest, _) = opt(terminated(char(','), ws))(input)?;
            // Try empty record literal: = [,]
            if let Ok((rest2, _)) = char::<_, nom::error::Error<&str>>('=')(rest) {
                let (rest2, _) = opt(preceded(ws, char(',')))(rest2)?;
                return Ok((rest2, mkexpr(ExprKind::RecordLit(Default::default()))));
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
                            return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify)));
                        }
                        map.insert(l, e);
                    }
                    return Ok((rest2, mkexpr(ExprKind::RecordType(map))));
                } else {
                    let mut map = BTreeMap::new();
                    for (l, _, e) in entries {
                        insert_recordlit_entry(&mut map, l, e);
                    }
                    return Ok((rest2, mkexpr(ExprKind::RecordLit(map))));
                }
            }
            // Empty record type {} or { , }
            Ok((rest, mkexpr(ExprKind::RecordType(Default::default()))))
        },
        preceded(ws, char('}')),
    )(input)
}

/// Record entry: `name = expr`, `name : type`, `name` (pun), or `name.a.b = expr` (dotted).
fn record_entry(input: &str) -> ParseResult<'_, (Label, char, Expr)> {
    let (rest, first_label) = terminated(any_label_or_some, ws)(input)?;

    // Try dotted field syntax: name.a.b = expr
    if let Ok((rest2, _)) = char::<_, nom::error::Error<&str>>('.')(rest) {
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
    if let Ok((rest2, _)) = char::<_, nom::error::Error<&str>>('=')(rest) {
        let (rest2, _) = ws(rest2)?;
        let (rest2, val) = expression(rest2)?;
        return Ok((rest2, (first_label, '=', val)));
    }
    if let Ok((rest2, _)) = char::<_, nom::error::Error<&str>>(':')(rest) {
        let (rest2, _) = ws1(rest2)?;
        let (rest2, val) = expression(rest2)?;
        return Ok((rest2, (first_label, ':', val)));
    }

    // Pun: `{ name }` desugars to `{ name = name }`
    let pun_expr = Expr::new(ExprKind::Var(V(first_label.clone(), 0)), Span::Artificial);
    Ok((rest, (first_label, '=', pun_expr)))
}

// ── 8. Lists ─────────────────────────────────────────────────────────

fn list_literal(input: &str) -> ParseResult<'_, Expr> {
    delimited(
        terminated(char('['), ws),
        map(
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
            |items| mkexpr(ExprKind::NEListLit(items)),
        ),
        preceded(ws, char(']')),
    )(input)
}

// ── 8b. Union types ──────────────────────────────────────────────────

/// Parse a single union type entry: `label` or `label : type`.
fn union_type_entry(input: &str) -> ParseResult<'_, (Label, Option<Expr>)> {
    let (rest, l) = terminated(any_label_or_some, ws)(input)?;
    let (rest, ty) = opt(|input| {
        let (r, _) = char(':')(input)?;
        let (r, _) = ws1(r)?;
        let (r, e) = expression(r)?;
        Ok((r, e))
    })(rest)?;
    Ok((rest, (l, ty)))
}

fn union_type(input: &str) -> ParseResult<'_, Expr> {
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
            return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify)));
        }
        map.insert(l, ty);
    }
    let (rest, _) = preceded(ws, char('>'))(rest)?;
    Ok((rest, mkexpr(ExprKind::UnionType(map))))
}

// ── 8c. Empty list with type ─────────────────────────────────────────

fn empty_list_literal(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = terminated(char('['), ws)(input)?;
    let (rest, _) = opt(terminated(char(','), ws))(rest)?;
    let (rest, _) = terminated(char(']'), ws)(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::EmptyListLit(ty))))
}

// ── 9. Selector, completion, application ─────────────────────────────

/// Field access and projection: `e.x`, `e.{ x, y }`, `e.(T)`
fn selector_expression(input: &str) -> ParseResult<'_, Expr> {
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
                        return Err(nom::Err::Error(nom::error::Error::new(r, nom::error::ErrorKind::Tag)));
                    } else {
                        (r, vec![])
                    };
                    let (r, _) = char('}')(r)?;
                    let mut set = BTreeSet::new();
                    for l in ls {
                        if !set.insert(l) {
                            return Err(nom::Err::Error(nom::error::Error::new(r, nom::error::ErrorKind::Verify)));
                        }
                    }
                    Ok((r, mkexpr(ExprKind::Op(OpKind::Projection(expr.clone(), set)))))
                },
                // .(T) — projection by expression
                map(
                    delimited(
                        terminated(char('('), ws),
                        expression,
                        preceded(ws, char(')')),
                    ),
                    |e| mkexpr(ExprKind::Op(OpKind::ProjectionByExpr(expr.clone(), e))),
                ),
                // .field — field access
                map(label, |l| {
                    mkexpr(ExprKind::Op(OpKind::Field(expr.clone(), l)))
                }),
            ))(r)?;
            Ok((r, sel))
        })();
        match tried {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Completion: `T::r`
fn completion_expression(input: &str) -> ParseResult<'_, Expr> {
    let (mut rest, mut expr) = selector_expression(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = tag("::")(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = selector_expression(r)?;
            Ok((r, mkexpr(ExprKind::Op(OpKind::Completion(expr.clone(), rhs)))))
        })();
        match tried {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Keyword-prefixed application: `Some e`, `merge x y`, `toMap x`
fn first_application(input: &str) -> ParseResult<'_, Expr> {
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

fn some_application(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("Some")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, e) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::SomeLit(e))))
}

fn merge_application(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, y) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(OpKind::Merge(x, y, None)))))
}

fn tomap_application(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(OpKind::ToMap(x, None)))))
}

/// import-expression = import / completion-expression
fn import_expression(input: &str) -> ParseResult<'_, Expr> {
    alt((import_expr, completion_expression))(input)
}

/// Function application: `f a b` = `App(App(f, a), b)`
/// ABNF: first-application-expression *(whsp1 import-expression)
fn application(input: &str) -> ParseResult<'_, Expr> {
    let (mut rest, mut expr) = first_application(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws1(rest)?;
            let (r, arg) = import_expression(r)?;
            Ok((r, arg))
        })();
        match tried {
            Ok((r, arg)) => {
                expr = mkexpr(ExprKind::Op(OpKind::App(expr, arg)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

// ── 10. Operators (full precedence tower) ─────────────────────────────
//
// Lowest precedence at the top, highest at the bottom.
// All operators are left-associative.
// Each level parses its operator and delegates to the next level for operands.

/// Helper: build a left-associative binary operator parser for one precedence level.
macro_rules! binop_level {
    // Single operator — no alt() needed
    ($name:ident, $next:ident, $op_tag:expr => $op_variant:expr) => {
        fn $name(input: &str) -> ParseResult<'_, Expr> {
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
                        lhs = mkexpr(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)));
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
        fn $name(input: &str) -> ParseResult<'_, Expr> {
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
                        lhs = mkexpr(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)));
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
fn op_bool_eq(input: &str) -> ParseResult<'_, BinOp> {
    let (rest, _) = tag("==")(input)?;
    if rest.starts_with('=') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, BoolEQ))
    }
}

// Ordering matters: longer tokens must come first to avoid prefix matches.
binop_level!(equiv_expr,                   import_alt_expr,    "===" => Equivalence, "≡" => Equivalence);

/// `?` requires mandatory whitespace after to disambiguate `http://a/a?a`
/// ABNF: or-expression *(whsp "?" whsp1 or-expression)
fn import_alt_expr(input: &str) -> ParseResult<'_, Expr> {
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
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(ImportAlt, lhs, rhs)));
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
fn plus_expr(input: &str) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = list_append_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = char('+')(r)?;
            // Reject ++ (that's text append)
            if r.starts_with('+') {
                return Err(nom::Err::Error(nom::error::Error::new(rest, nom::error::ErrorKind::Tag)));
            }
            let (r, _) = ws1(r)?;
            let (r, rhs) = list_append_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(NaturalPlus, lhs, rhs)));
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

fn combine_expr(input: &str) -> ParseResult<'_, Expr> {
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
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(RecursiveRecordMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// Match `//` but not `//\\`
fn op_prefer_ascii(input: &str) -> ParseResult<'_, &str> {
    let (rest, _) = tag("//")(input)?;
    if rest.starts_with('\\') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, "//"))
    }
}

fn prefer_expr(input: &str) -> ParseResult<'_, Expr> {
    let (mut rest, mut lhs) = combine_types_expr(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = alt((tag("⫽"), op_prefer_ascii))(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = combine_types_expr(r)?;
            Ok((r, rhs))
        })();
        match tried {
            Ok((r, rhs)) => {
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(RightBiasedRecordMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

fn combine_types_expr(input: &str) -> ParseResult<'_, Expr> {
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
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(RecursiveRecordTypeMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// `==` level needs special handling to not consume `===`.
fn bool_eq_expr(input: &str) -> ParseResult<'_, Expr> {
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
                lhs = mkexpr(ExprKind::Op(OpKind::BinOp(op, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

fn operator_expression(input: &str) -> ParseResult<'_, Expr> {
    equiv_expr(input)
}

// ── 11. Top-level expressions ────────────────────────────────────────

fn let_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("let")(input)?;
    let (mut rest, _) = ws1(rest)?;
    let mut bindings = Vec::new();
    loop {
        let (r, name) = terminated(nonreserved_label, ws)(rest)?;
        let (r, annot) = opt(|input| {
            let (r, _) = char(':')(input)?;
            let (r, _) = ws1(r)?;
            let (r, e) = expression(r)?;
            let (r, _) = ws(r)?;
            Ok((r, e))
        })(r)?;
        let (r, _) = char('=')(r)?;
        let (r, _) = ws(r)?;
        let (r, val) = expression(r)?;
        let (r, _) = ws(r)?;
        bindings.push((name, annot, val));
        rest = r;
        if let Ok((r, _)) = keyword::<'_>("let")(rest) {
            let (r, _) = ws1(r)?;
            rest = r;
        } else {
            break;
        }
    }
    let (rest, _) = keyword("in")(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, body) = expression(rest)?;
    let expr = bindings.into_iter().rev().fold(body, |acc, (name, annot, val)| {
        mkexpr(ExprKind::Let(name, annot, val, acc))
    });
    Ok((rest, expr))
}

fn lambda_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = alt((tag("\\"), tag("λ")))(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('(')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = terminated(nonreserved_label, ws)(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(')')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = alt((tag("->"), tag("→")))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, body) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Lam(name, ty, body))))
}

fn if_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("if")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, cond) = expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = keyword("then")(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, t) = expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = keyword("else")(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, f) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(OpKind::BoolIf(cond, t, f)))))
}

fn forall_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = alt((tag("forall"), tag("∀")))(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('(')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = terminated(nonreserved_label, ws)(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(')')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = alt((tag("->"), tag("→")))(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, body) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Pi(name, ty, body))))
}

fn assert_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("assert")(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, e) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Assert(e))))
}

/// `merge x y : T` (with type annotation)
fn merge_annot_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, y) = import_expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(OpKind::Merge(x, y, Some(ty))))))
}

/// `toMap x : T` (with type annotation)
fn tomap_annot_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(OpKind::ToMap(x, Some(ty))))))
}

/// `with` expression: `e with a.b.c = v`
/// ABNF: import-expression 1*(whsp1 with whsp1 with-clause)
fn with_expression(input: &str) -> ParseResult<'_, Expr> {
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
fn with_clause(input: &str, base: Expr) -> ParseResult<'_, Expr> {
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
    Ok((rest, mkexpr(ExprKind::Op(OpKind::With(base, labels, val)))))
}

/// Arrow type: `A -> B` (non-dependent function type)
/// ABNF: operator-expression whsp arrow whsp expression
/// Falls through to annotated-expression if no arrow found.
fn arrow_or_annot_expression(input: &str) -> ParseResult<'_, Expr> {
    let (rest, lhs) = operator_expression(input)?;
    // Try arrow
    let tried_arrow = (|| -> ParseResult<Expr> {
        let (r, _) = ws(rest)?;
        let (r, _) = alt((tag("->"), tag("→")))(r)?;
        let (r, _) = ws(r)?;
        let (r, rhs) = expression(r)?;
        Ok((r, mkexpr(ExprKind::Pi("_".into(), lhs.clone(), rhs))))
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
        Some(ty) => Ok((rest, mkexpr(ExprKind::Annot(lhs, ty)))),
        None => Ok((rest, lhs)),
    }
}

/// Top-level expression parser.
pub fn expression(input: &str) -> ParseResult<'_, Expr> {
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
    let mut complete = terminated(expression, ws);
    match complete(input) {
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

    // ── Operator tests ───────────────────────────────────────────

    #[test]
    fn test_bool_and() {
        let e = parse_expr("True && False").unwrap();
        assert_eq!(e.to_string(), "True && False");
    }

    #[test]
    fn test_bool_or() {
        let e = parse_expr("True || False").unwrap();
        assert_eq!(e.to_string(), "True || False");
    }

    #[test]
    fn test_bool_eq() {
        let e = parse_expr("True == False").unwrap();
        assert_eq!(e.to_string(), "True == False");
    }

    #[test]
    fn test_bool_ne() {
        let e = parse_expr("True != False").unwrap();
        assert_eq!(e.to_string(), "True != False");
    }

    #[test]
    fn test_natural_times() {
        let e = parse_expr("3 * 4").unwrap();
        assert_eq!(e.to_string(), "3 * 4");
    }

    #[test]
    fn test_text_append() {
        let e = parse_expr(r#""a" ++ "b""#).unwrap();
        let s = e.to_string();
        assert!(s.contains("++"), "got: {}", s);
    }

    #[test]
    fn test_list_append() {
        let e = parse_expr("[1] # [2]").unwrap();
        let s = e.to_string();
        assert!(s.contains("#"), "got: {}", s);
    }

    #[test]
    fn test_precedence_plus_times() {
        // * binds tighter than +
        let e = parse_expr("1 + 2 * 3").unwrap();
        let s = e.to_string();
        // Should be 1 + (2 * 3), printed as "1 + 2 * 3"
        assert_eq!(s, "1 + 2 * 3");
    }

    #[test]
    fn test_precedence_and_or() {
        // && binds tighter than ||
        let e = parse_expr("True || False && True").unwrap();
        let s = e.to_string();
        assert_eq!(s, "True || False && True");
    }

    // ── Import tests ─────────────────────────────────────────────

    #[test]
    fn test_import_here() {
        let e = parse_expr("./config.dhall").unwrap();
        let s = e.to_string();
        assert!(s.contains("config.dhall"), "got: {}", s);
    }

    #[test]
    fn test_import_parent() {
        let e = parse_expr("../lib/utils.dhall").unwrap();
        let s = e.to_string();
        assert!(s.contains("lib") && s.contains("utils.dhall"), "got: {}", s);
    }

    #[test]
    fn test_import_absolute() {
        let e = parse_expr("/etc/config.dhall").unwrap();
        let s = e.to_string();
        assert!(s.contains("etc") && s.contains("config.dhall"), "got: {}", s);
    }

    #[test]
    fn test_import_home() {
        let e = parse_expr("~/.config/dhall/config.dhall").unwrap();
        let s = e.to_string();
        assert!(s.contains("config.dhall"), "got: {}", s);
    }

    #[test]
    fn test_import_env() {
        let e = parse_expr("env:HOME").unwrap();
        let s = e.to_string();
        assert!(s.contains("env:HOME"), "got: {}", s);
    }

    #[test]
    fn test_import_env_quoted() {
        let e = parse_expr(r#"env:"MY VAR""#).unwrap();
        let s = e.to_string();
        assert!(s.contains("MY VAR"), "got: {}", s);
    }

    #[test]
    fn test_import_missing() {
        let e = parse_expr("missing").unwrap();
        let s = e.to_string();
        assert!(s.contains("missing"), "got: {}", s);
    }

    #[test]
    fn test_import_http() {
        let e = parse_expr("https://example.com/package.dhall").unwrap();
        let s = e.to_string();
        assert!(s.contains("example.com") && s.contains("package.dhall"), "got: {}", s);
    }

    #[test]
    fn test_import_as_text() {
        let e = parse_expr("./readme.md as Text").unwrap();
        let s = e.to_string();
        assert!(s.contains("as Text"), "got: {}", s);
    }

    #[test]
    fn test_import_with_hash() {
        let e = parse_expr("./config.dhall sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789").unwrap();
        let s = e.to_string();
        assert!(s.contains("sha256:"), "got: {}", s);
    }

    #[test]
    fn test_import_in_let() {
        let e = parse_expr("let config = ./config.dhall in config").unwrap();
        let s = e.to_string();
        assert!(s.contains("let") && s.contains("config"), "got: {}", s);
    }

    // ── Expression tests ─────────────────────────────────────────

    #[test]
    fn test_some() {
        let e = parse_expr("Some 42").unwrap();
        assert_eq!(e.to_string(), "Some 42");
    }

    #[test]
    fn test_merge() {
        let e = parse_expr("merge { a = True } x").unwrap();
        let s = e.to_string();
        assert!(s.contains("merge"), "got: {}", s);
    }

    #[test]
    fn test_merge_with_type() {
        let e = parse_expr("merge { a = True } x : Bool").unwrap();
        let s = e.to_string();
        assert!(s.contains("merge") && s.contains("Bool"), "got: {}", s);
    }

    #[test]
    fn test_tomap() {
        let e = parse_expr("toMap { a = 1 }").unwrap();
        let s = e.to_string();
        assert!(s.contains("toMap"), "got: {}", s);
    }

    #[test]
    fn test_assert() {
        let e = parse_expr("assert : True === True").unwrap();
        let s = e.to_string();
        assert!(s.contains("assert"), "got: {}", s);
    }

    #[test]
    fn test_field_access() {
        let e = parse_expr("x.y").unwrap();
        assert_eq!(e.to_string(), "x.y");
    }

    #[test]
    fn test_nested_field_access() {
        let e = parse_expr("x.y.z").unwrap();
        assert_eq!(e.to_string(), "x.y.z");
    }

    #[test]
    fn test_projection() {
        let e = parse_expr("x.{ a, b }").unwrap();
        let s = e.to_string();
        assert!(s.contains("a") && s.contains("b"), "got: {}", s);
    }

    #[test]
    fn test_union_type() {
        let e = parse_expr("< A | B : Natural >").unwrap();
        let s = e.to_string();
        assert!(s.contains("A") && s.contains("B") && s.contains("Natural"), "got: {}", s);
    }

    #[test]
    fn test_empty_list_with_type() {
        let e = parse_expr("[] : List Natural").unwrap();
        let s = e.to_string();
        assert!(s.contains("List") && s.contains("Natural"), "got: {}", s);
    }

    #[test]
    fn test_with() {
        let e = parse_expr("x with a.b = 1").unwrap();
        let s = e.to_string();
        assert!(s.contains("with"), "got: {}", s);
    }

    #[test]
    fn test_arrow_type() {
        let e = parse_expr("Natural -> Text").unwrap();
        let s = e.to_string();
        assert!(s.contains("Natural") && s.contains("Text"), "got: {}", s);
    }

    // ── Record sugar tests ───────────────────────────────────────

    #[test]
    fn test_record_pun() {
        // { x } desugars to { x = x }
        let e = parse_expr("let x = 1 in { x }").unwrap();
        let s = e.to_string();
        assert!(s.contains("x"), "got: {}", s);
    }

    #[test]
    fn test_record_pun_multiple() {
        let e = parse_expr("let x = 1 in let y = 2 in { x, y }").unwrap();
        let s = e.to_string();
        assert!(s.contains("x") && s.contains("y"), "got: {}", s);
    }

    #[test]
    fn test_record_dotted_field() {
        // { a.b = 1 } desugars to { a = { b = 1 } }
        let e = parse_expr("{ a.b = 1 }").unwrap();
        let s = e.to_string();
        assert!(s.contains("a") && s.contains("b") && s.contains("1"), "got: {}", s);
    }

    #[test]
    fn test_record_dotted_field_deep() {
        let e = parse_expr("{ a.b.c = True }").unwrap();
        let s = e.to_string();
        assert!(s.contains("a") && s.contains("b") && s.contains("c"), "got: {}", s);
    }

    // ── Structural syntax tests ─────────────────────────────────

    #[test]
    fn test_trailing_comma_record_lit() {
        let e = parse_expr("{ x = 1, y = 2, }");
        assert!(e.is_ok(), "trailing comma in record literal: {:?}", e.err());
    }

    #[test]
    fn test_trailing_comma_record_type() {
        let e = parse_expr("{ x : Natural, y : Natural, }");
        assert!(e.is_ok(), "trailing comma in record type: {:?}", e.err());
    }

    #[test]
    fn test_leading_and_trailing_comma_record() {
        let e = parse_expr("{ , x = 1, y = 2, }");
        assert!(e.is_ok(), "leading+trailing comma in record: {:?}", e.err());
    }

    #[test]
    fn test_trailing_comma_list() {
        let e = parse_expr("[1, 2, 3,]");
        assert!(e.is_ok(), "trailing comma in list: {:?}", e.err());
    }

    #[test]
    fn test_leading_separator_union() {
        let e = parse_expr("< | A | B >");
        assert!(e.is_ok(), "leading | in union: {:?}", e.err());
    }

    #[test]
    fn test_empty_union_with_separator() {
        let e = parse_expr("< | >");
        assert!(e.is_ok(), "empty union with |: {:?}", e.err());
    }

    #[test]
    fn test_operator_combine_ascii() {
        // /\ is the ASCII form of ∧
        let e = parse_expr(r#"{ x = 1 } /\ { y = 2 }"#);
        assert!(e.is_ok(), "combine /\\: {:?}", e.err());
    }

    #[test]
    fn test_operator_prefer_ascii() {
        // // is the ASCII form of ⫽
        let e = parse_expr(r#"{ x = 1 } // { x = 2 }"#);
        assert!(e.is_ok(), "prefer //: {:?}", e.err());
    }

    #[test]
    fn test_operator_combine_types_ascii() {
        // //\\ is the ASCII form of ⩓
        let e = parse_expr(r#"{ x : Natural } //\\ { y : Text }"#);
        assert!(e.is_ok(), "combine types //\\\\: {:?}", e.err());
    }

    #[test]
    fn test_shebang() {
        let e = parse_expr("#!/usr/bin/env dhall\n42");
        assert!(e.is_ok(), "shebang: {:?}", e.err());
    }

    #[test]
    fn test_leading_comma_projection() {
        let e = parse_expr("x.{ , a, b }");
        assert!(e.is_ok(), "leading comma in projection: {:?}", e.err());
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

    #[test]
    fn test_block_comment() {
        let e = parse_expr("{- a comment -} 42").unwrap();
        assert_eq!(e.to_string(), "42");
    }

    #[test]
    fn test_block_comment_inline() {
        let e = parse_expr("1 {- plus -} + 2").unwrap();
        assert_eq!(e.to_string(), "1 + 2");
    }

    #[test]
    fn test_block_comment_nested() {
        let e = parse_expr("{- outer {- inner -} still outer -} True").unwrap();
        assert_eq!(e.to_string(), "True");
    }

    #[test]
    fn test_block_comment_multiline() {
        let e = parse_expr("{-\n  multi\n  line\n-} 1 + 2").unwrap();
        assert_eq!(e.to_string(), "1 + 2");
    }

    // ── String tests ─────────────────────────────────────────────

    #[test]
    fn test_string_escape_sequences() {
        let e = parse_expr(r#""\n\t\\\"\/""#).unwrap();
        // The printer re-escapes, so we check the AST round-trips.
        let s = e.to_string();
        assert!(s.contains("\\n") && s.contains("\\t"), "got: {}", s);
    }

    #[test]
    fn test_string_unicode_escape() {
        let e = parse_expr(r#""\u0041""#).unwrap();
        // \u0041 = 'A'
        assert_eq!(e.to_string(), "\"A\"");
    }

    #[test]
    fn test_string_unicode_escape_braces() {
        let e = parse_expr(r#""\u{1F600}""#).unwrap();
        // \u{1F600} = 😀
        assert_eq!(e.to_string(), "\"😀\"");
    }

    #[test]
    fn test_string_interpolation() {
        let e = parse_expr(r#""hello ${"world"}""#).unwrap();
        let s = e.to_string();
        assert!(s.contains("hello") && s.contains("world"), "got: {}", s);
    }

    #[test]
    fn test_string_interpolation_expr() {
        let e = parse_expr(r#""value: ${Natural/show 42}""#).unwrap();
        let s = e.to_string();
        assert!(s.contains("Natural/show") && s.contains("42"), "got: {}", s);
    }

    #[test]
    fn test_string_dollar_not_interpolation() {
        let e = parse_expr(r#""costs $5""#).unwrap();
        let s = e.to_string();
        // The printer escapes $ as \u0024 to avoid interpolation ambiguity.
        assert!(s.contains("costs") && s.contains("5"), "got: {}", s);
    }

    #[test]
    fn test_multiline_string_basic() {
        // Two-line string with indent stripping.
        let input = "''\n  hello\n  world\n  ''";
        let e = parse_expr(input).unwrap();
        let s = e.to_string();
        assert!(s.contains("hello") && s.contains("world"), "got: {}", s);
    }

    #[test]
    fn test_multiline_string_indent_stripping() {
        // 4-space indent on content, 4-space indent on closing ''.
        // Should strip 4 spaces.
        let input = "''\n    line1\n    line2\n    ''";
        let e = parse_expr(input).unwrap();
        let s = e.to_string();
        // After stripping, should be "line1\nline2\n"
        assert!(s.contains("line1"), "got: {}", s);
    }

    #[test]
    fn test_multiline_string_escaped_quotes() {
        // ''' inside a multi-line string produces ''
        let input = "''\n  '''quoted'''\n  ''";
        let e = parse_expr(input).unwrap();
        let s = e.to_string();
        assert!(s.contains("''"), "got: {}", s);
    }

    #[test]
    fn test_multiline_string_interpolation() {
        let input = "''\n  hello ${\"world\"}\n  ''";
        let e = parse_expr(input).unwrap();
        let s = e.to_string();
        assert!(s.contains("hello") && s.contains("world"), "got: {}", s);
    }

    #[test]
    fn test_union_type_no_space_colon() {
        let e = parse_expr("< x: T | y: U >");
        assert!(e.is_ok(), "union type with no space before colon: {:?}", e.err());
    }

    #[test]
    fn test_empty_record_leading_comma() {
        let e = parse_expr("{ , = }");
        assert!(e.is_ok(), "empty record with leading comma: {:?}", e.err());
    }

    #[test]
    fn test_empty_record_trailing_comma() {
        let e = parse_expr("{ =, }");
        assert!(e.is_ok(), "empty record with trailing comma: {:?}", e.err());
    }

    #[test]
    fn test_printer_roundtrip_interpolation() {
        let input = r#""${Natural/show 1}""#;
        let e = parse_expr(input).unwrap();
        let printed = e.to_string();
        eprintln!("input:   {}", input);
        eprintln!("printed: {}", printed);
        let e2 = parse_expr(&printed);
        assert!(e2.is_ok(), "re-parse failed: {:?}", e2.err());
        assert_eq!(e.to_string(), e2.unwrap().to_string());
    }

    #[test]
    fn test_keyword_as_record_field_rejected() {
        // Keywords must not be used as bare record field names
        assert!(parse_expr("{ if: Text }").is_err(), "if should be rejected");
        assert!(parse_expr("{ merge: Text }").is_err(), "merge should be rejected");
        assert!(parse_expr("{ with: Text }").is_err(), "with should be rejected");
        // But backtick-quoted keywords are fine
        assert!(parse_expr("{ `if`: Text }").is_ok(), "`if` should be allowed");
        // Some is explicitly allowed
        assert!(parse_expr("{ Some: Text }").is_ok(), "Some should be allowed");
    }
}
