//! What `lib /DEF` makes of each export, given its `.def` file, its
//! `/EXPORT` options, and the objects on its command line: the import
//! object's symbol and name, the name the DLL exports it by, and the symbol
//! the export file's address table refers to. Both the import library
//! ([`crate::imports`]) and the export file ([`crate::exp`]) are made of
//! these.
//!
//! Measured with `lib.exe` 14.52:
//!
//! - **Without objects**, an export's symbol is its name. On x86, `_` is
//!   put before a name (and an internal name) that doesn't start with `?`
//!   or `@`, and the import is `NOPREFIX`. On ARM64EC code is mangled (`#f`,
//!   `?f@@$$hYAXXZ`).
//! - **Objects' `/EXPORT` directives** (`.drectve`) are taken as they are
//!   written: their names are symbols. On x86 a name that starts with `_`
//!   and has no `@` is imported `NOPREFIX` (exported without the `_`); any
//!   other by name.
//! - **With objects**, an export of the `.def` file or the command line is
//!   found among the objects' external definitions: as its name (on x86,
//!   with `_` put before it), as written, then (x86) as a `__stdcall`
//!   (`_f@8`) or `__fastcall` (`@f@4`) name, then as a C++ name (`?f@...`,
//!   in any scope), which must be the only one (else LNK4022). Found
//!   decorated, it is imported `UNDECORATE`. `a=b` finds `b`: found as
//!   written, the import's symbol is `a`, by name; found with `_` put
//!   before it, `_a`, `NOPREFIX`; found decorated, `a` decorated as `b` is
//!   (`_a@8`), `UNDECORATE`. One not found is LNK2001, and a forwarder is
//!   reported as its export (`x.y` as `y`).
//! - Two exports with one name, or one symbol, are one: the first (the
//!   `.def` file's, then the command line's, then the objects'
//!   directives, in order). For one symbol, the index's list of members
//!   has an empty place where the second would be.
//! - On x86 a name holding `@`, found with `_` before it, is imported as
//!   written, by name, and the address table refers to it as written.
//!
//! The hint of an import by name is its export's place among the exported
//! names, sorted by their bytes (ARM64EC: 0, see [`crate::imports`]).

use alloc::borrow::Cow;
use alloc::vec::Vec;

use turbonop_archive::symbols::{ec_demangle, ec_mangle};
use turbonop_pe::consts::machine;
use turbonop_pe::import_object::{ImportNameType, ImportType};

use crate::{Export, ExportKind};

/// Where an export was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The `.def` file's `EXPORTS`.
    Definition,
    /// An `/EXPORT` option.
    Option,
    /// An object's `/EXPORT` directive: its names are symbols.
    Directive,
}

/// One export as `lib.exe` resolves it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Resolved<'a> {
    /// The import object's symbol, by which exports are sorted (for their
    /// ordinals, and the export file's order).
    pub symbol: Cow<'a, [u8]>,
    /// The name the DLL exports it by (unless `noname`).
    pub name: Cow<'a, [u8]>,
    /// The symbol the export address table refers to (for a forwarder, the
    /// export file's own symbol for the forwarder's string: `symbol`).
    pub internal: Cow<'a, [u8]>,
    /// The forwarder (`other.Func`, `other.#3`), if it is one.
    pub forwarder: Option<&'a [u8]>,
    /// The ordinal it was given.
    pub ordinal: Option<u16>,
    /// `NONAME`.
    pub noname: bool,
    /// Code, `DATA`, or `CONSTANT`.
    pub kind: ExportKind,
    /// `PRIVATE`: not in the import library.
    pub private: bool,
    /// How the import (by name) finds it.
    pub name_type: ImportNameType,
    /// The import's name after the DLL's (`EXPORTAS`, and ARM64EC's plain
    /// names).
    pub import_name: Option<Cow<'a, [u8]>>,
}

impl Resolved<'_> {
    /// The import object's type.
    pub fn import_type(&self) -> ImportType {
        match self.kind {
            ExportKind::Code => ImportType::Code,
            ExportKind::Data => ImportType::Data,
            ExportKind::Constant => ImportType::Const,
        }
    }
}

/// Why an export can't be resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Unresolved<'a> {
    /// No object defines it (LNK2001), by the name reported.
    Missing(Cow<'a, [u8]>),
    /// Several C++ names match it (LNK4022, then LNK2001): it, and them.
    Ambiguous(&'a [u8], Vec<&'a [u8]>),
}

/// The external symbols the objects define, sorted, for finding exports.
#[derive(Clone, Debug, Default)]
pub struct Definitions<'a> {
    sorted: Vec<&'a [u8]>,
}

impl<'a> Definitions<'a> {
    /// The definitions, in any order.
    pub fn new(mut names: Vec<&'a [u8]>) -> Self {
        names.sort_unstable();
        names.dedup();
        Self { sorted: names }
    }

    /// Whether `name` is defined.
    pub fn defines(&self, name: &[u8]) -> bool {
        self.sorted.binary_search(&name).is_ok()
    }

    /// The defined names that start with `prefix`.
    fn starting(&self, prefix: &[u8]) -> &[&'a [u8]] {
        let from = self.sorted.partition_point(|n| *n < prefix);
        let to = from + self.sorted[from..].partition_point(|n| n.starts_with(prefix));
        &self.sorted[from..to]
    }
}

/// How a name was found among the definitions.
enum Found<'a> {
    /// As given, `_` put before it (x86).
    Prefixed(Vec<u8>),
    /// As written.
    Exact,
    /// Decorated: the symbol, and the decoration around the name (what
    /// comes before it, and after).
    Decorated(&'a [u8], &'a [u8], &'a [u8]),
}

/// `_` before `name` on x86, unless it starts with `?` or `@`.
fn x86_prefixed(name: &[u8]) -> Option<Vec<u8>> {
    if matches!(name.first(), Some(b'?' | b'@')) {
        return None;
    }
    let mut v = Vec::with_capacity(name.len() + 1);
    v.push(b'_');
    v.extend_from_slice(name);
    Some(v)
}

fn find<'a>(
    name: &'a [u8],
    x86: bool,
    defs: &Definitions<'a>,
) -> Result<Found<'a>, Unresolved<'a>> {
    if x86
        && let Some(p) = x86_prefixed(name)
        && defs.defines(&p)
    {
        return Ok(Found::Prefixed(p));
    }
    if defs.defines(name) {
        return Ok(Found::Exact);
    }
    let digits = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
    if x86 {
        for lead in [&b"_"[..], &b"@"[..]] {
            let mut prefix = lead.to_vec();
            prefix.extend_from_slice(name);
            prefix.push(b'@');
            for &s in defs.starting(&prefix) {
                if digits(&s[prefix.len()..]) {
                    let after = &s[prefix.len() - 1..];
                    return Ok(Found::Decorated(s, &s[..lead.len()], after));
                }
            }
        }
    }
    // A C++ name: `?name@` and the rest.
    let mut prefix = b"?".to_vec();
    prefix.extend_from_slice(name);
    prefix.push(b'@');
    let matches = defs.starting(&prefix);
    match matches {
        [] => Err(Unresolved::Missing(Cow::Borrowed(name))),
        [one] => Ok(Found::Decorated(one, &one[..1], &one[1 + name.len()..])),
        many => Err(Unresolved::Ambiguous(name, many.to_vec())),
    }
}

fn concat<'a>(parts: &[&[u8]]) -> Cow<'a, [u8]> {
    Cow::Owned(parts.concat())
}

/// The name an import by `name_type` of `symbol` is exported by.
fn exported_by(symbol: &[u8], name_type: ImportNameType) -> &[u8] {
    let stripped = match symbol.first() {
        Some(b'?' | b'@' | b'_') => &symbol[1..],
        _ => symbol,
    };
    match name_type {
        ImportNameType::NoPrefix => stripped,
        ImportNameType::Undecorate => {
            &stripped[..stripped
                .iter()
                .position(|&b| b == b'@')
                .unwrap_or(stripped.len())]
        }
        _ => symbol,
    }
}

/// Resolves `exports` (each with where it was given) as `lib /DEF` does
/// for `machine`, finding them among `objects`' definitions if there are
/// objects (`None`: none on the command line). The result is in the given
/// order, each symbol once.
pub fn resolve<'a>(
    exports: &[(Export<'a>, Origin)],
    machine: u16,
    objects: Option<&Definitions<'a>>,
) -> Result<Vec<Resolved<'a>>, Vec<Unresolved<'a>>> {
    resolve_with_holes(exports, machine, objects).map(|(r, _)| r)
}

/// [`resolve`], and where `lib.exe`'s index lists an empty place for an
/// export whose symbol an earlier one has (as places among the imports,
/// `PRIVATE` exports not counted, the empty places counted).
pub fn resolve_with_holes<'a>(
    exports: &[(Export<'a>, Origin)],
    machine: u16,
    objects: Option<&Definitions<'a>>,
) -> Result<(Vec<Resolved<'a>>, Vec<usize>), Vec<Unresolved<'a>>> {
    let mut holes = Vec::new();
    let mut listed = 0;
    let x86 = machine == machine::I386;
    let ec = machine == machine::ARM64EC;
    let mut out: Vec<Resolved<'a>> = Vec::with_capacity(exports.len());
    let mut errors = Vec::new();
    let mut seen_names: alloc::collections::BTreeSet<&[u8]> = alloc::collections::BTreeSet::new();
    let mut seen: alloc::collections::BTreeSet<Vec<u8>> = alloc::collections::BTreeSet::new();
    for &(e, origin) in exports {
        if !seen_names.insert(e.name) {
            continue;
        }
        // An empty internal name: the `.def` file's is none (`a=` exports
        // `a`); an option's stays empty (measured).
        let mut e = e;
        if origin == Origin::Definition && e.internal == Some(&[][..]) {
            e.internal = None;
        }
        let forwarder = e.forwarder().map(|_| e.internal.unwrap_or_default());
        let r = match (origin, objects) {
            (Origin::Directive, _) => Ok(directive(&e, x86)),
            (_, None) => Ok(plain(&e, x86, ec)),
            (_, Some(defs)) => match forwarder {
                // Measured once: a forwarder, with objects, is reported
                // unresolved, by the name after its last `.`.
                Some(f) => {
                    let dot = f.iter().rposition(|&b| b == b'.').unwrap_or(0);
                    Err(Unresolved::Missing(Cow::Borrowed(&f[dot + 1..])))
                }
                None => found(&e, x86, defs),
            },
        };
        match r {
            Ok(mut r) => {
                if r.forwarder.is_none() {
                    r.forwarder = forwarder;
                    if forwarder.is_some() {
                        r.internal = r.symbol.clone();
                    }
                }
                if seen.insert(r.symbol.to_vec()) {
                    listed += usize::from(!r.private);
                    out.push(r);
                } else if !r.private {
                    holes.push(listed);
                    listed += 1;
                }
            }
            Err(u) => errors.push(u),
        }
    }
    if errors.is_empty() {
        Ok((out, holes))
    } else {
        Err(errors)
    }
}

fn base<'a>(e: &Export<'a>) -> Resolved<'a> {
    Resolved {
        symbol: Cow::Borrowed(e.name),
        name: Cow::Borrowed(e.export_as.unwrap_or(e.name)),
        internal: Cow::Borrowed(e.internal.unwrap_or(e.name)),
        forwarder: None,
        ordinal: e.ordinal,
        noname: e.noname,
        kind: e.kind,
        private: e.private,
        name_type: if e.export_as.is_some() {
            ImportNameType::ExportAs
        } else {
            ImportNameType::Name
        },
        import_name: e.export_as.map(Cow::Borrowed),
    }
}

/// An export of the `.def` file or the command line, without objects.
fn plain<'a>(e: &Export<'a>, x86: bool, ec: bool) -> Resolved<'a> {
    let mut r = base(e);
    let forwarder = e.forwarder().is_some();
    if ec && e.kind == ExportKind::Code {
        r.symbol = ec_mangle(e.name).unwrap_or(Cow::Borrowed(e.name));
        let plain = e
            .export_as
            .map_or_else(|| ec_demangle(e.name), Cow::Borrowed);
        r.name = plain.clone();
        r.import_name = Some(plain);
        r.name_type = ImportNameType::ExportAs;
        if !forwarder {
            let internal = e.internal.unwrap_or(e.name);
            r.internal = ec_mangle(internal).unwrap_or(Cow::Borrowed(internal));
        }
    } else if x86 && let Some(p) = x86_prefixed(e.name) {
        r.symbol = Cow::Owned(p);
        if e.export_as.is_none() {
            r.name_type = ImportNameType::NoPrefix;
        }
        if !forwarder {
            let internal = e.internal.unwrap_or(e.name);
            r.internal = x86_prefixed(internal).map_or(Cow::Borrowed(internal), Cow::Owned);
        }
    }
    r
}

/// An object's `/EXPORT` directive: names are symbols.
fn directive<'a>(e: &Export<'a>, x86: bool) -> Resolved<'a> {
    let mut r = base(e);
    if x86 && e.export_as.is_none() && e.name.first() == Some(&b'_') && !e.name.contains(&b'@') {
        r.name_type = ImportNameType::NoPrefix;
    }
    if e.export_as.is_none() {
        r.name = Cow::Borrowed(exported_by(e.name, r.name_type));
    }
    r
}

/// An export of the `.def` file or the command line, found among the
/// objects' definitions.
fn found<'a>(
    e: &Export<'a>,
    x86: bool,
    defs: &Definitions<'a>,
) -> Result<Resolved<'a>, Unresolved<'a>> {
    let mut r = base(e);
    let renamed = e.internal.is_some();
    let target = e.internal.unwrap_or(e.name);
    let by_name = |t| {
        if e.export_as.is_some() {
            ImportNameType::ExportAs
        } else {
            t
        }
    };
    match find(target, x86, defs)? {
        // A name holding `@`, found with `_` before it, is imported as
        // written, by name (measured: `Func@8` for `_Func@8`).
        Found::Prefixed(_) if e.name.contains(&b'@') && !renamed => {
            r.name_type = by_name(ImportNameType::Name);
        }
        Found::Prefixed(p) => {
            r.symbol = if renamed {
                x86_prefixed(e.name).map_or(Cow::Borrowed(e.name), Cow::Owned)
            } else {
                Cow::Owned(p.clone())
            };
            r.internal = Cow::Owned(p);
            r.name_type = by_name(ImportNameType::NoPrefix);
        }
        Found::Exact => {
            r.internal = Cow::Borrowed(target);
            r.name_type = by_name(ImportNameType::Name);
        }
        Found::Decorated(symbol, before, after) => {
            r.symbol = if renamed {
                concat(&[before, e.name, after])
            } else {
                Cow::Borrowed(symbol)
            };
            r.internal = Cow::Borrowed(symbol);
            r.name_type = by_name(ImportNameType::Undecorate);
        }
    }
    Ok(r)
}

/// The hint of each of `exports` imported by name: its place among the
/// exported names (all but `NONAME` exports'), sorted.
pub fn hints(exports: &[Resolved<'_>]) -> Vec<u16> {
    let mut named: Vec<&[u8]> = exports
        .iter()
        .filter(|e| !e.noname)
        .map(|e| &*e.name)
        .collect();
    named.sort_unstable();
    exports
        .iter()
        .map(|e| {
            let at = named.binary_search(&&*e.name).unwrap_or(0);
            u16::try_from(at).unwrap_or(u16::MAX)
        })
        .collect()
}

#[cfg(test)]
mod tests;
