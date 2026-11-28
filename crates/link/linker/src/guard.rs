//! `/GUARD:CF`'s tables, as link.exe builds them: the control-flow guard
//! function table (`.gfids`: every function whose address the program
//! takes, as the objects' `.gfids$y` sections list them, and the entry
//! point), the address-taken import table (`.giats`, from `.giats$y`), the
//! long-jump target table (`.gljmp`, from `.gljmp$y`), and with
//! `/GUARD:EHCONT` the EH continuation table (`.gehcont`, from
//! `.gehcont$y`). Each table is its targets' RVAs, sorted, each once (4
//! bytes each); the load configuration finds them through the
//! `__guard_*` symbols link.exe defines.

use crate::object::{Selection, SymbolKind};
use crate::resolve::{Def, FileKind, NONE, Resolved};

/// What the `__guard_*` symbols link.exe defines are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    /// A table's address (0 functions, 1 imports, 2 long jumps, 3 EH
    /// continuations; 0 if it is empty).
    Table(u8),
    /// A table's count of entries.
    Count(u8),
    /// `__guard_flags`.
    Flags,
}

impl Symbol {
    /// The symbol a name is, with `/GUARD:CF`.
    pub fn of(name: &[u8]) -> Option<Symbol> {
        Some(match name {
            b"__guard_fids_table" => Symbol::Table(0),
            b"__guard_fids_count" => Symbol::Count(0),
            b"__guard_iat_table" => Symbol::Table(1),
            b"__guard_iat_count" => Symbol::Count(1),
            b"__guard_longjmp_table" => Symbol::Table(2),
            b"__guard_longjmp_count" => Symbol::Count(2),
            b"__guard_eh_cont_table" => Symbol::Table(3),
            b"__guard_eh_cont_count" => Symbol::Count(3),
            b"__guard_flags" => Symbol::Flags,
            _ => return None,
        })
    }
}

/// The group (input section name) each table is in, as link.exe names it.
pub const GROUPS: [&[u8]; 4] = [b".gfids", b".giats", b".gljmp", b".gehcont"];

/// Where a table's entry points: a chunk and an offset in it, or an
/// import's address table slot (its import file).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Place {
    /// A chunk (its index) and an offset.
    Chunk(u32, u32),
    /// An import's slot (the import file).
    Slot(u32),
}

/// The tables' entries, each table's places once (in no order: they are
/// sorted by address when written).
#[derive(Clone, Debug, Default)]
pub struct Tables {
    /// The functions (`.gfids`).
    pub functions: Vec<Place>,
    /// The functions' metadata, by place (link.exe's `IMAGE_GUARD_FLAG_`
    /// bits: 2 for an exported function nothing else makes a target, its
    /// export suppressed); absent is 0.
    pub function_flags: turbonop_hash::Map<Place, u8>,
    /// The address-taken imports (`.giats`).
    pub imports: Vec<Place>,
    /// The long-jump targets (`.gljmp`).
    pub long_jumps: Vec<Place>,
    /// The EH continuation targets (`.gehcont`), with `/GUARD:EHCONT`.
    pub eh_continuations: Vec<Place>,
}

impl Tables {
    /// Table `i`'s entries (as `Symbol::Table` numbers them).
    pub fn table(&self, i: u8) -> &[Place] {
        match i {
            0 => &self.functions,
            1 => &self.imports,
            2 => &self.long_jumps,
            _ => &self.eh_continuations,
        }
    }
}

/// The tables' entries for `r` (with `/GUARD:CF`): `chunk_of(file,
/// section)` is a section's chunk (`u32::MAX` if not in the image),
/// `thunk_of(import)` an import's thunk's, `delay_thunks` the delay-loaded
/// imports' `__imp_load_` thunks' chunks.
pub fn tables(
    r: &Resolved,
    chunk_of: &dyn Fn(u32, u32) -> u32,
    thunk_of: &dyn Fn(u32) -> Option<u32>,
    delay_thunks: &[u32],
) -> Tables {
    let mut t = Tables::default();
    let mut seen: [turbonop_hash::Set<Place>; 4] = Default::default();
    // Where global `g` is.
    let global = |mut g: u32| -> Option<Place> {
        for _ in 0..64 {
            return match r.globals[g as usize].def {
                Def::Alias(to) => {
                    g = to;
                    continue;
                }
                Def::Section {
                    file,
                    section,
                    value,
                } => {
                    let c = chunk_of(file, section);
                    (c != u32::MAX).then_some(Place::Chunk(c, value))
                }
                Def::ImportThunk(f) => thunk_of(f).map(|c| Place::Chunk(c, 0)),
                Def::ImportAddress(f) => Some(Place::Slot(f)),
                _ => None,
            };
        }
        None
    };
    for (f, file) in r.files.iter().enumerate() {
        let FileKind::Object {
            object,
            globals,
            kept,
        } = &file.kind
        else {
            continue;
        };
        for (s, sec) in object.sections.iter().enumerate() {
            let which = match sec.name {
                b".gfids$y" => 0,
                b".giats$y" => 1,
                b".gljmp$y" if r.config.guard_longjmp => 2,
                b".gehcont$y" if r.config.guard_ehcont => 3,
                _ => continue,
            };
            // A table goes with what it describes: an associative one with
            // its function (in the image, or folded into one that is), the
            // rest with the object.
            let wanted = match sec.comdat {
                Some((Selection::Associative(parent), _)) => {
                    chunk_of(crate::n32(f), parent) != u32::MAX
                }
                _ => kept.get(s).copied().unwrap_or(false),
            };
            if !wanted {
                continue;
            }
            for &k in sec.data.as_chunks::<4>().0 {
                let k = u32::from_le_bytes(k);
                let place = match globals.get(k as usize) {
                    Some(&g) if g != NONE => global(g),
                    _ => match object.symbols.get(k as usize).map(|s| s.kind) {
                        Some(SymbolKind::Local { section, value }) => {
                            let c = chunk_of(crate::n32(f), section);
                            (c != u32::MAX).then_some(Place::Chunk(c, value))
                        }
                        _ => None,
                    },
                };
                if let Some(p) = place
                    && seen[which].insert(p)
                {
                    match which {
                        0 => t.functions.push(p),
                        1 => t.imports.push(p),
                        2 => t.long_jumps.push(p),
                        _ => t.eh_continuations.push(p),
                    }
                }
            }
        }
    }
    // The entry point is a valid target too; and with delay loading, its
    // helper (the tail merges call it).
    let helper = (!delay_thunks.is_empty())
        .then(|| r.names.get(&b"__delayLoadHelper2"[..]).copied())
        .flatten();
    for p in r.entry.into_iter().chain(helper).filter_map(global) {
        if seen[0].insert(p) {
            t.functions.push(p);
        }
    }
    // An exported function is too, its export suppressed (flag 2) unless
    // something else made it a target.
    for e in &r.config.exports {
        if e.data {
            continue;
        }
        let name = e.internal.as_deref().unwrap_or(&e.name);
        if let Some(&g) = r.names.get(name.as_bytes())
            && let Some(p) = global(g)
            && seen[0].insert(p)
        {
            t.functions.push(p);
            t.function_flags.insert(p, 2);
        }
    }
    t
}

/// The bytes of each function table entry: 4 (the RVA), and 5 when an
/// entry has metadata (then each has a byte of it).
pub fn function_entry_size(t: &Tables) -> u32 {
    if t.function_flags.is_empty() { 4 } else { 5 }
}

/// The load configuration's `__guard_flags` with `/GUARD:CF`, as link.exe
/// sets them: instrumented, the function table present, the delay-load IAT
/// protected and in its own section, export suppression information
/// present, the long-jump table present (and with `/GUARD:EHCONT` the EH
/// continuation table).
/// The function table's stride (its entries' metadata bytes) is the top
/// nibble.
pub fn flags(r: &Resolved, t: &Tables) -> u32 {
    let mut f = 0x0001_7500;
    // (Without the long-jump table with /GUARD:NOLONGJMP.)
    if !r.config.guard_longjmp {
        f &= !0x0001_0000;
    }
    if r.config.guard_ehcont {
        f |= 0x0040_0000;
    }
    (function_entry_size(t) - 4) << 28 | f
}
