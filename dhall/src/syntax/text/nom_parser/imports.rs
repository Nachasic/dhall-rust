use alloc::string::String;
use alloc::vec;
use nom::{branch::alt, bytes::complete::{tag, take_while, take_while1},
    character::complete::char,
    combinator::{cut, map, opt, value},
    error::context,
    multi::many0,
    sequence::{delimited, preceded, terminated}};
use super::input::Input;
use super::helpers::*;
use super::helpers::keyword;
use super::application::import_expression;
use crate::syntax::{Expr, ExprKind, FilePath, FilePrefix, Hash, ImportMode, ImportTarget, Scheme, URL};

/// A single path component without leading /
pub(super) fn path_component_body(input: Input<'_>) -> ParseResult<'_, String> {
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
                |s: Input<'_>| s.fragment.to_owned(),
            ),
            char('"'),
        ),
        map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@".contains(c)),
            |s: Input<'_>| s.fragment.to_owned(),
        ),
    ))(input)
}

pub(super) fn path_component(input: Input<'_>) -> ParseResult<'_, String> {
    preceded(char('/'), path_component_body)(input)
}

pub(super) fn absolute_path_prefix(input: Input<'_>) -> ParseResult<'_, FilePrefix> {
    let (rest, _) = char('/')(input)?;
    if rest.is_empty() || rest.starts_with_char('\\') || rest.starts_with_char('/') || rest.starts_with_char(' ') {
        Err(tag_err(input))
    } else {
        Ok((rest, FilePrefix::Absolute))
    }
}

pub(super) fn local_import(input: Input<'_>) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, prefix) = alt((
        value(FilePrefix::Parent, tag("../")),
        value(FilePrefix::Here, tag("./")),
        value(FilePrefix::Home, tag("~/")),
        absolute_path_prefix,
    ))(input)?;

    let (rest, first) = path_component_body(rest)?;
    if prefix != FilePrefix::Absolute && first.is_empty() {
        return Err(make_err(input, nom::error::ErrorKind::TakeWhile1));
    }
    let (rest, mut more) = many0(path_component)(rest)?;
    let mut components = vec![first];
    components.append(&mut more);

    Ok((rest, ImportTarget::Local(prefix, FilePath { file_path: components })))
}

/// HTTP(S) import: https://example.com/foo/bar.dhall [using headers]
pub(super) fn http_import(input: Input<'_>) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, scheme) = alt((
        value(Scheme::HTTPS, tag("https://")),
        value(Scheme::HTTP, tag("http://")),
    ))(input)?;

    // Authority: everything up to the first /
    let (rest, authority) = map(
        take_while1(|c: char| c != '/' && c != '?' && c != '#' && !c.is_whitespace()),
        |s: Input<'_>| s.fragment.to_owned(),
    )(rest)?;

    // Path segments (URL paths allow percent-encoding)
    let (rest, segments) = many0(preceded(
        char('/'),
        map(
            take_while(|c: char| c.is_ascii_alphanumeric() || "-._~!$&'*+;=:@%".contains(c)),
            |s: Input<'_>| s.fragment.to_owned(),
        ),
    ))(rest)?;
    let file_path = if segments.is_empty() { vec!["".to_owned()] } else { segments };

    // Optional query
    let (rest, query) = opt(preceded(
        char('?'),
        map(take_while(|c: char| c != ' ' && c != '\n' && c != '\r'), |s: Input<'_>| s.fragment.to_owned()),
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
pub(super) fn env_import(input: Input<'_>) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, _) = tag("env:")(input)?;
    let (rest, name) = alt((
        // Quoted: env:"NAME" (POSIX env var with escapes)
        delimited(char('"'), posix_env_var, char('"')),
        // Unquoted: env:NAME (bash-style)
        map(take_while1(|c: char| c.is_ascii_alphanumeric() || c == '_'), |s: Input<'_>| s.fragment.to_owned()),
    ))(rest)?;
    Ok((rest, ImportTarget::Env(name)))
}

/// Parse a POSIX-compliant quoted environment variable name.
pub(super) fn posix_env_var(input: Input<'_>) -> ParseResult<'_, String> {
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
        return Err(make_err(input, nom::error::ErrorKind::TakeWhile1));
    }
    Ok((rest, chars.into_iter().collect()))
}

/// `missing` keyword — only needs to not be a prefix of an identifier
pub(super) fn missing_import(input: Input<'_>) -> ParseResult<'_, ImportTarget<Expr>> {
    let (rest, _) = tag("missing")(input)?;
    if rest.fragment.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/') {
        return Err(tag_err(input));
    }
    Ok((rest, ImportTarget::Missing))
}

/// SHA256 hash: sha256:hex...
pub(super) fn import_hash(input: Input<'_>) -> ParseResult<'_, Hash> {
    let (rest, _) = tag("sha256:")(input)?;
    // After sha256:, commit — this is unambiguously a hash attempt
    let (rest, hex_str) = cut(context(
        "64 hex digits after `sha256:` (integrity hash contains non-hex characters)",
        take_while1(|c: char| c.is_ascii_hexdigit()),
    ))(rest)?;
    // Reject if followed by more alphanumeric chars (truncated match)
    if rest.fragment.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        return Err(nom::Err::Failure(nom::error::VerboseError {
            errors: alloc::vec![(rest, nom::error::VerboseErrorKind::Context(
                "Integrity hash contains non-hex characters"
            ))],
        }));
    }
    let bytes = hex::decode(hex_str.fragment).map_err(|_| nom::Err::Failure(nom::error::VerboseError {
        errors: alloc::vec![(input, nom::error::VerboseErrorKind::Context(
            "Integrity hash must be exactly 64 hex digits"
        ))],
    }))?;
    Ok((rest, Hash::SHA256(bytes.into())))
}

/// Full import expression: location hash? (as Text | as Location)?
pub(super) fn import_expr(input: Input<'_>) -> ParseResult<'_, Expr> {
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
    Ok((rest, spanned(input, rest, ExprKind::Import(import))))
}

