//! A lit test, as far as lld's COFF tests need: its RUN lines (with `\`
//! continuations), and the REQUIRES, UNSUPPORTED and XFAIL lines.

/// One test file.
#[derive(Clone, Debug, Default)]
pub struct Test {
    /// Its file name (`arm64-thunks.s`).
    pub name: String,
    /// Its RUN lines' commands, continuations joined.
    pub runs: Vec<String>,
    /// What REQUIRES names (features, as written: `x86`, `aarch64 || arm`).
    pub requires: Vec<String>,
    /// What UNSUPPORTED names.
    pub unsupported: Vec<String>,
    /// What XFAIL names.
    pub xfail: Vec<String>,
}

/// The text after `key:` on `line`, if `line` has it (lit finds keywords
/// anywhere in a line: after `#`, `;`, `//` or nothing).
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(key)?;
    // A word of its own (not `XRUN:` or `CHECK-RUN:`).
    let before = line[..at].chars().next_back();
    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(&line[at + key.len()..])
}

/// Parses `text`, the test file `name`.
pub fn parse(name: &str, text: &str) -> Test {
    let mut t = Test {
        name: name.to_owned(),
        ..Test::default()
    };
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = after(line, "RUN:") {
            let rest = rest.trim();
            let (body, more) = match rest.strip_suffix('\\') {
                Some(b) => (b.trim_end(), true),
                None => (rest, false),
            };
            let joined = match pending.take() {
                Some(mut p) => {
                    p.push(' ');
                    p.push_str(body);
                    p
                }
                None => body.to_owned(),
            };
            if more {
                pending = Some(joined);
            } else if !joined.is_empty() {
                t.runs.push(joined);
            }
            continue;
        }
        for (key, list) in [
            ("REQUIRES:", &mut t.requires),
            ("UNSUPPORTED:", &mut t.unsupported),
            ("XFAIL:", &mut t.xfail),
        ] {
            if let Some(rest) = after(line, key) {
                list.extend(
                    rest.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    if let Some(p) = pending {
        t.runs.push(p);
    }
    t
}

/// Whether feature expression `e` (`x86`, `!asan`, `aarch64 || arm`,
/// `x86 && windows`) holds when `has` says which features there are.
pub fn holds(e: &str, has: &dyn Fn(&str) -> bool) -> bool {
    e.split("||").any(|alt| {
        alt.split("&&").all(|f| {
            let f = f.trim().trim_matches(|c| c == '(' || c == ')');
            match f.strip_prefix('!') {
                Some(neg) => !has(neg.trim()),
                None => has(f),
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lines_and_their_continuations() {
        let t = parse(
            "x.s",
            "# REQUIRES: x86, aarch64 || arm\n\
             # RUN: llvm-mc -filetype=obj %s -o %t.obj\n\
             # RUN: lld-link /out:%t.exe \\\n\
             # RUN:   /entry:main %t.obj\n\
             # CHECK-RUN: not a run line\n\
             # XFAIL: *\n",
        );
        assert_eq!(
            t.runs,
            [
                "llvm-mc -filetype=obj %s -o %t.obj",
                "lld-link /out:%t.exe /entry:main %t.obj"
            ]
        );
        assert_eq!(t.requires, ["x86", "aarch64 || arm"]);
        assert_eq!(t.xfail, ["*"]);
    }

    #[test]
    fn feature_expressions() {
        let has = |f: &str| f == "x86" || f == "windows";
        assert!(holds("x86", &has));
        assert!(!holds("aarch64", &has));
        assert!(holds("aarch64 || x86", &has));
        assert!(holds("x86 && windows", &has));
        assert!(!holds("x86 && !windows", &has));
    }
}
