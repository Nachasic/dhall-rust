use alloc::format;
use alloc::string::{String, ToString};
use super::input::Input;
use super::labels::{RESERVED, is_builtin_name};

pub(super) fn diagnose_leftover(remaining: &str, _had_leading_ws: bool, before: &str) -> String {
    let trimmed = remaining.trim_start();
    if trimmed.starts_with('(') && !remaining.starts_with(|c: char| c.is_whitespace()) {
        "function application requires a space before `(` (e.g. `f (x)` not `f(x)`)".into()
    } else if trimmed.starts_with(':') && !trimmed.starts_with("::") {
        if before.ends_with(" sha256") || before.ends_with("\tsha256") || before.ends_with("\nsha256") || before == "sha256" {
            "`sha256:` integrity hash must be attached to an import, not a parenthesized expression; move it inside the parentheses".into()
        } else {
            let after_colon = &trimmed[1..];
            if after_colon.starts_with(|c: char| c.is_whitespace()) || after_colon.is_empty() {
                "unexpected `:` — type annotations are not allowed at this position; try parenthesizing the expression".into()
            } else {
                "type annotation requires whitespace after `:` (e.g. `x : T` not `x :T`)".into()
            }
        }
    } else if trimmed.starts_with("with") && trimmed[4..].starts_with(|c: char| !c.is_alphanumeric() && c != '_') {
        "`with` cannot be used at this precedence level; try parenthesizing the left-hand side".into()
    } else if trimmed.starts_with('+') && !trimmed.starts_with("++") {
        "the `+` operator requires whitespace on both sides (e.g. `x + y`)".into()
    } else if trimmed.starts_with("Some") && trimmed[4..].starts_with(|c: char| !c.is_alphanumeric() && c != '_') {
        "`Some` is a keyword and cannot be used as a function argument; try parenthesizing it".into()
    } else if trimmed.starts_with(".{") && trimmed.contains(':') {
        "projection by type requires parentheses: use `r.(T)` instead of `r.{ x: T }`".into()
    } else {
        "unexpected input; expected operator, end of input, or whitespace-separated expression".into()
    }
}

/// Format a VerboseError into a human-readable message with line/column info,
/// source context, and a caret pointing at the error position.
pub(super) fn format_verbose_error(input: &str, err: &nom::error::VerboseError<Input<'_>>) -> String {
    use nom::error::VerboseErrorKind;

    // Find the deepest (most specific) error position — prefer the context
    // that consumed the most input (i.e. smallest remaining fragment).
    // When multiple contexts have the same position, prefer the last one
    // (outermost wrapper, which is typically more descriptive), unless
    // it's the generic "expression" context.
    let (err_input, kind) = err.errors.iter()
        .filter(|(_, k)| matches!(k, VerboseErrorKind::Context(_)))
        .min_by(|(a, ka), (b, kb)| {
            a.fragment.len().cmp(&b.fragment.len()).then_with(|| {
                // At the same position, prefer non-"expression" contexts
                let a_generic = matches!(ka, VerboseErrorKind::Context("expression"));
                let b_generic = matches!(kb, VerboseErrorKind::Context("expression"));
                match (a_generic, b_generic) {
                    (true, false) => core::cmp::Ordering::Greater,
                    (false, true) => core::cmp::Ordering::Less,
                    _ => core::cmp::Ordering::Greater, // prefer later (outermost)
                }
            })
        })
        .or_else(|| err.errors.iter().min_by_key(|(i, _)| i.fragment.len()))
        .unwrap_or(&err.errors[0]);

    let offset = input.len() - err_input.fragment.len();
    let prefix = &input[..offset];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_nl = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = prefix[last_nl..].chars().count() + 1;

    // Extract the source line
    let line_start = last_nl;
    let line_end = input[offset..].find('\n').map(|i| offset + i).unwrap_or(input.len());
    let source_line = &input[line_start..line_end];

    // Build the caret indicator
    let caret_offset = col - 1;
    let caret = format!("{}^---", " ".repeat(caret_offset));

    // Use the most specific context label (the one that consumed the most input).
    // When tied, prefer non-"expression" contexts, then prefer the last (outermost).
    let best_context = err.errors.iter()
        .filter_map(|(i, k)| match k {
            VerboseErrorKind::Context(ctx) => Some((i.fragment.len(), *ctx)),
            _ => None,
        })
        .min_by(|(a_len, a_ctx), (b_len, b_ctx)| {
            a_len.cmp(b_len).then_with(|| {
                let a_generic = *a_ctx == "expression";
                let b_generic = *b_ctx == "expression";
                match (a_generic, b_generic) {
                    (true, false) => core::cmp::Ordering::Greater,
                    (false, true) => core::cmp::Ordering::Less,
                    _ => core::cmp::Ordering::Greater,
                }
            })
        })
        .map(|(_, ctx)| ctx);

    let line_num_width = format!("{}", line).len();
    let padding = " ".repeat(line_num_width);

    let mut msg = format!(
        " --> {}:{}\n{} |\n{} | {}\n{} | {}\n{} |",
        line, col, padding, line, source_line, padding, caret, padding
    );

    // Try to produce a better message than just "expected expression"
    let hint = if best_context == Some("expression") {
        diagnose_atom_failure(err_input.fragment)
    } else if best_context == Some("variable name in `let` binding") {
        diagnose_bad_label(err_input.fragment, "variable name in `let` binding")
    } else {
        None
    };

    if let Some(hint) = hint {
        msg.push_str(&format!("\n{} = {}", padding, hint));
    } else if let Some(ctx) = best_context {
        if ctx.starts_with(|c: char| c.is_uppercase()) {
            msg.push_str(&format!("\n{} = {}", padding, ctx));
        } else {
            msg.push_str(&format!("\n{} = expected {}", padding, ctx));
        }
    } else {
        // Fall back to the nom error kind
        let hint = match kind {
            VerboseErrorKind::Nom(k) => format!("{:?}", k),
            VerboseErrorKind::Char(c) => format!("'{}'", c),
            VerboseErrorKind::Context(c) => c.to_string(),
        };
        msg.push_str(&format!("\n{} = expected {}", padding, hint));
    }

    msg
}

/// When `nonreserved_label` fails, explain why the identifier is rejected.
pub(super) fn diagnose_bad_label(at: &str, context_msg: &str) -> Option<String> {
    // Extract the identifier at the error position
    let word: String = at.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '/').collect();
    if word.is_empty() {
        return None;
    }
    if RESERVED.contains(&word.as_str()) {
        Some(format!("`{}` is a reserved keyword and cannot be used as a {}", word, context_msg))
    } else if is_builtin_name(&word) {
        Some(format!("`{}` is a builtin and cannot be used as a {}", word, context_msg))
    } else {
        None
    }
}

/// When the parser fails at the atom level ("expected expression"), try to
/// diagnose the specific problem from the input text.
pub(super) fn diagnose_atom_failure(at: &str) -> Option<String> {
    let trimmed = at.trim_start();

    // [] without type annotation
    if trimmed.starts_with("[]") {
        return Some("empty list requires a type annotation: `[] : List T`".into());
    }

    // Keywords used bare (without required arguments/structure)
    for (kw, hint) in &[
        ("merge", "`merge` requires at least two arguments: `merge handler union`"),
        ("Some", "`Some` requires an argument: `Some value`"),
        ("toMap", "`toMap` requires an argument: `toMap record`"),
        ("assert", "`assert` requires a type annotation: `assert : expr`"),
    ] {
        if trimmed.starts_with(kw) {
            let rest = &trimmed[kw.len()..];
            if rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '/') {
                return Some((*hint).into());
            }
        }
    }

    // Keywords that require whitespace before `(`
    for (kw, name) in &[
        ("if", "if"),
        ("forall", "forall"),
    ] {
        if trimmed.starts_with(kw) {
            let rest = &trimmed[kw.len()..];
            if rest.starts_with('(') {
                return Some(format!("`{}` requires a space before `(` (e.g. `{} (x)`)", name, name));
            }
        }
    }

    // Lambda without space
    if trimmed.starts_with('\\') || trimmed.starts_with('λ') {
        let rest = if trimmed.starts_with('\\') { &trimmed[1..] } else { &trimmed['λ'.len_utf8()..] };
        if rest.starts_with('(') {
            return Some("`λ`/`\\` requires a space before `(` (e.g. `\\(x : T) -> x`)".into());
        }
    }

    // Some(x) without space
    if trimmed.starts_with("Some(") {
        return Some("`Some` requires a space before its argument: `Some (x)` not `Some(x)`".into());
    }

    // merge(x) without space
    if trimmed.starts_with("merge(") {
        return Some("`merge` requires a space before its arguments: `merge handler union`".into());
    }

    // Keyword used as record field
    for kw in RESERVED {
        if trimmed.starts_with('{') {
            // Already inside record — check if the error is at a keyword position
            // This is handled by the record parser, not here
        }
        if trimmed.starts_with(kw) {
            let rest = &trimmed[kw.len()..];
            if rest.starts_with(':') || rest.starts_with(' ') && rest.trim_start().starts_with(':') {
                return Some(format!("`{}` is a reserved keyword and cannot be used as a field name; use backticks: `` `{}` ``", kw, kw));
            }
        }
    }

    // Leading zeros in natural
    if trimmed.starts_with('0') && trimmed.len() > 1 {
        let second = trimmed.as_bytes().get(1).copied();
        if second.map_or(false, |b| b.is_ascii_digit()) {
            return Some("natural literals cannot have leading zeros (use `0x` prefix for hexadecimal)".into());
        }
    }

    // Builtin with de Bruijn index
    for name in &["True", "False", "Type", "Kind", "Sort",
                   "Bool", "Natural", "Integer", "Double", "Text",
                   "List", "Optional", "None"] {
        if trimmed.starts_with(name) {
            let rest = &trimmed[name.len()..];
            if rest.starts_with('@') {
                return Some(format!("`{}` is a builtin and cannot have a de Bruijn index (`@`)", name));
            }
        }
    }

    // Double out of bounds
    if trimmed.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
        if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
            return Some("double literal is out of the representable range".into());
        }
    }

    // { with keyword field
    if trimmed.starts_with('{') {
        let inner = trimmed[1..].trim_start();
        // Check for leading comma
        let inner = if inner.starts_with(',') { inner[1..].trim_start() } else { inner };
        for kw in RESERVED {
            if inner.starts_with(kw) {
                let after_kw = &inner[kw.len()..];
                if after_kw.starts_with(|c: char| c == ':' || c == '=' || c == ',' || c == '}' || c.is_whitespace()) {
                    return Some(format!("`{}` is a reserved keyword and cannot be used as a record field name; use backticks: `\\`{}\\``", kw, kw));
                }
            }
        }
    }

    // < with duplicate separator
    if trimmed.starts_with('<') {
        let inner = trimmed[1..].trim_start();
        if inner.starts_with("||") || inner.starts_with("| |") {
            return Some("unexpected `|` in union type".into());
        }
    }

    // [ or { with duplicate comma
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        let inner = trimmed[1..].trim_start();
        let inner = if inner.starts_with(',') { inner[1..].trim_start() } else { inner };
        if inner.starts_with(',') {
            return Some("unexpected `,` — duplicate commas are not allowed".into());
        }
    }

    // Old union literal syntax: < x = 3 | ... >
    if trimmed.starts_with('<') {
        let inner = trimmed[1..].trim_start();
        // Skip leading |
        let inner = if inner.starts_with('|') { inner[1..].trim_start() } else { inner };
        // Look for `label = expr` pattern
        if let Some(eq_pos) = inner.find('=') {
            let before_eq = inner[..eq_pos].trim();
            if !before_eq.is_empty() && before_eq.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '/') {
                return Some("union literal syntax `< x = value >` is no longer supported; use `(< X : T | ... >).X value` instead".into());
            }
        }
    }

    // `let assert = ...` — assert is a keyword
    if trimmed.starts_with("let") {
        let rest = trimmed[3..].trim_start();
        if rest.starts_with("assert") {
            return Some("`assert` is a reserved keyword and cannot be used as a variable name".into());
        }
    }

    None
}

