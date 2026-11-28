//! `ar`'s, `llvm-ar`'s, and their `ranlib`s' command lines, parsed as each
//! tool parses them (GNU binutils 2.46's `ar.c`, LLVM 24's `llvm-ar.cpp`).
//!
//! GNU `ar` explodes a first word without `-` into one option per letter
//! (`rcs` is `-r -c -s`), then reads options anywhere among the other
//! words (as `getopt` permutes them) until `--`. `llvm-ar` takes the first
//! word without `-` as its letters and every `-` word's letters after it,
//! in order, and reads `a`, `b`, `i`, and `N`'s arguments from the
//! positional words when it meets those letters. Both then take
//! `[relpos] [count] archive [files...]`.
//!
//! turbolib's own options (see `crate::settings`) are taken as `--<option>`
//! words (`--fold=identical`, `--stats`) and `-j N`, by every flavor.

/// Which tool's command line, and which of its programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// GNU `ar`.
    GnuAr,
    /// GNU `ranlib`.
    GnuRanlib,
    /// `llvm-ar`.
    LlvmAr,
    /// `llvm-ranlib`.
    LlvmRanlib,
}

impl Flavor {
    /// Whether this is one of LLVM's.
    pub fn is_llvm(self) -> bool {
        matches!(self, Flavor::LlvmAr | Flavor::LlvmRanlib)
    }

    /// Whether this is a `ranlib`.
    pub fn is_ranlib(self) -> bool {
        matches!(self, Flavor::GnuRanlib | Flavor::LlvmRanlib)
    }
}

/// The flavor a program name selects: `ar` and `*-ar`
/// (`x86_64-linux-gnu-ar`), `ranlib` and `*-ranlib`, and `llvm-ar` and
/// `llvm-ranlib` (with a version, `llvm-ar-21`), in any case, with or
/// without `.exe`.
pub fn flavor(argv0: &str) -> Option<Flavor> {
    let file = argv0.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    // llvm-ar's rule: the name, then the stem's end or a non-alphanumeric.
    let has = |tool: &str| {
        stem.rfind(tool).is_some_and(|i| {
            stem[i + tool.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric())
        })
    };
    if has("llvm-ranlib") {
        Some(Flavor::LlvmRanlib)
    } else if has("llvm-ar") {
        Some(Flavor::LlvmAr)
    } else if stem == "ranlib" || stem.ends_with("-ranlib") {
        Some(Flavor::GnuRanlib)
    } else if stem == "ar" || stem.ends_with("-ar") {
        Some(Flavor::GnuAr)
    } else {
        None
    }
}

/// The operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `d`.
    Delete,
    /// `m`.
    Move,
    /// `p`.
    Print,
    /// `q`.
    QuickAppend,
    /// `r`.
    Replace,
    /// `t`.
    Table,
    /// `x`.
    Extract,
    /// `s` alone, or `ranlib`: (re)build the symbol table.
    SymbolTable,
}

/// Where `a`, `b`, and `i` put members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Position {
    /// After the member named.
    After(String),
    /// Before it.
    Before(String),
}

/// A parsed command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub op: Option<Op>,
    /// `c`: no message when the archive is created.
    pub create: bool,
    /// `o`: extracted files keep the members' dates.
    pub preserve_dates: bool,
    /// `O`: `t` shows offsets.
    pub offsets: bool,
    /// `P`: match (and, for GNU, store) full paths.
    pub full_path: bool,
    /// `s` (`Some(true)`) or `S` (`Some(false)`).
    pub symtab: Option<bool>,
    /// `u`: replace only with newer files.
    pub update: bool,
    /// `v`.
    pub verbose: bool,
    pub position: Option<Position>,
    /// `N`'s count.
    pub count: Option<u32>,
    /// `f` (GNU): truncate names.
    pub truncate: bool,
    /// `T` or `--thin`.
    pub thin: bool,
    /// `D` (`Some(true)`) or `U` (`Some(false)`).
    pub deterministic: Option<bool>,
    /// `L` (llvm-ar): `q` adds archives' members.
    pub add_library: bool,
    /// `-M`: an MRI script on standard input.
    pub mri: bool,
    /// `--format` (llvm-ar).
    pub format: Option<String>,
    /// `--output` (the directory `x` extracts into).
    pub output: Option<String>,
    /// `l` or `--record-libdeps` (GNU).
    pub libdeps: Option<String>,
    /// `--target` (GNU): accepted.
    pub target: Option<String>,
    /// `ranlib -t` (GNU): update the table's date only.
    pub touch: bool,
    pub help: bool,
    pub version: bool,
    /// The archives (one, but for `ranlib`).
    pub archives: Vec<String>,
    pub files: Vec<String>,
    /// turbolib's options, as `name[=value]`.
    pub turbolib: Vec<String>,
}

/// turbolib's own long options.
const TURBOLIB: &[&str] = &[
    "fold",
    "pick",
    "pull",
    "locals",
    "precomp",
    "debug-types",
    "report",
    "threads",
    "diagnose",
    "stats",
];

/// `--name[=value]` if it is one of turbolib's options.
fn turbolib_option(word: &str) -> Option<&str> {
    let o = word.strip_prefix("--")?;
    let name = o.split('=').next()?;
    TURBOLIB.contains(&name).then_some(o)
}

/// Takes turbolib's options (`--<option>`, `-j N`, `-jN`) out of `words`,
/// before `--`.
pub(super) fn take_turbolib(words: &mut Vec<String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if w == "--" {
            break;
        }
        if let Some(o) = turbolib_option(w) {
            out.push(o.to_string());
            words.remove(i);
        } else if w == "-j" {
            let n = words.get(i + 1).ok_or("-j needs a number of threads")?;
            out.push(format!("threads={n}"));
            words.drain(i..i + 2);
        } else if let Some(n) = w.strip_prefix("-j")
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            out.push(format!("threads={n}"));
            words.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// Applies one operation or modifier letter, GNU's way.
fn gnu_letter(c: &mut Command, letter: char) -> Result<(), String> {
    let op = |c: &mut Command, op| {
        if c.op.is_some() {
            Err("two different operation options specified".to_string())
        } else {
            c.op = Some(op);
            Ok(())
        }
    };
    match letter {
        'd' => op(c, Op::Delete)?,
        'm' => op(c, Op::Move)?,
        'p' => op(c, Op::Print)?,
        'q' => op(c, Op::QuickAppend)?,
        'r' => op(c, Op::Replace)?,
        't' => op(c, Op::Table)?,
        'x' => op(c, Op::Extract)?,
        'h' => c.help = true,
        'c' => c.create = true,
        'o' => c.preserve_dates = true,
        'O' => c.offsets = true,
        'V' => c.version = true,
        's' => c.symtab = Some(true),
        'S' => c.symtab = Some(false),
        'u' => c.update = true,
        'v' => c.verbose = true,
        // (The name comes from the positional words.)
        'a' => c.position = Some(Position::After(String::new())),
        'b' | 'i' => c.position = Some(Position::Before(String::new())),
        'M' => c.mri = true,
        'N' => c.count = Some(0),
        'f' => c.truncate = true,
        'P' => c.full_path = true,
        'T' => c.thin = true,
        'D' => c.deterministic = Some(true),
        'U' => c.deterministic = Some(false),
        // (`l` takes an argument; the caller handles it.)
        other => return Err(format!("invalid option -- '{other}'")),
    }
    Ok(())
}

/// GNU `ar`'s command line.
fn gnu_ar(mut words: Vec<String>) -> Result<Command, String> {
    let mut c = Command {
        turbolib: take_turbolib(&mut words)?,
        ..Command::default()
    };
    let mut exploded = false;
    loop {
        // An old-style first word: each letter an option.
        if !exploded && words.first().is_some_and(|w| !w.starts_with('-')) {
            let first = words.remove(0);
            let mut letters: Vec<String> = first.chars().map(|l| format!("-{l}")).collect();
            letters.append(&mut words);
            words = letters;
            exploded = true;
        }
        let mut positional = Vec::new();
        let mut it = words.into_iter();
        while let Some(w) = it.next() {
            if w == "--" {
                positional.extend(it.by_ref());
                break;
            }
            if let Some(long) = w.strip_prefix("--") {
                let (name, value) = match long.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (long, None),
                };
                let mut arg = || value.clone().or_else(|| it.next());
                match name {
                    "help" => c.help = true,
                    "version" => c.version = true,
                    "thin" => c.thin = true,
                    "plugin" => {
                        arg().ok_or("option '--plugin' requires an argument")?;
                    }
                    "target" => {
                        c.target = Some(arg().ok_or("option '--target' requires an argument")?);
                    }
                    "output" => {
                        c.output = Some(arg().ok_or("option '--output' requires an argument")?);
                    }
                    "record-libdeps" => {
                        let v = arg().ok_or("option '--record-libdeps' requires an argument")?;
                        if c.libdeps.replace(v).is_some() {
                            return Err("libdeps specified more than once".into());
                        }
                    }
                    _ => return Err(format!("unrecognized option '{w}'")),
                }
                continue;
            }
            if w.len() > 1 && w.starts_with('-') {
                let letters: Vec<char> = w[1..].chars().collect();
                let mut k = 0;
                while k < letters.len() {
                    if letters[k] == 'l' {
                        let rest: String = letters[k + 1..].iter().collect();
                        let v = if rest.is_empty() {
                            it.next().ok_or("option requires an argument -- 'l'")?
                        } else {
                            rest
                        };
                        if c.libdeps.replace(v).is_some() {
                            return Err("libdeps specified more than once".into());
                        }
                        break;
                    }
                    gnu_letter(&mut c, letters[k])?;
                    k += 1;
                }
                continue;
            }
            positional.push(w);
        }
        // PR 13256: options first (`--plugin x`), then an old-style word.
        if c.op.is_none() && c.symtab != Some(true) && !c.mri && !exploded && !positional.is_empty()
        {
            words = positional;
            continue;
        }
        let mut positional = positional.into_iter();
        if let Some(p) = &mut c.position {
            let name = positional.next().ok_or("missing position arg.")?;
            match p {
                Position::After(n) | Position::Before(n) => *n = name,
            }
        }
        if c.count.is_some() {
            if !matches!(c.op, Some(Op::Extract | Op::Delete)) {
                return Err("`N' is only meaningful with the `x' and `d' options.".into());
            }
            let n = positional.next().ok_or("`N' missing value.")?;
            // (atoi: leading digits.)
            let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
            let n: u32 = digits.parse().unwrap_or(0);
            if n == 0 {
                return Err("Value for `N' must be positive.".into());
            }
            c.count = Some(n);
        }
        c.archives.extend(positional.next());
        c.files.extend(positional);
        // `s` alone on an archive (or with `t`) is ranlib.
        if matches!(c.op, None | Some(Op::Table)) && c.symtab == Some(true) {
            c.op = Some(Op::SymbolTable);
        }
        // `q` with `s` is `r` (GNU ar can't index a quick append).
        if c.op == Some(Op::QuickAppend) && c.symtab == Some(true) {
            c.op = Some(Op::Replace);
        }
        return Ok(c);
    }
}

/// `llvm-ar`'s command line.
fn llvm_ar(mut words: Vec<String>) -> Result<Command, String> {
    let mut c = Command {
        turbolib: take_turbolib(&mut words)?,
        ..Command::default()
    };
    let mut letters = String::new();
    let mut have_letters = false;
    let mut positional: Vec<String> = Vec::new();
    let mut it = words.into_iter();
    while let Some(w) = it.next() {
        if w == "--help" || w == "-h" {
            c.help = true;
            return Ok(c);
        }
        if w == "--version" {
            c.version = true;
            return Ok(c);
        }
        if w == "--" {
            positional.extend(it.by_ref());
            break;
        }
        if !w.starts_with('-') {
            // The first word without `-`, if no `-` word gave letters yet.
            if letters.is_empty() && !have_letters {
                letters.push_str(&w);
                have_letters = true;
            } else {
                positional.push(w);
            }
            continue;
        }
        if w == "-M" {
            c.mri = true;
            continue;
        }
        if w == "--thin" {
            c.thin = true;
            continue;
        }
        let with_arg =
            |name: &str, it: &mut std::vec::IntoIter<String>| -> Result<Option<String>, String> {
                let arg = w.strip_prefix("--").unwrap_or(&w);
                if arg == name {
                    return it
                        .next()
                        .map(Some)
                        .ok_or_else(|| format!("{name} requires an argument"));
                }
                Ok(arg
                    .strip_prefix(name)
                    .and_then(|r| r.strip_prefix('='))
                    .map(str::to_string))
            };
        if let Some(f) = with_arg("format", &mut it)? {
            if !matches!(
                f.as_str(),
                "default" | "gnu" | "darwin" | "bsd" | "bigarchive" | "coff" | "zos"
            ) {
                return Err(format!("Invalid format {f}"));
            }
            c.format = Some(f);
            continue;
        }
        if let Some(o) = with_arg("output", &mut it)? {
            c.output = Some(o);
            continue;
        }
        if with_arg("plugin", &mut it)?.is_some() || with_arg("rsp-quoting", &mut it)?.is_some() {
            continue;
        }
        if w.starts_with("-X") {
            return Err(format!("{w} option not supported on non AIX OS"));
        }
        letters.push_str(&w[1..]);
    }
    if c.mri {
        if !positional.is_empty() || !letters.is_empty() {
            return Err("cannot mix -M and other options".into());
        }
        return Ok(c);
    }
    let mut positional = positional.into_iter();
    let mut operations = 0;
    let mut positions = 0;
    let mut maybe_symtab = false;
    for l in letters.chars() {
        let mut op = |o| {
            operations += 1;
            c.op = Some(o);
        };
        match l {
            'd' => op(Op::Delete),
            'm' => op(Op::Move),
            'p' => op(Op::Print),
            'q' => op(Op::QuickAppend),
            'r' => op(Op::Replace),
            't' => op(Op::Table),
            'x' => op(Op::Extract),
            'c' => c.create = true,
            'l' => {}
            'o' => c.preserve_dates = true,
            'O' => c.offsets = true,
            'P' => c.full_path = true,
            's' => {
                c.symtab = Some(true);
                maybe_symtab = true;
            }
            'S' => c.symtab = Some(false),
            'u' => c.update = true,
            'v' => c.verbose = true,
            'a' | 'b' | 'i' => {
                let name = positional
                    .next()
                    .ok_or("expected [relpos] for 'a', 'b', or 'i' modifier")?;
                c.position = Some(if l == 'a' {
                    Position::After(name)
                } else {
                    Position::Before(name)
                });
                positions += 1;
            }
            'D' => c.deterministic = Some(true),
            'U' => c.deterministic = Some(false),
            'N' => {
                let n = positional
                    .next()
                    .ok_or("expected [count] for 'N' modifier")?;
                let v: i64 = n
                    .parse()
                    .map_err(|_| format!("value for [count] must be numeric, got: {n}"))?;
                if v < 1 {
                    return Err(format!("value for [count] must be positive, got: {n}"));
                }
                c.count = Some(u32::try_from(v).unwrap_or(u32::MAX));
            }
            'T' => c.thin = true,
            'L' => c.add_library = true,
            'V' => {
                c.version = true;
                return Ok(c);
            }
            'h' => {
                c.help = true;
                return Ok(c);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if c.thin {
        c.full_path = true;
    }
    c.archives.push(
        positional
            .next()
            .ok_or("an archive name must be specified")?,
    );
    c.files.extend(positional);
    if operations == 0 && maybe_symtab {
        operations = 1;
        c.op = Some(Op::SymbolTable);
        if !c.files.is_empty() {
            return Err("the 's' operation takes only an archive as argument".into());
        }
    }
    if operations == 0 {
        return Err("you must specify at least one of the operations".into());
    }
    if operations > 1 {
        return Err("only one operation may be specified".into());
    }
    if positions > 1 {
        return Err("you may only specify one of 'a', 'b', and 'i' modifiers".into());
    }
    if c.position.is_some() && !matches!(c.op, Some(Op::Move | Op::Replace)) {
        return Err(
            "the 'a', 'b' and 'i' modifiers can only be specified with the 'm' or 'r' operations"
                .into(),
        );
    }
    if c.count.is_some() && !matches!(c.op, Some(Op::Extract | Op::Delete)) {
        return Err("the 'N' modifier can only be specified with the 'x' or 'd' operations".into());
    }
    if c.preserve_dates && c.op != Some(Op::Extract) {
        return Err("the 'o' modifier is only applicable to the 'x' operation".into());
    }
    if c.update && c.op != Some(Op::Replace) {
        return Err("the 'u' modifier is only applicable to the 'r' operation".into());
    }
    if c.add_library && c.op != Some(Op::QuickAppend) {
        return Err("the 'L' modifier is only applicable to the 'q' operation".into());
    }
    if c.output.is_some() && c.op != Some(Op::Extract) {
        return Err("--output is only applicable to the 'x' operation".into());
    }
    Ok(c)
}

/// GNU `ranlib`'s command line: `-D`, `-U`, `-t`, `-h`, `-v`, `-V`,
/// `--plugin`, then archives.
fn gnu_ranlib(mut words: Vec<String>) -> Result<Command, String> {
    let mut c = Command {
        turbolib: take_turbolib(&mut words)?,
        op: Some(Op::SymbolTable),
        ..Command::default()
    };
    let mut it = words.into_iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "--help" => c.help = true,
            "--version" => c.version = true,
            "--plugin" => {
                it.next().ok_or("option '--plugin' requires an argument")?;
            }
            "--" => {
                c.archives.extend(it.by_ref());
            }
            _ if w.starts_with("--plugin=") => {}
            _ if w.len() > 1 && w.starts_with('-') => {
                for l in w[1..].chars() {
                    match l {
                        'D' => c.deterministic = Some(true),
                        'U' => c.deterministic = Some(false),
                        'h' | 'H' => c.help = true,
                        't' => c.touch = true,
                        'v' | 'V' => c.version = true,
                        // (getopt prints and goes on.)
                        _ => {}
                    }
                }
            }
            _ => c.archives.push(w),
        }
    }
    Ok(c)
}

/// `llvm-ranlib`'s command line: `-D`, `-U`, `-h`, `-V`, then archives.
fn llvm_ranlib(mut words: Vec<String>) -> Result<Command, String> {
    let mut c = Command {
        turbolib: take_turbolib(&mut words)?,
        op: Some(Op::SymbolTable),
        ..Command::default()
    };
    for w in words {
        if w == "--help" || w == "-h" {
            c.help = true;
            return Ok(c);
        }
        if w == "--version" {
            c.version = true;
            return Ok(c);
        }
        if let Some(letters) = w.strip_prefix('-') {
            for l in letters.chars() {
                match l {
                    'D' => c.deterministic = Some(true),
                    'U' => c.deterministic = Some(false),
                    'h' => {
                        c.help = true;
                        return Ok(c);
                    }
                    'V' => {
                        c.version = true;
                        return Ok(c);
                    }
                    'X' => return Err(format!("{w} option not supported on non AIX OS")),
                    _ => return Err(format!("Invalid option: '-{letters}'")),
                }
            }
        } else {
            c.archives.push(w);
        }
    }
    if c.archives.is_empty() {
        return Err("an archive name must be specified".into());
    }
    Ok(c)
}

/// Parses `words` (response files already expanded) as `flavor` does.
pub fn parse(flavor: Flavor, words: Vec<String>) -> Result<Command, String> {
    match flavor {
        Flavor::GnuAr => gnu_ar(words),
        Flavor::LlvmAr => llvm_ar(words),
        Flavor::GnuRanlib => gnu_ranlib(words),
        Flavor::LlvmRanlib => llvm_ranlib(words),
    }
}

/// Expands `@file` words (each file's words, GNU-quoted: whitespace
/// separates, `'`, `"`, and `\` quote), recursively, as both tools do with
/// their default quoting. A file that can't be read leaves its word as it
/// is (GNU's `expandargv`; LLVM's `ExpandResponseFiles` the same).
pub fn expand(
    words: &[String],
    read: &mut dyn FnMut(&str) -> std::io::Result<Vec<u8>>,
) -> Vec<String> {
    fn go(
        words: &[String],
        read: &mut dyn FnMut(&str) -> std::io::Result<Vec<u8>>,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        for w in words {
            if let Some(path) = w.strip_prefix('@')
                && depth < 32
                && let Ok(bytes) = read(path)
            {
                let text = String::from_utf8_lossy(&bytes);
                go(&split_gnu(&text), read, depth + 1, out);
            } else {
                out.push(w.clone());
            }
        }
    }
    let mut out = Vec::new();
    go(words, read, 0, &mut out);
    out
}

/// Splits `text` into words as libiberty's `buildargv` does.
pub fn split_gnu(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (_, '\\') => {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
                in_word = true;
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                in_word = true;
            }
            (Some(q), c) if c == q => quote = None,
            (_, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(word);
    }
    out
}

#[cfg(test)]
mod tests;
