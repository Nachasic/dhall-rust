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

/// Error type compatible with the pest parser's public API.
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
fn insert_recordlit_entry(map: &mut std::collections::BTreeMap<Label, Expr>, l: Label, e: Expr) {
    use std::collections::btree_map::Entry;
    match map.entry(l) {
        Entry::Vacant(entry) => { entry.insert(e); }
        Entry::Occupied(mut entry) => {
            let other = entry.insert(Expr::new(ExprKind::Num(crate::syntax::NumKind::Bool(false)), Span::Artificial));
            entry.insert(Expr::new(
                ExprKind::Op(crate::operations::OpKind::BinOp(
                    crate::operations::BinOp::RecursiveRecordMerge, other, e,
                )),
                Span::Artificial,
            ));
        }
    }
}

// ── 1. Whitespace and comments ───────────────────────────────────────

/// Skip whitespace and line comments (-- to end of line)
/// and block comments ({- ... -}, which can nest).
fn ws(input: &str) -> ParseResult<()> {
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
fn ws1(input: &str) -> ParseResult<()> {
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

fn natural_literal(input: &str) -> ParseResult<u64> {
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

fn decimal_natural(input: &str) -> ParseResult<u64> {
    let (rest, s) = digit1(input)?;
    if s.len() > 1 && s.starts_with('0') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        s.parse::<u64>()
            .map(|n| (rest, n))
            .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    }
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
        // With dot: 1.0, 1.0e5
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
        // Without dot: 1e4, -1E+5
        map_res(
            recognize(tuple((
                opt(one_of("+-")),
                digit1,
                one_of("eE"),
                opt(one_of("+-")),
                digit1,
            ))),
            |s: &str| s.parse::<f64>().map(NaiveDouble::from),
        ),
    ))(input)
}

/// Double-quoted string escape sequence.
fn double_quote_escaped(input: &str) -> ParseResult<String> {
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
                    .and_then(|n| char::from_u32(n).ok_or_else(|| "invalid codepoint".to_owned()))
                    .map(|c| c.to_string()),
            ),
        ))),
    )))(input)
}

/// A chunk of a double-quoted string: text, escape, or interpolation.
fn double_quote_chunk(input: &str) -> ParseResult<InterpolatedTextContents<Expr>> {
    alt((
        // Interpolation: ${expr}
        map(
            delimited(tag("${"), expression, char('}')),
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
fn double_quote_literal(input: &str) -> ParseResult<InterpolatedText<Expr>> {
    delimited(
        char('"'),
        map(many0(double_quote_chunk), |chunks| chunks.into_iter().collect()),
        char('"'),
    )(input)
}

/// A chunk of a single-quoted (multi-line) string.
fn single_quote_chunk(input: &str) -> ParseResult<InterpolatedTextContents<Expr>> {
    alt((
        // Escaped sequences specific to multi-line strings
        value(InterpolatedTextContents::Text("''".to_owned()), tag("'''")),
        value(InterpolatedTextContents::Text("${".to_owned()), tag("''${")),
        // Interpolation
        map(
            delimited(tag("${"), expression, char('}')),
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
fn single_quote_literal(input: &str) -> ParseResult<InterpolatedText<Expr>> {
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
    "Infinity", "NaN", "merge", "toMap", "assert", "forall",
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
    alt((backtick_label, simple_label))(input)
}

fn variable(input: &str) -> ParseResult<V> {
    let (rest, l) = label(input)?;
    let (rest, idx) = opt(preceded(
        delimited(ws, char('@'), ws),
        natural_literal,
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

fn builtin_no_index(input: &str) -> ParseResult<Expr> {
    let (rest, b) = builtin(input)?;
    if rest.starts_with('@') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, mkexpr(b)))
    }
}

// ── 4b. Imports ──────────────────────────────────────────────────────

use crate::syntax::{FilePath, FilePrefix, Hash, ImportMode, ImportTarget, Scheme, URL};

/// Path component: /segment
fn path_component(input: &str) -> ParseResult<String> {
    preceded(
        char('/'),
        map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@".contains(c)),
            |s: &str| s.to_owned(),
        ),
    )(input)
}

/// Local path: ./foo/bar.dhall, ../foo, /abs/path, ~/home/path
/// Match `/` as absolute path prefix, but not `/\` or `//` (which are operators).
fn absolute_path_prefix(input: &str) -> ParseResult<FilePrefix> {
    let (rest, _) = char('/')(input)?;
    if rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/') || rest.starts_with(' ') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, FilePrefix::Absolute))
    }
}

fn local_import(input: &str) -> ParseResult<ImportTarget<Expr>> {
    let (rest, prefix) = alt((
        value(FilePrefix::Parent, tag("../")),
        value(FilePrefix::Here, tag("./")),
        value(FilePrefix::Home, tag("~/")),
        absolute_path_prefix,
    ))(input)?;

    // For absolute paths, the first / was consumed by the prefix.
    // We need to parse the first component without a leading /.
    let (rest, components) = if prefix == FilePrefix::Absolute {
        let (rest, first) = map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@".contains(c)),
            |s: &str| s.to_owned(),
        )(rest)?;
        let (rest, mut more) = many0(path_component)(rest)?;
        let mut all = vec![first];
        all.append(&mut more);
        (rest, all)
    } else {
        // First component already has no leading /
        let (rest, first) = map(
            take_while1(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@".contains(c)),
            |s: &str| s.to_owned(),
        )(rest)?;
        let (rest, mut more) = many0(path_component)(rest)?;
        let mut all = vec![first];
        all.append(&mut more);
        (rest, all)
    };

    Ok((rest, ImportTarget::Local(prefix, FilePath { file_path: components })))
}

/// HTTP(S) import: https://example.com/foo/bar.dhall
fn http_import(input: &str) -> ParseResult<ImportTarget<Expr>> {
    let (rest, scheme) = alt((
        value(Scheme::HTTPS, tag("https://")),
        value(Scheme::HTTP, tag("http://")),
    ))(input)?;

    // Authority: everything up to the first /
    let (rest, authority) = map(
        take_while1(|c: char| c != '/' && c != '?' && c != '#' && !c.is_whitespace()),
        |s: &str| s.to_owned(),
    )(rest)?;

    // Path segments
    let (rest, segments) = many0(path_component)(rest)?;
    let file_path = if segments.is_empty() { vec!["".to_owned()] } else { segments };

    // Optional query
    let (rest, query) = opt(preceded(
        char('?'),
        map(take_while(|c: char| c != ' ' && c != '\n' && c != '\r'), |s: &str| s.to_owned()),
    ))(rest)?;

    Ok((rest, ImportTarget::Remote(URL {
        scheme,
        authority,
        path: FilePath { file_path },
        query,
        headers: None,
    })))
}

/// Environment variable import: env:NAME or env:"NAME"
fn env_import(input: &str) -> ParseResult<ImportTarget<Expr>> {
    let (rest, _) = tag("env:")(input)?;
    let (rest, name) = alt((
        // Quoted: env:"NAME"
        delimited(char('"'), map(take_while1(|c: char| c != '"'), |s: &str| s.to_owned()), char('"')),
        // Unquoted: env:NAME
        map(take_while1(|c: char| c.is_ascii_alphanumeric() || c == '_'), |s: &str| s.to_owned()),
    ))(rest)?;
    Ok((rest, ImportTarget::Env(name)))
}

/// `missing` keyword — only needs to not be a prefix of an identifier
fn missing_import(input: &str) -> ParseResult<ImportTarget<Expr>> {
    let (rest, _) = tag("missing")(input)?;
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)));
    }
    Ok((rest, ImportTarget::Missing))
}

/// SHA256 hash: sha256:hex...
fn import_hash(input: &str) -> ParseResult<Hash> {
    let (rest, _) = tag("sha256:")(input)?;
    let (rest, hex_str) = take_while1(|c: char| c.is_ascii_hexdigit())(rest)?;
    let bytes = hex::decode(hex_str).map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag))
    })?;
    Ok((rest, Hash::SHA256(bytes.into())))
}

/// Full import expression: location hash? (as Text | as Location)?
fn import_expr(input: &str) -> ParseResult<Expr> {
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

// ── 5. Atoms (primitive expressions) ─────────────────────────────────

fn atom(input: &str) -> ParseResult<Expr> {
    alt((
        // Parenthesized expression
        delimited(
            terminated(char('('), ws),
            expression,
            preceded(ws, char(')')),
        ),
        // Numeric literals (order matters: double before natural)
        map(double_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Double(n)))),
        map(integer_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Integer(n)))),
        map(natural_literal, |n| mkexpr(ExprKind::Num(crate::syntax::NumKind::Natural(n)))),
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

// ── 6. Records ───────────────────────────────────────────────────────

fn record_literal_or_type(input: &str) -> ParseResult<Expr> {
    use std::collections::BTreeMap;
    delimited(
        terminated(char('{'), ws),
        alt((
            // Empty record literal: { = }, { =, }, { , = }, { , =, }
            // ABNF: "{" whsp [ "," whsp ] record-type-or-literal whsp "}"
            //        empty-record-literal = "=" [ whsp "," ]
            map(
                |input| {
                    let (rest, _) = opt(terminated(char(','), ws))(input)?;
                    let (rest, _) = char('=')(rest)?;
                    let (rest, _) = opt(preceded(ws, char(',')))(rest)?;
                    Ok((rest, ()))
                },
                |_| mkexpr(ExprKind::RecordLit(Default::default())),
            ),
            // Non-empty record (with optional leading/trailing commas)
            map(
                |input| {
                    let (rest, _) = opt(terminated(char(','), ws))(input)?; // optional leading comma
                    let (rest, entries) = separated_list0(delimited(ws, char(','), ws), record_entry)(rest)?;
                    let (rest, _) = ws(rest)?;
                    let (rest, _) = opt(terminated(char(','), ws))(rest)?; // optional trailing comma
                    Ok((rest, entries))
                },
                |entries: Vec<(Label, char, Expr)>| {
                    if entries.is_empty() {
                        return mkexpr(ExprKind::RecordType(Default::default()));
                    }
                    let is_type = entries.iter().all(|(_, sep, _)| *sep == ':');
                    if is_type {
                        let map: BTreeMap<_, _> = entries.into_iter().map(|(l, _, e)| (l, e)).collect();
                        mkexpr(ExprKind::RecordType(map))
                    } else {
                        let mut map = BTreeMap::new();
                        for (l, _, e) in entries {
                            insert_recordlit_entry(&mut map, l, e);
                        }
                        mkexpr(ExprKind::RecordLit(map))
                    }
                },
            ),
        )),
        preceded(ws, char('}')),
    )(input)
}

/// Record entry: `name = expr`, `name : type`, `name` (pun), or `name.a.b = expr` (dotted).
fn record_entry(input: &str) -> ParseResult<(Label, char, Expr)> {
    let (rest, first_label) = terminated(label, ws)(input)?;

    // Try dotted field syntax: name.a.b = expr
    if let Ok((rest2, _)) = char::<_, nom::error::Error<&str>>('.')(rest) {
        let (rest2, _) = ws(rest2)?;
        // Collect remaining dot-separated labels
        let (rest2, more_labels) = separated_list0(
            delimited(ws, char('.'), ws),
            label,
        )(rest2)?;
        let (rest2, _) = ws(rest2)?;
        let (rest2, _) = char('=')(rest2)?;
        let (rest2, _) = ws(rest2)?;
        let (rest2, val) = expression(rest2)?;
        // Desugar: { a.b.c = v } → { a = { b = { c = v } } }
        let nested = more_labels.into_iter().rev().fold(val, |inner, l| {
            let map = std::iter::once((l, inner)).collect();
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

// ── 7. Lists ─────────────────────────────────────────────────────────

fn list_literal(input: &str) -> ParseResult<Expr> {
    delimited(
        terminated(char('['), ws),
        map(
            |input| {
                let (rest, _) = opt(terminated(char(','), ws))(input)?;
                let (rest, items) = separated_list0(delimited(ws, char(','), ws), expression)(rest)?;
                let (rest, _) = ws(rest)?;
                let (rest, _) = opt(terminated(char(','), ws))(rest)?;
                Ok((rest, items))
            },
            |items| mkexpr(ExprKind::NEListLit(items)),
        ),
        preceded(ws, char(']')),
    )(input)
}

// ── 7b. Union types ──────────────────────────────────────────────────

fn union_type(input: &str) -> ParseResult<Expr> {
    use std::collections::BTreeMap;
    delimited(
        terminated(char('<'), ws),
        map(
            |input| {
                let (rest, _) = opt(terminated(char('|'), ws))(input)?; // optional leading |
                let (rest, entries) = separated_list0(
                    delimited(ws, char('|'), ws),
                    pair(
                        terminated(label, ws),
                        opt(|input| {
                            let (r, _) = char(':')(input)?;
                            let (r, _) = ws1(r)?;
                            let (r, e) = expression(r)?;
                            Ok((r, e))
                        }),
                    ),
                )(rest)?;
                let (rest, _) = ws(rest)?;
                let (rest, _) = opt(preceded(char('|'), ws))(rest)?; // optional trailing |
                Ok((rest, entries))
            },
            |entries: Vec<(Label, Option<Expr>)>| {
                let map: BTreeMap<_, _> = entries.into_iter().collect();
                mkexpr(ExprKind::UnionType(map))
            },
        ),
        preceded(ws, char('>')),
    )(input)
}

// ── 7c. Empty list with type ─────────────────────────────────────────

fn empty_list_literal(input: &str) -> ParseResult<Expr> {
    let (rest, _) = terminated(char('['), ws)(input)?;
    let (rest, _) = opt(terminated(char(','), ws))(rest)?;
    let (rest, _) = terminated(char(']'), ws)(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::EmptyListLit(ty))))
}

// ── 8. Selector, completion, application ─────────────────────────────

/// Field access and projection: `e.x`, `e.{ x, y }`, `e.(T)`
fn selector_expression(input: &str) -> ParseResult<Expr> {
    use std::collections::BTreeSet;
    let (mut rest, mut expr) = atom(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = char('.')(r)?;
            let (r, _) = ws(r)?;
            let (r, sel) = alt((
                // .{ x, y } — projection (with optional leading comma)
                map(
                    delimited(
                        terminated(char('{'), ws),
                        |input| {
                            let (rest, _) = opt(terminated(char(','), ws))(input)?;
                            let (rest, ls) = separated_list0(
                                delimited(ws, char(','), ws),
                                label,
                            )(rest)?;
                            let (rest, _) = ws(rest)?;
                            let (rest, _) = opt(terminated(char(','), ws))(rest)?;
                            Ok((rest, ls))
                        },
                        char('}'),
                    ),
                    |ls| {
                        let set: BTreeSet<_> = ls.into_iter().collect();
                        mkexpr(ExprKind::Op(crate::operations::OpKind::Projection(expr.clone(), set)))
                    },
                ),
                // .(T) — projection by expression
                map(
                    delimited(
                        terminated(char('('), ws),
                        expression,
                        preceded(ws, char(')')),
                    ),
                    |e| mkexpr(ExprKind::Op(crate::operations::OpKind::ProjectionByExpr(expr.clone(), e))),
                ),
                // .field — field access
                map(label, |l| {
                    mkexpr(ExprKind::Op(crate::operations::OpKind::Field(expr.clone(), l)))
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
fn completion_expression(input: &str) -> ParseResult<Expr> {
    let (mut rest, mut expr) = selector_expression(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws(rest)?;
            let (r, _) = tag("::")(r)?;
            let (r, _) = ws(r)?;
            let (r, rhs) = selector_expression(r)?;
            Ok((r, mkexpr(ExprKind::Op(crate::operations::OpKind::Completion(expr.clone(), rhs)))))
        })();
        match tried {
            Ok((r, e)) => { expr = e; rest = r; }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Keyword-prefixed application: `Some e`, `merge x y`, `toMap x`
fn first_application(input: &str) -> ParseResult<Expr> {
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

fn some_application(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("Some")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, e) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::SomeLit(e))))
}

fn merge_application(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, y) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::Merge(x, y, None)))))
}

fn tomap_application(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::ToMap(x, None)))))
}

/// import-expression = import / completion-expression
fn import_expression(input: &str) -> ParseResult<Expr> {
    alt((import_expr, completion_expression))(input)
}

/// Function application: `f a b` = `App(App(f, a), b)`
/// ABNF: first-application-expression *(whsp1 import-expression)
fn application(input: &str) -> ParseResult<Expr> {
    let (mut rest, mut expr) = first_application(input)?;
    loop {
        let tried = (|| -> ParseResult<Expr> {
            let (r, _) = ws1(rest)?;
            let (r, arg) = import_expression(r)?;
            Ok((r, arg))
        })();
        match tried {
            Ok((r, arg)) => {
                expr = mkexpr(ExprKind::Op(crate::operations::OpKind::App(expr, arg)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

// ── 9. Operators (full precedence tower) ─────────────────────────────
//
// Lowest precedence at the top, highest at the bottom.
// All operators are left-associative.
// Each level parses its operator and delegates to the next level for operands.

/// Helper: build a left-associative binary operator parser for one precedence level.
macro_rules! binop_level {
    // Single operator — no alt() needed
    ($name:ident, $next:ident, $op_tag:expr => $op_variant:expr) => {
        fn $name(input: &str) -> ParseResult<Expr> {
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
                        lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(op, lhs, rhs)));
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
        fn $name(input: &str) -> ParseResult<Expr> {
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
                        lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(op, lhs, rhs)));
                        rest = r;
                    }
                    Err(_) => break,
                }
            }
            Ok((rest, lhs))
        }
    };
}

use crate::operations::BinOp::*;

/// Match `==` but not `===`
fn op_bool_eq(input: &str) -> ParseResult<crate::operations::BinOp> {
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
fn import_alt_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(ImportAlt, lhs, rhs)));
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
fn plus_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(NaturalPlus, lhs, rhs)));
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

fn combine_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(RecursiveRecordMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// Match `//` but not `//\\`
fn op_prefer_ascii(input: &str) -> ParseResult<&str> {
    let (rest, _) = tag("//")(input)?;
    if rest.starts_with('\\') {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    } else {
        Ok((rest, "//"))
    }
}

fn prefer_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(RightBiasedRecordMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

fn combine_types_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(RecursiveRecordTypeMerge, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

/// `==` level needs special handling to not consume `===`.
fn bool_eq_expr(input: &str) -> ParseResult<Expr> {
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
                lhs = mkexpr(ExprKind::Op(crate::operations::OpKind::BinOp(op, lhs, rhs)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, lhs))
}

fn operator_expression(input: &str) -> ParseResult<Expr> {
    equiv_expr(input)
}

// ── 10. Top-level expressions ────────────────────────────────────────

fn let_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("let")(input)?;
    let (mut rest, _) = ws1(rest)?;
    let mut bindings = Vec::new();
    loop {
        let (r, name) = terminated(label, ws)(rest)?;
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

fn lambda_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = alt((tag("\\"), tag("λ")))(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('(')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = terminated(label, ws)(rest)?;
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

fn if_expression(input: &str) -> ParseResult<Expr> {
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
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::BoolIf(cond, t, f)))))
}

fn forall_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = alt((tag("forall"), tag("∀")))(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('(')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, name) = terminated(label, ws)(rest)?;
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

fn assert_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("assert")(input)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, e) = expression(rest)?;
    Ok((rest, mkexpr(ExprKind::Assert(e))))
}

/// `merge x y : T` (with type annotation)
fn merge_annot_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("merge")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, y) = import_expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::Merge(x, y, Some(ty))))))
}

/// `toMap x : T` (with type annotation)
fn tomap_annot_expression(input: &str) -> ParseResult<Expr> {
    let (rest, _) = keyword("toMap")(input)?;
    let (rest, _) = ws1(rest)?;
    let (rest, x) = import_expression(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, ty) = application(rest)?;
    Ok((rest, mkexpr(ExprKind::Op(crate::operations::OpKind::ToMap(x, Some(ty))))))
}

/// `with` expression: `e with a.b.c = v`
/// ABNF: import-expression 1*(whsp1 with whsp1 with-clause)
/// Only matches if at least one `with` clause is present.
fn with_expression(input: &str) -> ParseResult<Expr> {
    let (rest, base) = import_expression(input)?;
    // Must have at least one `with` clause
    let (rest, _) = ws1(rest)?;
    let (rest, _) = keyword("with")(rest)?;
    let (rest, _) = ws1(rest)?;
    let (rest, labels) = separated_list0(
        delimited(ws, char('.'), ws),
        label,
    )(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, _) = char('=')(rest)?;
    let (rest, _) = ws(rest)?;
    let (rest, val) = operator_expression(rest)?;
    let mut expr = mkexpr(ExprKind::Op(crate::operations::OpKind::With(base, labels, val)));
    // Additional with clauses
    let mut rest = rest;
    loop {
        let tried = (|| -> ParseResult<(Vec<Label>, Expr)> {
            let (r, _) = ws1(rest)?;
            let (r, _) = keyword("with")(r)?;
            let (r, _) = ws1(r)?;
            let (r, labels) = separated_list0(
                delimited(ws, char('.'), ws),
                label,
            )(r)?;
            let (r, _) = ws(r)?;
            let (r, _) = char('=')(r)?;
            let (r, _) = ws(r)?;
            let (r, val) = operator_expression(r)?;
            Ok((r, (labels, val)))
        })();
        match tried {
            Ok((r, (labels, val))) => {
                expr = mkexpr(ExprKind::Op(crate::operations::OpKind::With(expr, labels, val)));
                rest = r;
            }
            Err(_) => break,
        }
    }
    Ok((rest, expr))
}

/// Arrow type: `A -> B` (non-dependent function type)
/// ABNF: operator-expression whsp arrow whsp expression
/// Falls through to annotated-expression if no arrow found.
fn arrow_or_annot_expression(input: &str) -> ParseResult<Expr> {
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
pub fn expression(input: &str) -> ParseResult<Expr> {
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

    // ── Known failures (from spec tests) ─────────────────────────

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
}
