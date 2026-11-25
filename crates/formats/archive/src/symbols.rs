//! The symbols an archive's index lists for a member, as `lib.exe` and
//! `llvm-lib` choose them (measured with MSVC 14.52 and LLVM 24, on
//! objects made to hold every kind of symbol).
//!
//! **A COFF object** (or `/bigobj` object): its symbols in symbol-table
//! order, those that are:
//!
//! | Symbol | `lib.exe` | `llvm-lib` |
//! |---|---|---|
//! | external, in a section or absolute (-1) | listed | listed |
//! | external, undefined with a value (common) | listed | listed |
//! | external, undefined | not | not |
//! | external, in the debug section (-2) | refused (LNK1234) | listed |
//! | weak external, in a section, absolute, or with a value | listed | only if an alias |
//! | weak external, undefined: an alias (`IMAGE_WEAK_EXTERN_SEARCH_ALIAS`) | listed | listed |
//! | weak external, undefined: no library, library search, anti-dependency | not | not |
//! | weak external without an auxiliary record or with another kind | refused (LNK1129) | not |
//! | static, label, section, function, file, CLR token, and the rest | not | not |
//!
//! In an ARM64EC or ARM64X archive, the symbols of ARM64EC and x64 objects
//! go into `/<ECSYMBOLS>/`, and the rest into the linker members.
//!
//! **An import object** (`S` its symbol):
//!
//! | Import | Listed |
//! |---|---|
//! | code or constant | `__imp_S` and `S` |
//! | data | `__imp_S` |
//!
//! For ARM64EC (all in `/<ECSYMBOLS>/`), with `D` the symbol demangled
//! (`#` removed from the front, or `$$h` from a C++ name) and `M` its
//! mangled form:
//!
//! | Import | `lib.exe` | `llvm-lib` |
//! |---|---|---|
//! | code | `M`, `D`, `__imp_D`, `__imp_aux_D` | the same |
//! | constant | `M`, `D`, `__imp_D`, `__imp_aux_D` | `D`, `__imp_D`, `__imp_aux_D` |
//! | data | `__imp_D` | the same |
//!
//! (A constant's C++ name that isn't a function's has no mangled form, and
//! `lib.exe` lists an empty name for it: measured for `?CppConst@@3HA`.)
//!
//! **An LTCG (`/GL`) object**'s symbols come from the compiler's
//! intermediate language, which `lib.exe` has the compiler read; they are
//! not computed here.
//!
//! `lib /DEF` also lists the import descriptor objects' symbols in
//! `/<ECSYMBOLS>/` of an ARM64EC import library, as well as in the linker
//! members. (The same objects in an archive of objects are only in the
//! linker members.) That is for the import library's writer to add.

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

use turbonop_pe::PeFile;
use turbonop_pe::consts::{machine, storage_class};
use turbonop_pe::import_object::{ImportObject, ImportType};
use turbonop_pe::ltcg::LtcgObject;

pub use crate::write::Tool;
use crate::write::{MemberSymbol, MemberSymbols};

/// Why a member's symbols weren't computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolsError {
    /// An LTCG (`/GL`) object, whose symbols only the compiler can list.
    Ltcg,
    /// Neither a COFF object nor an import object.
    NotAnObject,
    /// The object is damaged.
    Malformed(turbonop_core::Error),
    /// `lib.exe` refuses the object: an external symbol in the debug
    /// section (LNK1234), or a weak external without a usable auxiliary
    /// record (LNK1129).
    Refused {
        /// The symbol's index in the symbol table.
        symbol: usize,
    },
}

impl fmt::Display for SymbolsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SymbolsError::Ltcg => f.write_str("an LTCG object"),
            SymbolsError::NotAnObject => f.write_str("not an object"),
            SymbolsError::Malformed(e) => write!(f, "{e}"),
            SymbolsError::Refused { symbol } => {
                write!(f, "lib.exe refuses symbol {symbol}")
            }
        }
    }
}

impl core::error::Error for SymbolsError {}

/// `IMAGE_WEAK_EXTERN_SEARCH_ALIAS`.
const WEAK_ALIAS: u32 = 3;

/// Whether `name` is one of the symbols of an import library's descriptor
/// objects: `__IMPORT_DESCRIPTOR_<dll>`, `__NULL_IMPORT_DESCRIPTOR`, or
/// `\x7f<dll>_NULL_THUNK_DATA`.
pub fn is_import_descriptor(name: &[u8]) -> bool {
    name.starts_with(b"__IMPORT_DESCRIPTOR_")
        || name == b"__NULL_IMPORT_DESCRIPTOR"
        || (name.starts_with(b"\x7f") && name.ends_with(b"_NULL_THUNK_DATA"))
}

/// Whether a member of `machine` puts its symbols in `/<ECSYMBOLS>/` (in an
/// ARM64EC or ARM64X archive).
pub fn is_ec_machine(m: u16) -> bool {
    m == machine::ARM64EC || m == machine::AMD64
}

/// The symbols `tool` lists in the index for the member `data`, in the
/// member's order. `ec_archive` is whether the archive is ARM64EC or
/// ARM64X (has a `/<ECSYMBOLS>/`).
pub fn member_symbols(
    data: &[u8],
    tool: Tool,
    ec_archive: bool,
) -> Result<MemberSymbols<'_>, SymbolsError> {
    if ImportObject::is_import_object(data) {
        let import = ImportObject::parse(data).map_err(SymbolsError::Malformed)?;
        return Ok(import_symbols(&import, tool, ec_archive));
    }
    if LtcgObject::is_ltcg(data) {
        return Err(SymbolsError::Ltcg);
    }
    let file = PeFile::parse_object(data).map_err(|_| SymbolsError::NotAnObject)?;
    let symbols = coff_symbols(&file, tool)?;
    let ec = ec_archive && is_ec_machine(file.machine());
    Ok(if ec {
        MemberSymbols {
            symbols: Vec::new(),
            ec_symbols: symbols,
        }
    } else {
        MemberSymbols {
            symbols,
            ec_symbols: Vec::new(),
        }
    })
}

/// A COFF object's symbols that `tool` lists, in symbol-table order.
pub fn coff_symbols<'a>(
    file: &PeFile<'a>,
    tool: Tool,
) -> Result<Vec<MemberSymbol<'a>>, SymbolsError> {
    let Some(table) = file.symbols().map_err(SymbolsError::Malformed)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for s in table.iter() {
        let listed = match s.storage_class {
            storage_class::EXTERNAL => match (s.section, tool) {
                (0, _) => s.value != 0,
                (-2, Tool::LibExe) => return Err(SymbolsError::Refused { symbol: s.index }),
                _ => true,
            },
            storage_class::WEAK_EXTERNAL => {
                let kind = s
                    .aux()
                    .next()
                    .map(|a| u32::from_le_bytes([a[4], a[5], a[6], a[7]]));
                match tool {
                    Tool::LibExe => {
                        if s.section != 0 || s.value != 0 {
                            true
                        } else {
                            match kind {
                                Some(1 | 2 | 4) => false,
                                Some(WEAK_ALIAS) => true,
                                _ => return Err(SymbolsError::Refused { symbol: s.index }),
                            }
                        }
                    }
                    Tool::LlvmLib => kind == Some(WEAK_ALIAS),
                }
            }
            _ => false,
        };
        if listed {
            let name = table.name(&s).unwrap_or_default();
            out.push(MemberSymbol {
                name: Cow::Borrowed(name),
                common: s.storage_class == storage_class::EXTERNAL && s.section == 0,
            });
        }
    }
    Ok(out)
}

fn concat<'a>(prefix: &[u8], name: &[u8]) -> Cow<'a, [u8]> {
    let mut v = Vec::with_capacity(prefix.len() + name.len());
    v.extend_from_slice(prefix);
    v.extend_from_slice(name);
    Cow::Owned(v)
}

/// An ARM64EC symbol's demangled name: without the `#` in front, or a C++
/// name without its `$$h`. Others are unchanged.
pub fn ec_demangle(name: &[u8]) -> Cow<'_, [u8]> {
    if let Some(rest) = name.strip_prefix(b"#") {
        return Cow::Borrowed(rest);
    }
    if name.first() == Some(&b'?')
        && let Some(at) = name.windows(3).position(|w| w == b"$$h")
    {
        let mut v = name[..at].to_vec();
        v.extend_from_slice(&name[at + 3..]);
        return Cow::Owned(v);
    }
    Cow::Borrowed(name)
}

/// An ARM64EC symbol's mangled name: `#` in front, or for a C++ function's
/// name, `$$h` after its qualified name (the first `@@`, when a function's
/// type follows: an upper-case letter). A name already mangled is
/// unchanged; `None` for a C++ name that isn't a function's.
pub fn ec_mangle(name: &[u8]) -> Option<Cow<'_, [u8]>> {
    match name.first() {
        Some(b'#') => Some(Cow::Borrowed(name)),
        Some(b'?') => {
            if name.windows(3).any(|w| w == b"$$h") {
                return Some(Cow::Borrowed(name));
            }
            let at = name.windows(2).position(|w| w == b"@@")? + 2;
            if !name.get(at)?.is_ascii_uppercase() {
                return None;
            }
            let mut v = name[..at].to_vec();
            v.extend_from_slice(b"$$h");
            v.extend_from_slice(&name[at..]);
            Some(Cow::Owned(v))
        }
        _ => Some(concat(b"#", name)),
    }
}

/// An import object's symbols that `tool` lists, in the order `llvm-lib`
/// lists them.
pub fn import_symbols<'a>(
    import: &ImportObject<'a>,
    tool: Tool,
    ec_archive: bool,
) -> MemberSymbols<'a> {
    let s = import.symbol;
    let ec = import.machine == machine::ARM64EC;
    let symbols: Vec<Cow<'a, [u8]>> = if ec {
        let d = ec_demangle(s);
        match import.kind {
            ImportType::Data => alloc::vec![concat(b"__imp_", s)],
            kind => {
                let mut v = Vec::with_capacity(4);
                let mangled = match (kind, tool) {
                    (ImportType::Const, Tool::LlvmLib) => None,
                    (ImportType::Const, Tool::LibExe) => {
                        Some(ec_mangle(s).unwrap_or(Cow::Borrowed(&[])))
                    }
                    _ => ec_mangle(s),
                };
                if let Some(m) = mangled {
                    v.push(m);
                }
                v.push(concat(b"__imp_", &d));
                v.push(concat(b"__imp_aux_", &d));
                v.push(d);
                v
            }
        }
    } else {
        match import.kind {
            ImportType::Data => alloc::vec![concat(b"__imp_", s)],
            _ => alloc::vec![concat(b"__imp_", s), Cow::Borrowed(s)],
        }
    };
    let symbols: Vec<MemberSymbol<'a>> = symbols.into_iter().map(MemberSymbol::defined).collect();
    if ec_archive && is_ec_machine(import.machine) {
        MemberSymbols {
            symbols: Vec::new(),
            ec_symbols: symbols,
        }
    } else {
        MemberSymbols {
            symbols,
            ec_symbols: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
