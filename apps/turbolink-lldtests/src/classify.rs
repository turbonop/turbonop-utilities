//! What a test's result says about turbolink: it passes; it's skipped
//! (another machine, LTO, a tool or feature this setup lacks); it fails
//! only on lld's own wording or an lld-only option; turbolink rejects what
//! it should take (or crashes); turbolink takes what lld rejects; or the
//! image differs from what the test checks.

use crate::lit::{Test, holds};
use crate::run::Outcome;

/// A test's class, with its cause where there is one (a short, normalized
/// text: tests of one cause group together).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Every RUN line succeeded.
    Passes,
    /// Not run here: why.
    Skipped(String),
    /// Expected to fail (XFAIL), and did.
    ExpectedFailure,
    /// Fails only on lld's own text (its messages' wording, its output),
    /// turbolink having done the same (failed where lld fails, succeeded
    /// where it succeeds).
    LldWording(String),
    /// Uses an option only lld has.
    LldOnly(String),
    /// turbolink rejects an option or input (it should take it), or
    /// crashes.
    Rejects(String),
    /// turbolink succeeds where lld fails (the test expects an error).
    Accepts(String),
    /// The image's checked contents differ.
    Differs(String),
    /// A tool other than turbolink failed (the test's setup).
    Harness(String),
    /// Failed where link.exe and lld differ (checked with link.exe;
    /// turbolink does what link.exe does): how.
    LinkExe(String),
}

impl Class {
    /// The class's name, for the report.
    pub fn name(&self) -> &'static str {
        match self {
            Class::Passes => "passes",
            Class::Skipped(_) => "skipped",
            Class::ExpectedFailure => "expected failure",
            Class::LldWording(_) => "lld's wording only",
            Class::LldOnly(_) => "lld-only option",
            Class::Rejects(_) => "turbolink rejects",
            Class::Accepts(_) => "turbolink accepts",
            Class::Differs(_) => "image differs",
            Class::Harness(_) => "harness",
            Class::LinkExe(_) => "link.exe differs from lld",
        }
    }

    /// Its cause ("" if none).
    pub fn cause(&self) -> &str {
        match self {
            Class::Passes | Class::ExpectedFailure => "",
            Class::Skipped(c)
            | Class::LldWording(c)
            | Class::LldOnly(c)
            | Class::Rejects(c)
            | Class::Accepts(c)
            | Class::Differs(c)
            | Class::Harness(c)
            | Class::LinkExe(c) => c,
        }
    }
}

/// The features this setup has (lit's `REQUIRES`): x86 only, for now.
fn has(feature: &str) -> bool {
    matches!(feature, "x86" | "system-windows" | "non-root-user")
}

/// Options only lld-link has (not link.exe's).
const LLD_ONLY: &[&str] = &[
    "lldmap",
    "lldltocache",
    "lldltocachepolicy",
    "lldsavetemps",
    "lldemit",
    "lldmingw",
    "lldignoreenv",
    "mllvm",
    "lto-",
    "thinlto",
    "opt:lldlto",
    "opt:lldltojobs",
    "opt:lldltopartitions",
    "dwodir",
    "includeoptional",
    "time-trace",
    "reproduce",
    "wrap",
    "vfsoverlay",
    "start-lib",
    "end-lib",
    "call-graph-ordering-file",
    "call-graph-profile-sort",
    "print-symbol-order",
    "auto-import",
    "runtime-pseudo-reloc",
    "exclude-symbols",
    "exclude-all-symbols",
    "export-all-symbols",
    "kill-at",
    "output-def",
    "build-id",
    "threads",
    "demangle",
    "color-diagnostics",
    "rsp-quoting",
    "pdbstripped",
    "pdbsourcepath",
    "noseh",
    "lld-allow-duplicate-weak",
    "inferasanlibs",
    "arm64xsameaddress",
    "prefetch-inputs",
    "-version",
    "version",
    // (link.exe ignores these, LNK4044.)
    "timestamp",
    "summary",
    // (Values link.exe refuses, LNK1117.)
    "debug:symtab",
    "debug:dwarf",
    "debug:ghash",
    "debug:noghash",
    "subsystem:default",
];

/// What the inputs test `t` names from `Inputs/` say of their machine:
/// a YAML input's text, `i386` for an x86 object.
pub fn inputs_text(t: &Test, tests: &std::path::Path) -> String {
    let mut out = String::new();
    for r in &t.runs {
        for w in r.split_whitespace() {
            let Some(at) = w.find("Inputs/") else {
                continue;
            };
            let name: String = w[at..]
                .chars()
                .take_while(|c| !matches!(c, '"' | '\'' | ')' | ','))
                .collect();
            let path = tests.join(&name);
            if name.ends_with(".yaml") {
                out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
                out.push('\n');
            } else if name.ends_with(".obj")
                && std::fs::read(&path).is_ok_and(|b| b.get(..2) == Some(&[0x4c, 0x01][..]))
            {
                out.push_str("i386\n");
            }
        }
    }
    out
}

/// Whether test `t` (its text `text`) runs here, and if not, why.
pub fn skip(t: &Test, text: &str) -> Option<String> {
    for r in &t.requires {
        if !holds(r, &has) {
            return Some(format!("requires {r}"));
        }
    }
    if t.unsupported.iter().any(|u| holds(u, &has)) {
        return Some("unsupported here".into());
    }
    let runs = t.runs.join("\n");
    if runs.contains("llvm-as")
        || runs.contains("opt ")
        || t.name.ends_with(".ll") && !runs.contains("llc")
    {
        return Some("LTO (bitcode)".into());
    }
    if runs.contains("ld.lld") || runs.contains("-flavor") || runs.contains("wasm-ld") {
        return Some("another lld flavor".into());
    }
    // Other machines than x64 (a later step).
    let lower = text.to_ascii_lowercase();
    for (marker, machine) in [
        ("aarch64", "ARM64"),
        ("arm64ec", "ARM64EC"),
        ("armnt", "ARM"),
        ("thumb", "ARM"),
        ("arm64", "ARM64"),
        ("thumbv7", "ARM"),
        ("armv7", "ARM"),
        ("machine:arm", "ARM"),
        ("image_file_machine_arm", "ARM"),
        ("i686", "x86 (32-bit)"),
        ("i386", "x86 (32-bit)"),
        ("machine:x86", "x86 (32-bit)"),
        ("mips", "MIPS"),
    ] {
        if lower.contains(marker) {
            return Some(format!("another machine: {machine}"));
        }
    }
    None
}

/// The first line of turbolink's diagnostics in `stderr` that is an error
/// (or a crash), normalized: paths and numbers in it kept out.
fn turbolink_error(stderr: &str) -> Option<String> {
    for l in stderr.lines() {
        if l.contains("panicked at") || l.contains("STATUS_ACCESS_VIOLATION") {
            return Some("crash".into());
        }
        if let Some(rest) = l.strip_prefix("turbolink : ")
            && (rest.contains("error") || rest.contains("fatal"))
        {
            // (An option not supported yet keeps its name: the cause.)
            if rest.contains("is not supported yet") {
                return Some(rest.trim().to_owned());
            }
            return Some(normalize(rest));
        }
    }
    None
}

/// An unrecognized option turbolink warned about, if any (`LNK4044`).
fn unrecognized(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|l| {
        let at = l.find("unrecognized option '")?;
        let rest = &l[at + "unrecognized option '".len()..];
        Some(rest.split('\'').next().unwrap_or("").to_owned())
    })
}

/// `s` with what differs from test to test taken out: quoted names and
/// paths (`'...'`), and what follows `: ` in a file's name.
fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '\'' | '"' => {
                if !quoted {
                    out.push_str("'…'");
                }
                quoted = !quoted;
            }
            _ if quoted => {}
            _ => out.push(c),
        }
    }
    // A file's name before the message (`x.obj: unresolved ...`).
    let out = match out.find(".obj: ").or_else(|| out.find(".lib: ")) {
        Some(p) => {
            let start = out[..p].rfind(' ').map_or(0, |i| i + 1);
            format!("{}{}", &out[..start], &out[p + 6..])
        }
        None => out,
    };
    // Symbol names after "symbol ".
    match out.find("symbol ") {
        Some(p) => format!("{}symbol …", &out[..p]),
        None => out,
    }
}

/// The option on command line `line` that is lld-only, if any.
fn lld_only(line: &str) -> Option<String> {
    for word in line.split_whitespace() {
        let w = word.trim_start_matches(['/', '-']).to_ascii_lowercase();
        if !word.starts_with(['/', '-']) {
            continue;
        }
        if let Some(o) = LLD_ONLY.iter().find(|o| {
            w == **o || w.starts_with(&format!("{o}:")) || o.ends_with('-') && w.starts_with(**o)
        }) {
            return Some(format!("/{o}"));
        }
    }
    None
}

/// Tests where link.exe does otherwise than lld, checked with link.exe
/// 14.52 on the tests' own inputs (turbolink follows link.exe): how.
const LINK_EXE_DIFFERS: &[(&str, &str)] = &[
    (
        "crt-chars.test",
        "a read-only and a writable .CRT stay two sections (LNK4078)",
    ),
    (
        "output-chars.test",
        "same-named sections of other attributes stay apart; /MERGE moves only the first",
    ),
    (
        "export-limit.s",
        "LNK1189 from 65,533 exports (the import library's objects)",
    ),
    ("invalid-obj.test", "LNK1328: missing string table"),
    (
        "wholearchive-implib.s",
        "one import directory entry (the library's descriptor objects)",
    ),
    ("cfguard-weak-undef.s", "link.exe 14.52 crashes on it"),
    (
        "invalid-debug-type.test",
        "an unknown /DEBUGTYPE is LNK1117, not a warning",
    ),
    (
        "implib-name.test",
        "no import library from /DEF without objects (that's lib /DEF)",
    ),
    (
        "lib.test",
        "no import library from /DEF without objects (that's lib /DEF)",
    ),
    (
        "strtab.s",
        "no import library from /DEF without objects (that's lib /DEF)",
    ),
];

/// The class of test `t`'s outcome `o`.
pub fn classify(t: &Test, o: &Outcome) -> Class {
    if let Outcome::Failed { .. } = o
        && let Some((_, how)) = LINK_EXE_DIFFERS.iter().find(|(n, _)| *n == t.name)
    {
        return Class::LinkExe((*how).to_owned());
    }
    let expected = !t.xfail.is_empty();
    let Outcome::Failed {
        line,
        stderr,
        stdout,
        code,
        ..
    } = o
    else {
        return if expected {
            Class::Rejects("XFAIL test passes".into())
        } else {
            Class::Passes
        };
    };
    if expected {
        return Class::ExpectedFailure;
    }
    if code.is_none() {
        return Class::Rejects("timed out".into());
    }
    let uses_us = line.contains("lld-link");
    if uses_us && let Some(opt) = lld_only(line) {
        return Class::LldOnly(opt);
    }
    if uses_us {
        if let Some(e) = turbolink_error(stderr) {
            let negated = line.trim_start().starts_with("not ")
                || line.contains("| not ")
                || line.contains("&& not ")
                || line.contains("env ") && line.contains(" not ");
            if e == "crash" {
                return Class::Rejects("crash".into());
            }
            if negated && !e.contains("not supported yet") {
                return Class::LldWording(format!("error: {e}"));
            }
            if let Some(o) = unrecognized(stderr) {
                return Class::Rejects(format!("unrecognized option {o}"));
            }
            return Class::Rejects(e);
        }
        if let Some(o) = unrecognized(stderr) {
            return Class::Rejects(format!("unrecognized option {o}"));
        }
        let negated = line.trim_start().starts_with("not ")
            || line.contains("env ") && line.contains(" not ");
        if negated {
            return Class::Accepts(String::new());
        }
        // turbolink succeeded; what checked its output (its messages) didn't
        // match.
        if line.contains("FileCheck") {
            return Class::LldWording("output or warning text".into());
        }
        return Class::Harness(format!("after lld-link: {}", first(stderr, stdout)));
    }
    if line.contains("FileCheck") && !dumps(line) {
        // FileCheck of a file: lld-link's own output (its messages,
        // /verbose), if an earlier line wrote it.
        if let Outcome::Failed { index, .. } = o
            && let Some(input) = filecheck_input(&t.runs[*index])
            && t.runs[..*index]
                .iter()
                .any(|r| r.contains("lld-link") && writes(r, &input))
        {
            return Class::LldWording("lld-link's output, checked from a file".into());
        }
    }
    if line.contains("FileCheck") {
        let tool = line.split_whitespace().next().unwrap_or("").to_owned();
        if let Some(l) = stderr
            .lines()
            .find(|l| l.contains(": error:") && !l.contains("FileCheck"))
        {
            // (The message, not the file and line it was in.)
            let message = l.split("error: ").nth(1).unwrap_or(l);
            return Class::Differs(format!("{tool} failed: {}", normalize(message)));
        }
        return Class::Differs(format!("{tool} | FileCheck"));
    }
    let tool = line.split_whitespace().next().unwrap_or("").to_owned();
    Class::Harness(format!("{tool}: {}", first(stderr, stdout)))
}

/// Whether pipeline `line` feeds FileCheck from a tool that dumps an image
/// (so it checks the image, not lld's messages).
fn dumps(line: &str) -> bool {
    [
        "llvm-readobj",
        "llvm-readelf",
        "llvm-objdump",
        "llvm-nm",
        "llvm-pdbutil",
        "llvm-dwarfdump",
        "obj2yaml",
        "llvm-ar",
        "llvm-lib",
        "llvm-cvtres",
    ]
    .iter()
    .any(|t| line.contains(t))
}

/// The file FileCheck reads in (unsubstituted) RUN line `run`: after `<`
/// or `--input-file`.
fn filecheck_input(run: &str) -> Option<String> {
    let words: Vec<&str> = run.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if *w == "<" || *w == "--input-file" || *w == "-input-file" {
            return words.get(i + 1).map(|s| (*s).to_owned());
        }
        if let Some(f) = w.strip_prefix('<').filter(|f| !f.is_empty()) {
            return Some(f.to_owned());
        }
        if let Some(f) = w.strip_prefix("--input-file=") {
            return Some(f.to_owned());
        }
    }
    None
}

/// Whether (unsubstituted) RUN line `run` writes `file` (`> file`, `2>
/// file`, `&> file`).
fn writes(run: &str, file: &str) -> bool {
    let words: Vec<&str> = run.split_whitespace().collect();
    words.iter().enumerate().any(|(i, w)| {
        let redirect = w.ends_with('>') && w.len() <= 3;
        (redirect && words.get(i + 1) == Some(&file))
            || [">", "2>", "&>", "1>"]
                .iter()
                .any(|r| w.strip_prefix(r) == Some(file))
    })
}

/// The first line of `a`, else of `b`, normalized.
fn first(a: &str, b: &str) -> String {
    let l = a
        .lines()
        .chain(b.lines())
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let l: String = l.chars().take(120).collect();
    normalize(&l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(line: &str, stderr: &str) -> Outcome {
        Outcome::Failed {
            index: 0,
            line: line.into(),
            code: Some(1),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn classes() {
        let t = Test::default();
        assert_eq!(classify(&t, &Outcome::Passed), Class::Passes);
        // An error both expect, worded otherwise.
        assert_eq!(
            classify(
                &t,
                &failed(
                    "not lld-link /out:x.exe x.obj 2>&1 | FileCheck %s",
                    "turbolink : error LNK2001: x.obj: unresolved external symbol foo\n"
                )
            ),
            Class::LldWording("error: error LNK2001: unresolved external symbol …".into())
        );
        // An option turbolink doesn't take yet.
        assert_eq!(
            classify(
                &t,
                &failed(
                    "lld-link /out:x.exe /order:@x x.obj",
                    "turbolink : error: '/order' is not supported yet\n"
                )
            ),
            Class::Rejects("error: '/order' is not supported yet".into())
        );
        // lld-only.
        assert_eq!(
            classify(&t, &failed("lld-link /lldmap:x.map x.obj", "")),
            Class::LldOnly("/lldmap".into())
        );
        // What lld rejects, turbolink takes.
        assert_eq!(
            classify(&t, &failed("not lld-link x.obj", "")),
            Class::Accepts(String::new())
        );
        // The image checked.
        assert_eq!(
            classify(
                &t,
                &failed("llvm-readobj --file-headers x.exe | FileCheck %s", "")
            ),
            Class::Differs("llvm-readobj | FileCheck".into())
        );
    }

    #[test]
    fn skipped() {
        let mut t = Test {
            requires: vec!["aarch64".into()],
            ..Test::default()
        };
        assert_eq!(skip(&t, ""), Some("requires aarch64".into()));
        t.requires = vec!["x86".into()];
        assert_eq!(
            skip(&t, "--triple=i686-windows"),
            Some("another machine: x86 (32-bit)".into())
        );
        assert_eq!(skip(&t, "--triple=x86_64-windows"), None);
    }
}
