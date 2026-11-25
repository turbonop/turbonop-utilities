use super::*;
use std::collections::HashMap;

fn w(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

fn files(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> io::Result<Vec<u8>> {
    let map: HashMap<String, Vec<u8>> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();
    move |p: &str| {
        map.get(p)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, p.to_string()))
    }
}

#[test]
fn lib_splits_on_spaces_and_drops_quotes() {
    // Measured: /OUT:q"uo"te.lib is quote.lib, and "esc\"x.lib" is esc\x.lib.
    assert_eq!(
        split_lib("/OUT:q\"uo\"te.lib a.obj\r\n\tb.obj"),
        w(&["/OUT:quote.lib", "a.obj", "b.obj"])
    );
    assert_eq!(split_lib(r#"/OUT:"esc\"x.lib""#), w(&[r"/OUT:esc\x.lib"]));
    assert_eq!(
        split_lib(r#""sp ace\x y.obj" c"#),
        w(&[r"sp ace\x y.obj", "c"])
    );
    assert_eq!(split_lib("  "), Vec::<String>::new());
    assert_eq!(split_lib(r#""""#), w(&[""]));
}

#[test]
fn windows_rules() {
    assert_eq!(split_windows(r#"a "b c" d"#), w(&["a", "b c", "d"]));
    // Backslashes escape quotes only.
    assert_eq!(split_windows(r#"a\\b \"q\" "#), w(&[r"a\\b", r#""q""#]));
    assert_eq!(split_windows(r#""x\\" y"#), w(&[r"x\", "y"]));
    assert_eq!(split_windows(r#""x\\\"" y"#), w(&[r#"x\""#, "y"]));
    // "" inside quotes is a literal quote.
    assert_eq!(split_windows(r#""a""b""#), w(&[r#"a"b"#]));
    assert_eq!(split_windows("a\nb\r\nc"), w(&["a", "b", "c"]));
    assert_eq!(split_windows(r#""""#), w(&[""]));
}

#[test]
fn lib_expands_only_the_command_lines_response_files() {
    let mut read = files(&[
        ("outer.rsp", "/OUT:x.lib @inner.rsp"),
        ("inner.rsp", "b.obj"),
    ]);
    let got = expand(Flavor::Lib, &w(&["a.obj", "@outer.rsp"]), None, &mut read).unwrap();
    // Measured: lib.exe looks for an input file named @inner.rsp.
    assert_eq!(got, w(&["a.obj", "/OUT:x.lib", "@inner.rsp"]));
}

#[test]
fn lib_reads_lib_env_last() {
    let mut read = files(&[]);
    let got = expand(
        Flavor::Lib,
        &w(&["/OUT:cmd.lib", "a.obj"]),
        Some("/OUT:env.lib /VERBOSE"),
        &mut read,
    )
    .unwrap();
    assert_eq!(
        got,
        w(&["/OUT:cmd.lib", "a.obj", "/OUT:env.lib", "/VERBOSE"])
    );
}

#[test]
fn llvm_lib_expands_nested_response_files_and_ignores_env() {
    let mut read = files(&[
        ("outer.rsp", r#"/out:"x y.lib" @inner.rsp"#),
        ("inner.rsp", "b.obj"),
    ]);
    let got = expand(
        Flavor::LlvmLib,
        &w(&["@outer.rsp", "a.obj"]),
        Some("/verbose"),
        &mut read,
    )
    .unwrap();
    assert_eq!(got, w(&["/out:x y.lib", "b.obj", "a.obj"]));
}

#[test]
fn llvm_lib_recursion_is_an_error_and_missing_files_stay_words() {
    let mut read = files(&[("loop.rsp", "x @inner.rsp"), ("inner.rsp", "@LOOP.rsp")]);
    let err = expand(Flavor::LlvmLib, &w(&["@loop.rsp"]), None, &mut read).unwrap_err();
    assert_eq!(err.path, "LOOP.rsp");
    let mut read = files(&[]);
    let got = expand(Flavor::LlvmLib, &w(&["@nope.rsp", "a"]), None, &mut read).unwrap();
    assert_eq!(got, w(&["@nope.rsp", "a"]));
}

#[test]
fn missing_response_file() {
    let mut read = files(&[]);
    let err = expand(Flavor::Lib, &w(&["@nope.rsp"]), None, &mut read).unwrap_err();
    assert_eq!(err.path, "nope.rsp");
}

#[test]
fn decoding() {
    assert_eq!(decode(b"\xEF\xBB\xBFa b"), "a b");
    let le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("a é".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(decode(&le), "a é");
    let be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain("z".encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    assert_eq!(decode(&be), "z");
    assert_eq!(decode(b"plain"), "plain");
}
