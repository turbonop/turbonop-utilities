//! `/GUARD:CF`'s tables, as link.exe builds them: the control-flow guard
//! function table (`.gfids`: every function whose address the program
//! takes, as the objects' `.gfids$y` sections list them, and the entry
//! point), the address-taken import table (`.giats`, from `.giats$y`), the
//! long-jump target table (`.gljmp`, from `.gljmp$y`), and with
//! `/GUARD:EHCONT` the EH continuation table (`.gehcont`, from
//! `.gehcont$y`). Each table is its targets' RVAs, sorted, each once (4
//! bytes each); the load configuration finds them through the
//! `__guard_*` symbols link.exe defines.
//!
//! An object not compiled with `/guard:cf` (`@feat.00` without 0x800, or
//! none) lists nothing, so link.exe takes as targets every function it
//! might take the address of: each function a `REL32` or `ADDR64`
//! relocation of it refers to (from any of its sections in the image),
//! calls included, as the function's definition says (typed a function, in
//! a code section; an import's thunk). Other relocations (`ADDR32NB`, as
//! unwind data's, `REL32_1` to `_5`, `SECREL`, `SECTION`) don't make
//! targets, nor does a symbol typed a function where it's referred to but
//! not where it's defined.

use crate::object::{Selection, SymbolKind, scn};
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
    chunk_of: &(dyn Fn(u32, u32) -> u32 + Sync),
    thunk_of: &(dyn Fn(u32) -> Option<u32> + Sync),
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
    // The functions objects not compiled with /guard:cf may take the
    // address of.
    for p in unguarded(r, chunk_of, &global) {
        if seen[0].insert(p) {
            t.functions.push(p);
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

/// Whether an object was compiled with `/guard:cf` (`@feat.00`'s 0x800).
fn guarded(object: &crate::object::Object) -> bool {
    object.feat_00.is_some_and(|x| x & 0x800 != 0)
}

/// Whether a symbol is defined as a function: typed one, in a code
/// section.
fn is_function(object: &crate::object::Object, ty: u16, section: u32) -> bool {
    ty & 0x30 == 0x20
        && object
            .sections
            .get(section as usize)
            .is_some_and(|s| s.characteristics & (scn::CNT_CODE | scn::MEM_EXECUTE) != 0)
}

/// The places of the functions the objects not compiled with `/guard:cf`
/// refer to by a `REL32` or `ADDR64` relocation (in no order, perhaps more
/// than once), `global` placing a global symbol.
fn unguarded(
    r: &Resolved,
    chunk_of: &(dyn Fn(u32, u32) -> u32 + Sync),
    global: &(dyn Fn(u32) -> Option<Place> + Sync),
) -> Vec<Place> {
    const ADDR64: u16 = 1;
    const REL32: u16 = 4;
    let files: Vec<u32> = r
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(&f.kind, FileKind::Object { object, .. } if !guarded(object)))
        .map(|(i, _)| crate::n32(i))
        .collect();
    if files.is_empty() {
        return Vec::new();
    }
    // Which globals are defined as functions: by the symbol that defines
    // each (where it's referred to, it may be typed otherwise), or an
    // import's thunk.
    let defined: Vec<Vec<u32>> = turbonop_par::map_range(r.files.len(), 16, |f| {
        let FileKind::Object {
            object, globals, ..
        } = &r.files[f].kind
        else {
            return Vec::new();
        };
        let mut mine = Vec::new();
        for (k, sym) in object.symbols.iter().enumerate() {
            if let SymbolKind::Defined { section, .. } = sym.kind
                && is_function(object, sym.ty, section)
                && let Some(&g) = globals.get(k)
                && g != NONE
                && matches!(r.globals[g as usize].def,
                    Def::Section { file, section: s, .. } if file as usize == f && s == section)
            {
                mine.push(g);
            }
        }
        mine
    });
    let mut function = vec![false; r.globals.len()];
    for g in defined.into_iter().flatten() {
        function[g as usize] = true;
    }
    for (g, x) in r.globals.iter().enumerate() {
        if matches!(x.def, Def::ImportThunk(_)) {
            function[g] = true;
        }
    }
    // A global, its aliases followed.
    let target = |mut g: u32| -> u32 {
        for _ in 0..64 {
            match r.globals[g as usize].def {
                Def::Alias(to) => g = to,
                _ => break,
            }
        }
        g
    };
    let found: Vec<Vec<Place>> = turbonop_par::map(&files, 1, |&f| {
        let FileKind::Object {
            object, globals, ..
        } = &r.files[f as usize].kind
        else {
            return Vec::new();
        };
        let mut mine = Vec::new();
        for (s, sec) in object.sections.iter().enumerate() {
            if !sec.is_image() || chunk_of(f, crate::n32(s)) == u32::MAX {
                continue;
            }
            for rel in sec.relocations {
                if !matches!(rel.kind.get(), ADDR64 | REL32) {
                    continue;
                }
                let k = rel.symbol_table_index.get();
                let place = match globals.get(k as usize) {
                    Some(&g) if g != NONE => {
                        let g = target(g);
                        if function[g as usize] {
                            global(g)
                        } else {
                            None
                        }
                    }
                    _ => match object.symbols.get(k as usize) {
                        Some(sym) => match sym.kind {
                            SymbolKind::Local { section, value }
                                if is_function(object, sym.ty, section) =>
                            {
                                let c = chunk_of(f, section);
                                (c != u32::MAX).then_some(Place::Chunk(c, value))
                            }
                            _ => None,
                        },
                        None => None,
                    },
                };
                if let Some(p @ Place::Chunk(..)) = place {
                    mine.push(p);
                }
            }
        }
        mine
    });
    found.into_iter().flatten().collect()
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
