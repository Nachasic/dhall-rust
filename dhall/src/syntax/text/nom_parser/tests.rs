use super::expression::parse_expr;

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
