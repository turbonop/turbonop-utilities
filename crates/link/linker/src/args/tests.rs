use super::*;

fn words(s: &str) -> Vec<String> {
    split_words(s)
}

#[test]
fn splits_like_the_c_runtime() {
    assert_eq!(words("a b\tc\r\nd"), ["a", "b", "c", "d"]);
    assert_eq!(words("  lead  trail  "), ["lead", "trail"]);
    assert_eq!(words(r#""a b" c"#), ["a b", "c"]);
    assert_eq!(words(r#"/out:"c:\x y\a.exe""#), [r"/out:c:\x y\a.exe"]);
    // Quotes inside a word join it.
    assert_eq!(words(r#"ab"c d"e"#), ["abc de"]);
    // An empty quoted word is a word.
    assert_eq!(words(r#"a "" b"#), ["a", "", "b"]);
    // `""` inside quotes is a quote.
    assert_eq!(words(r#""a""b""#), [r#"a"b"#]);
    // Backslashes: literal, except before a quote.
    assert_eq!(words(r"c:\dir\file.obj"), [r"c:\dir\file.obj"]);
    assert_eq!(words(r#"a\"b"#), [r#"a"b"#]);
    assert_eq!(words(r#"a\\"b c""#), [r"a\b c"]);
    assert_eq!(words(r#"a\\\"b"#), [r#"a\"b"#]);
    assert_eq!(words(r#""c:\dir\\" next"#), [r"c:\dir\", "next"]);
    assert_eq!(words(r"trailing\\"), [r"trailing\\"]);
    // An unterminated quote runs to the end.
    assert_eq!(words(r#""open to the end"#), ["open to the end"]);
    assert!(words("").is_empty());
    assert!(words(" \r\n\t").is_empty());
}

#[test]
fn classifies_options_and_inputs() {
    assert_eq!(
        Arg::parse("/OUT:a.exe"),
        Arg::Option {
            name: "out".into(),
            value: Some("a.exe".into())
        }
    );
    assert_eq!(
        Arg::parse("-opt:REF,ICF"),
        Arg::Option {
            name: "opt".into(),
            value: Some("REF,ICF".into())
        }
    );
    // Only the first colon splits.
    assert_eq!(
        Arg::parse(r"/LIBPATH:C:\lib"),
        Arg::Option {
            name: "libpath".into(),
            value: Some(r"C:\lib".into())
        }
    );
    assert_eq!(
        Arg::parse("/Debug"),
        Arg::Option {
            name: "debug".into(),
            value: None
        }
    );
    // An empty value is a value.
    assert_eq!(
        Arg::parse("/out:"),
        Arg::Option {
            name: "out".into(),
            value: Some(String::new())
        }
    );
    assert_eq!(Arg::parse("main.obj"), Arg::Input("main.obj".into()));
    assert_eq!(Arg::parse(r"C:\x\y.lib"), Arg::Input(r"C:\x\y.lib".into()));
    assert_eq!(Arg::parse("/OPT:REF").to_string(), "/OPT:REF");
    assert_eq!(Arg::parse("/dll").to_string(), "/DLL");
}

#[test]
fn decodes_response_files() {
    assert_eq!(decode_text(b"a b"), "a b");
    assert_eq!(decode_text(b"\xEF\xBB\xBFa b"), "a b");
    let le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("x é".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(decode_text(&le), "x é");
    let be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain("y".encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    assert_eq!(decode_text(&be), "y");
    // Not UTF-8 (an ANSI file's é): replaced, the rest kept.
    assert_eq!(decode_text(b"a\xE9b"), "a\u{FFFD}b");
}

fn files(name: &str) -> std::io::Result<Vec<u8>> {
    match name {
        "one.rsp" => Ok(b"/nologo a.obj\r\n@two.rsp\r\nz.obj".to_vec()),
        "two.rsp" => Ok(b"\"b c.obj\" /out:x.exe".to_vec()),
        "self.rsp" => Ok(b"@self.rsp".to_vec()),
        _ => Err(std::io::ErrorKind::NotFound.into()),
    }
}

#[test]
fn expands_response_files_in_place() {
    let got = expand_response_files(
        ["first".to_owned(), "@one.rsp".to_owned(), "last".to_owned()],
        &files,
    )
    .unwrap();
    assert_eq!(
        got,
        [
            "first",
            "/nologo",
            "a.obj",
            "b c.obj",
            "/out:x.exe",
            "z.obj",
            "last"
        ]
    );
    let err = expand_response_files(["@missing.rsp".to_owned()], &files).unwrap_err();
    assert!(err.message.contains("missing.rsp"), "{err}");
    let err = expand_response_files(["@self.rsp".to_owned()], &files).unwrap_err();
    assert!(err.message.contains("nested"), "{err}");
}

#[test]
fn environment_comes_before_and_after() {
    let got = command_line(
        ["/out:a.exe".to_owned(), "m.obj".to_owned()],
        Some("/nologo /debug"),
        Some("extra.lib"),
        &files,
    )
    .unwrap();
    let text: Vec<String> = got.iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        ["/NOLOGO", "/DEBUG", "/OUT:a.exe", "m.obj", "extra.lib"]
    );
}

#[test]
fn reads_directives() {
    let got = directives(b"   /DEFAULTLIB:\"LIBCMT\" /DEFAULTLIB:\"OLDNAMES\" /EXPORT:f,DATA \0\0");
    let text: Vec<String> = got.iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        [
            "/DEFAULTLIB:LIBCMT",
            "/DEFAULTLIB:OLDNAMES",
            "/EXPORT:f,DATA"
        ]
    );
    assert!(directives(b"").is_empty());
    assert!(directives(b"\xEF\xBB\xBF /merge:a=b").len() == 1);
}
