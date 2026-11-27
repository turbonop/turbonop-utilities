//! A DLL's exports: what `/EXPORT`, objects' `/EXPORT` directives, and a
//! `.def` file's `EXPORTS` ask for; their ordinals; and the export
//! directory (`.edata`) that lists them.

use crate::config::{Config, Export};

/// A `.def` file, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefFile {
    /// `LIBRARY name` (or `NAME name`, for an EXE).
    pub name: Option<String>,
    /// Whether it says `LIBRARY` (a DLL).
    pub library: bool,
    /// `LIBRARY name BASE=n`.
    pub base: Option<u64>,
    /// `EXPORTS`.
    pub exports: Vec<Export>,
    /// `HEAPSIZE reserve[,commit]`.
    pub heap: Option<(u64, Option<u64>)>,
    /// `STACKSIZE reserve[,commit]`.
    pub stack: Option<(u64, Option<u64>)>,
    /// `VERSION major[.minor]`.
    pub version: Option<(u16, u16)>,
    /// `SECTIONS`: names and attributes, as `/SECTION` takes them.
    pub sections: Vec<String>,
}

/// Reads a `.def` file's text. Keywords are in any case; `;` starts a
/// comment; names may be quoted.
pub fn parse_def(text: &str) -> Result<DefFile, String> {
    let mut def = DefFile::default();
    // The words, by line (a statement's arguments are on its line, or on
    // the lines after, for EXPORTS and SECTIONS).
    #[derive(PartialEq)]
    enum In {
        Top,
        Exports,
        Sections,
    }
    let mut state = In::Top;
    for raw in text.lines() {
        let line = raw.split(';').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let words = crate::args::split_words(line);
        let Some(first) = words.first() else { continue };
        let keyword = first.to_ascii_uppercase();
        match keyword.as_str() {
            "LIBRARY" | "NAME" => {
                state = In::Top;
                def.library = keyword == "LIBRARY";
                for w in &words[1..] {
                    if let Some(b) = w.to_ascii_uppercase().strip_prefix("BASE=") {
                        def.base = Some(
                            crate::config::parse_number(b)
                                .ok_or_else(|| format!("bad BASE '{w}'"))?,
                        );
                    } else if def.name.is_none() {
                        def.name = Some(w.clone());
                    }
                }
                continue;
            }
            "EXPORTS" => {
                state = In::Exports;
                if words.len() > 1 {
                    def.exports.push(def_export(&words[1..])?);
                }
                continue;
            }
            "SECTIONS" | "SEGMENTS" => {
                state = In::Sections;
                if words.len() > 1 {
                    def.sections.push(words[1..].join(","));
                }
                continue;
            }
            "HEAPSIZE" | "STACKSIZE" => {
                state = In::Top;
                let v = words
                    .get(1)
                    .ok_or_else(|| format!("{keyword} needs a size"))?;
                let mut parts = v.split(',');
                let reserve = crate::config::parse_number(parts.next().unwrap_or_default())
                    .ok_or_else(|| format!("bad {keyword} '{v}'"))?;
                let commit = parts.next().and_then(crate::config::parse_number);
                if keyword == "HEAPSIZE" {
                    def.heap = Some((reserve, commit));
                } else {
                    def.stack = Some((reserve, commit));
                }
                continue;
            }
            "VERSION" => {
                state = In::Top;
                let v = words.get(1).ok_or("VERSION needs a version")?;
                let (a, b) = v.split_once('.').unwrap_or((v, "0"));
                def.version = Some((
                    a.parse().map_err(|_| format!("bad VERSION '{v}'"))?,
                    b.parse().map_err(|_| format!("bad VERSION '{v}'"))?,
                ));
                continue;
            }
            // Ignored by link.exe too (or meaningless for Win32).
            "DESCRIPTION" | "STUB" | "CODE" | "DATA" | "EXETYPE" | "HEAPSIZE_" => {
                state = In::Top;
                continue;
            }
            _ => {}
        }
        match state {
            In::Exports => def.exports.push(def_export(&words)?),
            In::Sections => def.sections.push(words.join(",")),
            In::Top => return Err(format!("unknown statement '{first}'")),
        }
    }
    Ok(def)
}

/// One `EXPORTS` entry: `name[=internal] [@ordinal [NONAME]] [DATA]
/// [PRIVATE] [EXPORTAS name]`.
fn def_export(words: &[String]) -> Result<Export, String> {
    let head = &words[0];
    // `name = internal` may be written with spaces.
    let mut rest = &words[1..];
    let mut spec = head.clone();
    if rest.first().is_some_and(|w| w == "=") && rest.len() > 1 {
        spec = format!("{head}={}", rest[1]);
        rest = &rest[2..];
    } else if let Some(r) = rest.first().and_then(|w| w.strip_prefix('=')) {
        spec = format!("{head}={r}");
        rest = &rest[1..];
    }
    let mut e = Export::parse(&spec)?;
    let mut it = rest.iter();
    while let Some(w) = it.next() {
        if let Some(o) = w.strip_prefix('@') {
            // `@ 5` is allowed too.
            let o = if o.is_empty() {
                it.next().map_or("", String::as_str)
            } else {
                o
            };
            e.ordinal = Some(
                crate::config::parse_number(o)
                    .and_then(|n| u16::try_from(n).ok())
                    .filter(|&n| n != 0)
                    .ok_or_else(|| format!("bad ordinal '{w}'"))?,
            );
            continue;
        }
        match w.to_ascii_uppercase().as_str() {
            "NONAME" => e.noname = true,
            "DATA" | "CONSTANT" => e.data = true,
            "PRIVATE" => e.private = true,
            "EXPORTAS" => {
                e.export_as = Some(it.next().ok_or("EXPORTAS needs a name")?.clone());
            }
            _ => return Err(format!("unknown export attribute '{w}'")),
        }
    }
    Ok(e)
}

/// Applies a `.def` file to the configuration (the command line's options
/// win where both say).
pub fn apply_def(c: &mut Config, def: &DefFile) {
    c.exports.extend(def.exports.iter().cloned());
    if def.library {
        c.dll = true;
    }
    if def.library
        && let Some(n) = &def.name
    {
        let mut p = std::path::PathBuf::from(n);
        if p.extension().is_none() {
            p.set_extension("dll");
        }
        c.library_name = Some(p.to_string_lossy().into_owned());
    }
    if c.out.is_none()
        && let Some(n) = &def.name
    {
        let mut p = std::path::PathBuf::from(n);
        if p.extension().is_none() {
            p.set_extension(if def.library { "dll" } else { "exe" });
        }
        c.out = Some(p);
    }
    if c.base.is_none() {
        c.base = def.base;
    }
    if let Some(h) = def.heap {
        c.heap = h;
    }
    if let Some(s) = def.stack {
        c.stack = s;
    }
    if let Some(v) = def.version
        && c.version == (0, 0)
    {
        c.version = v;
    }
    for s in &def.sections {
        if let Ok(a) = crate::config::SectionAttributes::parse(s) {
            c.sections.push(a);
        }
    }
}

/// The exports, merged (the first of each name wins, as link.exe keeps
/// the first and warns, LNK4197) and given ordinals: explicit ones kept,
/// the others numbered after the largest, in name order.
pub fn finish(exports: &[Export]) -> Result<Vec<Export>, String> {
    let mut seen = turbonop_hash::Set::default();
    let mut out: Vec<Export> = Vec::new();
    for e in exports {
        let key = e.export_as.clone().unwrap_or_else(|| e.name.clone());
        if seen.insert(key) {
            out.push(e.clone());
        }
    }
    let mut used = turbonop_hash::Set::default();
    for e in &out {
        if let Some(o) = e.ordinal
            && !used.insert(o)
        {
            return Err(format!("ordinal {o} is exported twice"));
        }
    }
    let mut next = out.iter().filter_map(|e| e.ordinal).max().unwrap_or(0);
    let mut unnumbered: Vec<usize> = (0..out.len())
        .filter(|&i| out[i].ordinal.is_none())
        .collect();
    unnumbered.sort_by(|&a, &b| exported_name(&out[a]).cmp(exported_name(&out[b])));
    for i in unnumbered {
        next = next.checked_add(1).ok_or("more than 65535 exports")?;
        out[i].ordinal = Some(next);
    }
    Ok(out)
}

/// The name an export is exported by.
pub fn exported_name(e: &Export) -> &str {
    e.export_as.as_deref().unwrap_or(&e.name)
}

/// A forwarder's target (`/EXPORT:f=other.g` names `other.g`, a function
/// of another DLL, when `other.g` isn't a symbol).
pub fn forwarder(e: &Export, defined: impl Fn(&str) -> bool) -> Option<&str> {
    let target = e.internal.as_deref()?;
    (target.contains('.') && !defined(target)).then_some(target)
}

/// The export directory's layout: its size, and where each part is.
#[derive(Clone, Debug)]
pub struct Directory {
    /// The exports, in ordinal order.
    pub exports: Vec<Export>,
    /// The lowest ordinal.
    pub base: u16,
    /// Address table entries (highest ordinal - base + 1).
    pub functions: u32,
    /// Named exports, sorted by name: (name, index in `exports`).
    pub names: Vec<(String, usize)>,
    /// The size of the directory and everything it holds.
    pub size: u32,
}

/// Plans the export directory for `exports` (from [`finish`]), with
/// `forwarders[i]` the forwarder string of export `i`, if it is one.
pub fn plan(exports: Vec<Export>, dll_name: &str, forwarders: &[Option<String>]) -> Directory {
    let base = exports.iter().filter_map(|e| e.ordinal).min().unwrap_or(1);
    let max = exports.iter().filter_map(|e| e.ordinal).max().unwrap_or(0);
    let functions = u32::from(max.saturating_sub(base)) + u32::from(!exports.is_empty());
    let mut names: Vec<(String, usize)> = exports
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.noname)
        .map(|(i, e)| (exported_name(e).to_owned(), i))
        .collect();
    names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut size = 40 + functions * 4 + crate::n32(names.len()) * 6;
    size += crate::n32(dll_name.len()) + 1;
    for (n, _) in &names {
        size += crate::n32(n.len()) + 1;
    }
    for f in forwarders.iter().flatten() {
        size += crate::n32(f.len()) + 1;
    }
    Directory {
        exports,
        base,
        functions,
        names,
        size,
    }
}

impl Directory {
    /// The export object's symbols (link.exe's `.exp`, which it links into
    /// the image) and their offsets in the directory: the section, the
    /// address table (`rgpv`), the name pointers (`rgszName`), the
    /// ordinals (`rgwOrd`), the DLL's name (`szName`), and each name's
    /// string (`$N` and its export's ordinal, five digits).
    pub fn symbols(&self, dll_name: &str) -> Vec<(String, u32)> {
        let functions_at = 40u32;
        let names_at = functions_at + self.functions * 4;
        let ordinals_at = names_at + crate::n32(self.names.len()) * 4;
        let strings_at = ordinals_at + crate::n32(self.names.len()) * 2;
        let mut out = vec![
            (".edata".to_owned(), 0),
            ("rgpv".to_owned(), functions_at),
            ("rgszName".to_owned(), names_at),
            ("rgwOrd".to_owned(), ordinals_at),
            ("szName".to_owned(), strings_at),
        ];
        let mut at = strings_at + crate::n32(dll_name.len()) + 1;
        for (name, i) in &self.names {
            let ordinal = self.exports[*i].ordinal.unwrap_or(self.base);
            out.push((format!("$N{ordinal:05}"), at));
            at += crate::n32(name.len()) + 1;
        }
        out
    }

    /// Writes the directory at `rva` into `out`: `address(i)` is export
    /// `i`'s address (unused for forwarders).
    pub fn write(
        &self,
        out: &mut [u8],
        rva: u32,
        dll_name: &str,
        timestamp: u32,
        address: impl Fn(usize) -> u32,
        forwarders: &[Option<String>],
    ) {
        let functions_at = 40u32;
        let names_at = functions_at + self.functions * 4;
        let ordinals_at = names_at + crate::n32(self.names.len()) * 4;
        let mut strings_at = ordinals_at + crate::n32(self.names.len()) * 2;
        let put32 = |out: &mut [u8], at: u32, v: u32| {
            out[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        };
        let mut put_string = |out: &mut [u8], s: &str| -> u32 {
            let at = strings_at;
            out[at as usize..at as usize + s.len()].copy_from_slice(s.as_bytes());
            strings_at += crate::n32(s.len()) + 1;
            rva + at
        };
        let name_rva = put_string(out, dll_name);
        put32(out, 4, timestamp);
        put32(out, 12, name_rva);
        put32(out, 16, u32::from(self.base));
        put32(out, 20, self.functions);
        put32(out, 24, crate::n32(self.names.len()));
        put32(out, 28, rva + functions_at);
        put32(out, 32, rva + names_at);
        put32(out, 36, rva + ordinals_at);
        for (k, (name, i)) in self.names.iter().enumerate() {
            let at = put_string(out, name);
            put32(out, names_at + crate::n32(k) * 4, at);
            let ord = self.exports[*i].ordinal.unwrap_or(self.base) - self.base;
            let o = (ordinals_at + crate::n32(k) * 2) as usize;
            out[o..o + 2].copy_from_slice(&ord.to_le_bytes());
        }
        for (i, e) in self.exports.iter().enumerate() {
            let slot = u32::from(e.ordinal.unwrap_or(self.base) - self.base);
            let target = match &forwarders[i] {
                Some(f) => put_string(out, f),
                None => address(i),
            };
            put32(out, functions_at + slot * 4, target);
        }
    }
}

/// What the image's export directory needs: the directory, each
/// export's forwarder string (if it is one) and global (if not), and the
/// DLL's name.
#[derive(Clone, Debug)]
pub struct ExportTable {
    /// The directory.
    pub directory: Directory,
    /// Per export: its forwarder.
    pub forwarders: Vec<Option<String>>,
    /// Per export: its global (`u32::MAX` for a forwarder).
    pub targets: Vec<u32>,
    /// The DLL's name.
    pub dll_name: String,
}

#[cfg(test)]
mod tests;
