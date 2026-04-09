# Dhall nom Parser

A hand-written recursive-descent parser for the [Dhall configuration language](https://dhall-lang.org), built on the [`nom`](https://docs.rs/nom) parser combinator library. It implements the [Dhall ABNF grammar](https://github.com/dhall-lang/dhall-lang/blob/master/standard/dhall.abnf) and passes all 1937 specification tests.

## LLM usage disclosure

Code of this parser was generated with help of Claude Sonnet, through multiple iterations, with careful architectural oversight, review and hand-written adjustments from a human maintainer. LLM assistance was used to create the basic structure of the parser as well as making it compliant with the Dhall language specification. The extensive test suite created by the original maintainers of `dhall-rust` was used as an integral part of parser development and assessment.

In accordance with the "human-in-the-loop" model, the code of this parser inherits the original BSD-2 licence

## Module Structure

The parser is split into focused modules that follow the grammar bottom-up:

| Module | Purpose |
|---|---|
| `input.rs` | Custom `Input<'a>` wrapper over `&str` that carries source tracking for span creation. Implements all required nom traits. |
| `helpers.rs` | Shared types (`ParseResult`, `InputVerboseError`), error constructors (`make_err`, `tag_err`), whitespace/comment handling (`ws`, `ws1`), and the `keyword` combinator. |
| `literals.rs` | Numeric literals (natural, integer, double) and string literals (double-quoted with escapes/interpolation, single-quoted multi-line with indent stripping). |
| `labels.rs` | Identifiers, reserved words, builtins, backtick-quoted labels, and de Bruijn–indexed variables. |
| `imports.rs` | Local, HTTP, environment, and `missing` imports, plus `sha256:` integrity hashes and `as Text`/`as Location` modes. |
| `structure.rs` | Atom-level expressions: parenthesized expressions, record literals/types (with puns and dotted fields), union types, and list literals. |
| `application.rs` | Field access, projection, completion (`T::r`), keyword-prefixed application (`Some`, `merge`, `toMap`), and general function application. |
| `operators.rs` | Full operator precedence tower via the `binop_level!` macro, with hand-written parsers for ambiguous operators (`/\`, `//`, `//\\`). |
| `expression.rs` | Top-level expression forms (`let`, `λ`, `if`, `∀`, `assert`, `with`, `→`, `:` annotation), the `expression()` entry point, and `parse_expr()` — the public API. |
| `errors.rs` | Error formatting and diagnostic heuristics for producing human-readable parse error messages. |
| `tests.rs` | Unit tests for the parser. |

## `no_std` Support

The parser is `no_std`-compatible. All heap allocation goes through `alloc` (`alloc::vec`, `alloc::string`, `alloc::rc::Rc`, etc.) rather than `std`. The `std` feature is optional and only affects error formatting — when enabled, the `annotate-snippets` crate renders richer error output; without it, errors are formatted manually.

## How Parsing Works

### Input Tracking

The `Input<'a>` type wraps a `&str` slice alongside a pointer to the full source `Rc<str>`. This lets any parser compute its byte offset via pointer arithmetic and create `Span::Parsed` values without threading position state through every combinator. The `Rc<str>` is cloned only when a span is actually created.

### Grammar Mapping

Each grammar production maps to a Rust function returning `ParseResult<'_, T>`. The parser is organized as a precedence-climbing tower:

```
expression          (top: let, λ, if, ∀, assert, with, →, :)
  └─ operator_expression   (equiv → import_alt → or → ... → application)
       └─ application      (first_application *(ws1 import_expression))
            └─ first_application  (Some/merge/toMap or import_expression)
                 └─ import_expression  (import or completion_expression)
                      └─ selector_expression  (atom (.field | .{proj} | .(T))*)
                           └─ atom  (parens | literal | record | union | list | import | builtin | variable)
```

Operators use the `binop_level!` macro to generate left-associative binary operator parsers from a simple declaration:

```rust
binop_level!(or_expr, text_append_expr, "||" => BoolOr);
```

### Committed Parsing with `cut`

After a keyword is recognized (e.g. `if`, `let`, `merge`), the parser uses nom's `cut` combinator to commit to that branch. This prevents backtracking to the generic `atom` parser and ensures that subsequent failures produce specific error messages rather than the unhelpful "expected expression".

For example, in `if_expression`:

```rust
let (rest, _) = keyword("if")(input)?;                          // try: can backtrack
let (rest, _) = cut(context("whitespace after `if`", ws1))(rest)?;  // committed
let (rest, cond) = cut(context("condition after `if`", expression))(rest)?;
```

If the input is `if(b) then x else y`, the `keyword("if")` succeeds, then `cut(ws1)` fails with a `Failure` (not `Error`), producing: "expected whitespace after `if`".

### Uncommitted Fallthrough

Some expressions try a more specific form first and fall through to a general one. For example, `merge_annot_expression` tries `merge x y : T`. The `:` match is *not* behind `cut`, so if there's no `:`, it returns `Error`, `alt` catches it, and eventually `merge_application` (via `arrow_or_annot_expression`) handles the `merge x y` case without annotation.

## Error Reporting

Errors are produced at three levels, each with its own strategy:

### 1. Committed Parse Errors (nom `Failure`)

When `cut(context("...", parser))` fails, nom produces a `Failure` with a context label. The `format_verbose_error` function renders this as:

```
 --> 1:5
  |
1 | let assert = 2 in 1
  |     ^---
  |
  = `assert` is a reserved keyword and cannot be used as a variable name in `let` binding
```

### 2. Leftover Input (partial parse)

When `expression` succeeds but doesn't consume all input, `parse_expr` calls `diagnose_leftover()` which inspects the remaining text to produce targeted hints:

- `:` without space after → "type annotation requires whitespace after `:`"
- `: T` at wrong precedence → "type annotations are not allowed at this position"
- `(` without space before → "function application requires a space before `(`"
- `with` after operator → "`with` cannot be used at this precedence level"
- `.{ x: T }` → "projection by type requires parentheses: use `r.(T)`"

### 3. Atom-Level Failures (heuristic diagnosis)

When the parser fails at the `atom` level, the only context is the generic "expression". The `diagnose_atom_failure()` function inspects the input text to identify common mistakes:

- `[]` → "empty list requires a type annotation: `[] : List T`"
- `042` → "natural literals cannot have leading zeros"
- `True@0` → "`True` is a builtin and cannot have a de Bruijn index"
- `{ if: Text }` → "`if` is a reserved keyword and cannot be used as a record field name"
- `< x = 3 >` → "union literal syntax is no longer supported"

### Context Selection

When multiple `context` labels exist at the same error position, the formatter uses a priority scheme:

1. Prefer the context at the deepest position (smallest remaining input).
2. At the same position, prefer non-generic labels over `"expression"`.
3. Among non-generic labels at the same position, prefer the outermost (last in the error chain), since it's typically the most descriptive wrapper.

## Notable Patterns

- **`keyword()` combinator**: Matches a tag and verifies the next character isn't alphanumeric/`_`/`-`/`/`, preventing `"if"` from matching the prefix of `"iffy"`.

- **Record literal vs type**: Both use `{ ... }` syntax. The parser collects entries as `(Label, char, Expr)` tuples where the `char` is `=` or `:`, then decides the form based on whether all separators are `:`.

- **Duplicate detection**: Record types, union types, and projections check for duplicate fields/variants at parse time, producing `Failure` errors that propagate through `alt`.

- **Multi-line string indent stripping**: The single-quote literal parser splits text on newlines, computes the minimum indent across non-empty lines, and strips that many characters from each line's leading whitespace.

- **`binop_level!` macro**: Generates left-associative binary operator parsers. Supports both single-operator and multi-operator forms. Each level tries its operator in a loop, delegating to the next precedence level for operands.
