//! Writing the library: each member's name and header fields, its symbols
//! for the index, `lib.exe`'s warnings about names defined twice, and the
//! archive.
//!
//! Measured with `lib.exe` 14.52:
//!
//! - A member copied from an input library keeps its header's date and
//!   mode; under `/Brepro` its date is `-1`. An object read from a file is
//!   dated with the file's time, mode `100666` (`100444` if read-only).
//!   `llvm-lib` writes `0 0 0 644` for every member.
//! - LNK4006 ("already defined in ...; second definition ignored") is
//!   reported for a name defined again, outside a COMDAT or in a
//!   `NODUPLICATES` COMDAT, by a later member in archive order; not for
//!   other COMDATs or for commons. Each member's names come in sorted
//!   order, and a C++ name is shown undecorated, then decorated in
//!   parentheses.

use super::args::Flavor;
use super::diag::{Diag, Kind};
use std::borrow::Cow;
use turbonop_archive::write::{Index, MemberSymbol, MemberSymbols, Tool};

/// The tool whose index to build.
pub fn tool(flavor: Flavor) -> Tool {
    match flavor {
        Flavor::Lib => Tool::LibExe,
        Flavor::LlvmLib => Tool::LlvmLib,
    }
}

/// The symbols a `/GL` object puts in the index: its IL's public
/// definitions and commons, by their decorated names (x86's C names get
/// back the underscore the IL leaves off).
pub fn ltcg_symbols(data: &[u8]) -> Option<MemberSymbols<'_>> {
    use turbonop_pe::ltcg::{LtcgObject, Storage};
    let object = LtcgObject::parse(data).ok()?;
    let globals = object.globals().ok()?;
    use turbonop_pe::ltcg::{SymbolKind, function};
    let x86 = object.machine == 0x014C;
    // lib.exe lists an inline function only if something listed refers to
    // it, transitively. The roots: functions not inline; inline virtual
    // member functions, which vftables refer to (their initializers aren't
    // read); and data no inline function refers to (a local static of an
    // inline function is listed with its function). Matched on the
    // fixtures' 31 and x86 symbols; likely incomplete for other shapes.
    let inline = |s: &turbonop_pe::ltcg::IlSymbol<'_>| matches!(s.kind, SymbolKind::Function { attributes, .. } if attributes & function::INLINE != 0);
    let virtual_member = |s: &turbonop_pe::ltcg::IlSymbol<'_>| {
        let name = String::from_utf8_lossy(s.name);
        turbonop_demangle::msvc::demangle(&name, 0).is_some_and(|d| {
            d.starts_with("public: virtual ")
                || d.starts_with("protected: virtual ")
                || d.starts_with("private: virtual ")
        })
    };
    let by_id: std::collections::HashMap<u32, usize> = globals
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect();
    let mut from_inline = vec![false; globals.symbols.len()];
    for s in &globals.symbols {
        if inline(s)
            && let Some(il) = &s.il
        {
            for (id, _) in il.refs() {
                if let Some(&j) = by_id.get(&id) {
                    from_inline[j] = true;
                }
            }
        }
    }
    let mut listed = vec![false; globals.symbols.len()];
    let mut work: Vec<usize> = Vec::new();
    for (i, s) in globals.symbols.iter().enumerate() {
        let root = match s.kind {
            SymbolKind::Function { .. } => !inline(s) || virtual_member(s),
            SymbolKind::Data { .. } => !from_inline[i],
        };
        if root {
            listed[i] = true;
            work.push(i);
        }
    }
    while let Some(i) = work.pop() {
        let Some(il) = &globals.symbols[i].il else {
            continue;
        };
        for (id, _) in il.refs() {
            if let Some(&j) = by_id.get(&id)
                && !listed[j]
            {
                listed[j] = true;
                work.push(j);
            }
        }
    }
    let mut symbols = Vec::new();
    for (i, s) in globals.symbols.iter().enumerate() {
        let storage = s.storage();
        let common = storage == Storage::Common;
        if !listed[i] || (!storage.is_public_definition() && !common) {
            continue;
        }
        // x86 C names: `_name`, but `@name@n` for __fastcall, whose
        // attributes' low bits are 4 (seen on one fixture: likely).
        let fastcall =
            matches!(s.kind, SymbolKind::Function { attributes, .. } if attributes & 0x0F == 4);
        let name: Cow<'_, [u8]> = if x86 && !s.name.starts_with(b"?") && !s.name.starts_with(b"@") {
            let mut n = if fastcall {
                b"@".to_vec()
            } else {
                b"_".to_vec()
            };
            n.extend_from_slice(s.name);
            Cow::Owned(n)
        } else {
            Cow::Borrowed(s.name)
        };
        symbols.push(MemberSymbol { name, common });
    }
    Some(MemberSymbols {
        symbols,
        ec_symbols: Vec::new(),
    })
}

/// A member's symbols for the index. `symbol_table` is where `data` has
/// the symbol table, if `data` is an object without its larger section
/// data (see `turbonop_coff::Skeleton`).
pub fn member_symbols<'d>(
    flavor: Flavor,
    name: &str,
    data: &'d [u8],
    symbol_table: Option<u32>,
    ec: bool,
) -> Result<MemberSymbols<'d>, Diag> {
    use turbonop_archive::symbols::{self, SymbolsError};
    let listed = match symbol_table {
        Some(at) => symbols::skeleton_symbols(data, at, tool(flavor), ec),
        None => symbols::member_symbols(data, tool(flavor), ec),
    };
    match listed {
        Ok(s) => Ok(s),
        Err(SymbolsError::Ltcg) => ltcg_symbols(data).ok_or_else(|| match flavor {
            Flavor::Lib => Diag::fatal(1107, "invalid or corrupt file").on(name),
            Flavor::LlvmLib => Diag::llvm(Kind::Fatal, "unreadable /GL object").on(name),
        }),
        // Resource files and anything else stored whole define nothing.
        Err(_) => Ok(MemberSymbols::default()),
    }
}

/// The names a COFF object defines strongly: outside COMDATs or in a
/// `NODUPLICATES` COMDAT, the ones LNK4006 is about. Read with the
/// zero-copy reader: a COMDAT's selection is in its section symbol's
/// auxiliary record (byte 14). `symbol_table` is as for [`member_symbols`].
pub fn strong_definitions(data: &[u8], symbol_table: Option<u32>) -> Vec<&[u8]> {
    const IMAGE_SCN_LNK_COMDAT: u32 = 0x1000;
    const EXTERNAL: u8 = 2;
    const STATIC: u8 = 3;
    const NODUPLICATES: u8 = 1;
    let file = match symbol_table {
        Some(at) => turbonop_pe::PeFile::parse_object_at(data, at),
        None => turbonop_pe::PeFile::parse_object(data),
    };
    let Ok(file) = file else {
        return Vec::new();
    };
    let Ok(Some(table)) = file.symbols() else {
        return Vec::new();
    };
    let sections = file.sections();
    let mut selection = vec![0u8; sections.len() + 1];
    for s in table.iter() {
        if s.storage_class == STATIC
            && let Ok(n) = usize::try_from(s.section)
            && n >= 1
            && n <= sections.len()
            && let Some(aux) = s.aux().next()
            && aux.len() > 14
        {
            selection[n] = aux[14];
        }
    }
    let mut out = Vec::new();
    for s in table.iter() {
        if s.storage_class != EXTERNAL {
            continue;
        }
        let strong = match s.section {
            -1 => true,
            n if n >= 1 && (n as usize) <= sections.len() => {
                sections[n as usize - 1].characteristics.get() & IMAGE_SCN_LNK_COMDAT == 0
                    || selection[n as usize] == NODUPLICATES
            }
            _ => false,
        };
        if strong && let Some(name) = table.name(&s) {
            out.push(name);
        }
    }
    out
}

/// How `lib.exe` shows a name in a message: a C++ name undecorated, with
/// the decorated name after it in parentheses.
pub fn shown(name: &[u8]) -> String {
    let text = String::from_utf8_lossy(name);
    if text.starts_with('?')
        && let Some(u) = turbonop_demangle::msvc::demangle(&text, 0)
    {
        return format!("\"{u}\" ({text})");
    }
    text.into_owned()
}

/// LNK4006 for each strong name defined again by a later member. `places`
/// says how each member is named in messages (`a.obj`, `x.lib(a.obj)`).
pub fn duplicate_warnings(strong: &[Vec<&[u8]>], places: &[String]) -> Vec<Diag> {
    let mut first: std::collections::HashMap<&[u8], usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for (m, names) in strong.iter().enumerate() {
        let mut sorted: Vec<&[u8]> = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        for n in sorted {
            match first.get(n) {
                Some(&f) => out.push(
                    Diag::warning(
                        4006,
                        format!(
                            "{} already defined in {}; second definition ignored",
                            shown(n),
                            places[f]
                        ),
                    )
                    .on(places[m].clone()),
                ),
                None => {
                    first.insert(n, m);
                }
            }
        }
    }
    out
}

/// The index of `symbols` (each member's, in archive order).
pub fn index<'d>(flavor: Flavor, symbols: Vec<MemberSymbols<'d>>, ec: bool) -> Index<'d> {
    Index::build(
        // Member numbers fit: the writer refuses more than 65,535 members.
        (0u32..).zip(symbols),
        tool(flavor),
        ec,
    )
}

#[cfg(test)]
mod tests;
