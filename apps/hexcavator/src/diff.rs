//! `hexcavator diff`: what differs between two images, or two PDBs, and
//! why: a linker's output against another's (turbolink's against
//! link.exe's), down to the byte.
//!
//! For images: whether they are identical; each range of bytes that
//! differs, attributed to what holds it (a header's field, a section
//! table's entry, a data directory, a function or datum); then the
//! structures compared one by one (headers, sections, imports, exports,
//! base relocations, exception data, the load configuration, TLS,
//! resources, the debug directory, the Rich header) and the functions and
//! data by name: which are identical, which differ only where they refer
//! to things that moved, which really differ, which are folded
//! differently, which one image lacks.
//!
//! For PDBs: whether they say the same, not whether their bytes are:
//! types and ids compared as records whose indices name records (so a
//! renumbering is no difference), modules, section contributions and the
//! section map, each module's symbols (their offsets, which only say
//! where they are, aside), lines, checksums, and inlinee lines, the
//! globals, the publics, and the string table.

mod image;
mod pdb;
mod symbols;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

pub const USAGE: &str = "\
usage: hexcavator diff [options] <a> <b>

What differs between two images, or two PDBs (by their contents: given an
image and a PDB, each is compared with its kind). For images: the bytes
that differ and what holds them, then each structure, then the functions
and data by name. For PDBs: what they say, not their bytes (types compared
as graphs, so that renumbered records are the same; symbols without their
offsets).

options:
  -debug <path>   a's debug information (its PDB), for names (default: found
                  as every command finds it)
  -debug2 <path>  b's
  -map <path>     a's linker map (link.exe's /MAP, or turbolink's), for names
                  when there is no PDB
  -map2 <path>    b's
  -n <n>          differences listed in each part (default 20; 0: all)
  -only <parts>   only these parts (comma-separated: bytes, headers,
                  sections, directories, imports, exports, relocations,
                  exceptions, loadconfig, tls, resources, debug, rich,
                  functions; for PDBs: info, types, ids, modules,
                  contributions, sectionmap, symbols, lines, globals, publics,
                  names)
  -json           as JSON

Exits with 0 if they are the same, 1 if not, 2 for an error.";

/// What `diff` was asked.
pub(crate) struct Options {
    pub a: PathBuf,
    pub b: PathBuf,
    pub debug: [Option<PathBuf>; 2],
    pub map: [Option<PathBuf>; 2],
    pub rows: usize,
    pub only: Option<Vec<String>>,
    pub json: bool,
}

impl Options {
    /// Whether part `name` is to be compared.
    pub fn wants(&self, name: &str) -> bool {
        self.only
            .as_ref()
            .is_none_or(|o| o.iter().any(|p| p == name))
    }
}

/// One part of the comparison: its differences, the first few kept.
pub(crate) struct Part {
    pub key: &'static str,
    pub title: String,
    pub differences: usize,
    pub lines: Vec<String>,
    /// A line said whatever the differences (counts compared).
    pub summary: Vec<String>,
    limit: usize,
}

impl Part {
    pub fn new(key: &'static str, title: impl Into<String>, limit: usize) -> Part {
        Part {
            key,
            title: title.into(),
            differences: 0,
            lines: Vec::new(),
            summary: Vec::new(),
            limit,
        }
    }

    /// A difference.
    pub fn add(&mut self, line: impl Into<String>) {
        self.differences += 1;
        if self.lines.len() < self.limit {
            self.lines.push(line.into());
        }
    }

    /// A difference, its line made only if it is kept.
    pub fn add_with(&mut self, line: impl FnOnce() -> String) {
        self.differences += 1;
        if self.lines.len() < self.limit {
            self.lines.push(line());
        }
    }

    pub fn note(&mut self, line: impl Into<String>) {
        self.summary.push(line.into());
    }

    /// Compares two values of a named field.
    pub fn field<T: PartialEq + std::fmt::Display>(&mut self, name: &str, a: T, b: T) {
        if a != b {
            self.add(format!("{name}: {a} | {b}"));
        }
    }
}

/// The whole comparison.
pub(crate) struct Report {
    pub kind: &'static str,
    pub parts: Vec<Part>,
}

impl Report {
    pub fn identical(&self) -> bool {
        self.parts.iter().all(|p| p.differences == 0)
    }

    fn text(&self, o: &Options) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "a: {}", o.a.display());
        let _ = writeln!(s, "b: {}", o.b.display());
        if self.identical() {
            let _ = writeln!(
                s,
                "{}: the same",
                if self.kind == "pdb" {
                    "PDBs (logically)"
                } else {
                    "images"
                }
            );
        }
        for p in &self.parts {
            let _ = writeln!(s);
            if p.differences == 0 {
                let _ = writeln!(s, "{}: the same", p.title);
            } else {
                let _ = writeln!(
                    s,
                    "{}: {} difference{}",
                    p.title,
                    p.differences,
                    if p.differences == 1 { "" } else { "s" }
                );
            }
            for l in &p.summary {
                let _ = writeln!(s, "  {l}");
            }
            for l in &p.lines {
                let _ = writeln!(s, "  {l}");
            }
            if p.differences > p.lines.len() {
                let _ = writeln!(s, "  ... and {} more", p.differences - p.lines.len());
            }
        }
        s
    }

    fn json(&self, o: &Options) -> Value {
        json!({
            "a": o.a.display().to_string(),
            "b": o.b.display().to_string(),
            "kind": self.kind,
            "identical": self.identical(),
            "parts": self.parts.iter().map(|p| json!({
                "part": p.key,
                "title": p.title,
                "differences": p.differences,
                "summary": p.summary,
                "lines": p.lines,
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let mut paths = Vec::new();
    let mut o = Options {
        a: PathBuf::new(),
        b: PathBuf::new(),
        debug: [None, None],
        map: [None, None],
        rows: 20,
        only: None,
        json: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut path = |slot: &mut Option<PathBuf>, name: &str| -> Result<(), ExitCode> {
            match it.next() {
                Some(p) => {
                    *slot = Some(PathBuf::from(p));
                    Ok(())
                }
                None => Err(usage(&format!("{name} needs a path"))),
            }
        };
        let r = match arg.as_str() {
            "-debug" | "/debug" | "-pdb" | "/pdb" => path(&mut o.debug[0], arg),
            "-debug2" | "/debug2" | "-pdb2" | "/pdb2" => path(&mut o.debug[1], arg),
            "-map" | "/map" => path(&mut o.map[0], arg),
            "-map2" | "/map2" => path(&mut o.map[1], arg),
            "-n" | "/n" => match it.next().and_then(|n| n.parse::<usize>().ok()) {
                Some(0) => {
                    o.rows = usize::MAX;
                    Ok(())
                }
                Some(n) => {
                    o.rows = n;
                    Ok(())
                }
                None => Err(usage("-n needs a number")),
            },
            "-only" | "/only" => match it.next() {
                Some(v) => {
                    o.only = Some(
                        v.split(',')
                            .map(|s| s.trim().to_ascii_lowercase())
                            .collect(),
                    );
                    Ok(())
                }
                None => Err(usage("-only needs a list of parts")),
            },
            "-json" | "/json" => {
                o.json = true;
                Ok(())
            }
            s if s.starts_with('-') && s.len() > 1 => Err(usage(&format!("unknown option '{s}'"))),
            s => {
                paths.push(PathBuf::from(s));
                Ok(())
            }
        };
        if let Err(code) = r {
            return code;
        }
    }
    let [a, b] = <[PathBuf; 2]>::try_from(paths).unwrap_or_default();
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return usage("two files to compare are needed");
    }
    o.a = a;
    o.b = b;
    match compare(&o) {
        Ok(report) => {
            let text = if o.json {
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&report.json(&o)).unwrap_or_default()
                )
            } else {
                report.text(&o)
            };
            let code = crate::out::print(text.as_bytes());
            if code != ExitCode::SUCCESS {
                return code;
            }
            if report.identical() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            ereport!("hexcavator diff: {e}");
            ExitCode::from(2)
        }
    }
}

fn usage(message: &str) -> ExitCode {
    ereport!("hexcavator diff: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

/// Whether `data` is a PDB (an MSF 7.00 file).
pub(crate) fn is_pdb(data: &[u8]) -> bool {
    data.starts_with(b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0")
}

fn open(path: &Path) -> Result<turbonop_mmap::Mmap, String> {
    turbonop_mmap::Mmap::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Compares `o.a` and `o.b`.
pub(crate) fn compare(o: &Options) -> Result<Report, String> {
    let a = open(&o.a)?;
    let b = open(&o.b)?;
    match (is_pdb(&a), is_pdb(&b)) {
        (true, true) => pdb::compare(&a, &b, o),
        (false, false) => image::compare(&a, &b, o),
        _ => Err("one is a PDB and the other isn't: compare like with like".into()),
    }
}

#[cfg(test)]
mod tests;
