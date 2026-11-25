//! Editing: adding and removing sections and symbols, with every reference
//! to them renumbered.
//!
//! References are kept by position ([`SectionId`], [`SymbolId`]), so an
//! edit that moves sections or symbols rewrites every reference the model
//! holds: symbols' section numbers, relocations' symbols, line numbers'
//! functions, and the typed auxiliary records' symbols and sections
//! (function definitions, `.bf`, weak externals' defaults, section
//! definitions' associated sections, CLR tokens).
//!
//! What the model doesn't see isn't rewritten:
//!
//! - [`Aux::Raw`] records, which are kept as bytes;
//! - section data that holds symbol table indices: `.sxdata`, and the
//!   `/guard` tables (`.gfids$y`, `.giats$y`, `.gljmp$y`, `.gehcont$y`).
//!   [`Renumbering::table_index`] maps their entries.
//!
//! A structural edit forgets the layout the object was read with (see
//! [`Object::forget_layout`]), so the object is written afresh.

use alloc::vec::Vec;
use core::fmt;

use crate::object::{Aux, LineNumber, Object, Section, SectionId, SectionNumber, Symbol, SymbolId};

/// Why an edit was refused: something that stays refers to something that
/// goes. The object is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    /// A symbol that stays is defined in a section that goes.
    SymbolInRemovedSection {
        /// The symbol.
        symbol: SymbolId,
        /// Its section.
        section: SectionId,
    },
    /// A relocation or line number of a section that stays names a symbol
    /// that goes.
    SectionRefersToRemovedSymbol {
        /// The section.
        section: SectionId,
        /// The symbol.
        symbol: SymbolId,
    },
    /// The auxiliary record of a symbol that stays names a symbol that
    /// goes.
    AuxRefersToRemovedSymbol {
        /// The symbol whose record it is.
        symbol: SymbolId,
        /// The symbol it names.
        target: SymbolId,
    },
    /// The auxiliary record of a symbol that stays names a section that
    /// goes.
    AuxRefersToRemovedSection {
        /// The symbol whose record it is.
        symbol: SymbolId,
        /// The section it names.
        section: SectionId,
    },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::SymbolInRemovedSection { symbol, section } => write!(
                f,
                "symbol {} is defined in section {}, which is removed",
                symbol.0,
                section.number()
            ),
            EditError::SectionRefersToRemovedSymbol { section, symbol } => write!(
                f,
                "section {} refers to symbol {}, which is removed",
                section.number(),
                symbol.0
            ),
            EditError::AuxRefersToRemovedSymbol { symbol, target } => write!(
                f,
                "symbol {}'s auxiliary record names symbol {}, which is removed",
                symbol.0, target.0
            ),
            EditError::AuxRefersToRemovedSection { symbol, section } => write!(
                f,
                "symbol {}'s auxiliary record names section {}, which is removed",
                symbol.0,
                section.number()
            ),
        }
    }
}

impl core::error::Error for EditError {}

/// How an edit renumbered sections and symbols: where each one that stayed
/// went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renumbering {
    sections: Vec<Option<SectionId>>,
    symbols: Vec<Option<SymbolId>>,
    /// Each old symbol's table index, in order.
    old_indices: Vec<u32>,
    /// Each new symbol's table index.
    new_indices: Vec<u32>,
}

impl Renumbering {
    /// Where section `old` went, or `None` if it was removed.
    pub fn section(&self, old: SectionId) -> Option<SectionId> {
        self.sections.get(old.index()).copied().flatten()
    }

    /// Where symbol `old` went, or `None` if it was removed.
    pub fn symbol(&self, old: SymbolId) -> Option<SymbolId> {
        self.symbols.get(old.index()).copied().flatten()
    }

    /// The new symbol table index of the symbol whose primary record was
    /// at `old` in the table, or `None` if it was removed (or `old` was no
    /// symbol's): what an entry of `.sxdata` or a `/guard` table becomes.
    pub fn table_index(&self, old: u32) -> Option<u32> {
        let id = self.old_indices.binary_search(&old).ok()?;
        let new = self.symbols[id]?;
        Some(self.new_indices[new.index()])
    }
}

/// Each symbol's table index.
fn table_indices(symbols: &[Symbol<'_>], stride: usize) -> Vec<u32> {
    let mut at = 0u32;
    symbols
        .iter()
        .map(|s| {
            let index = at;
            at = at.wrapping_add(1 + s.aux.count(stride) as u32);
            index
        })
        .collect()
}

impl<'data> Object<'data> {
    /// The symbol table index of symbol `id`'s primary record.
    pub fn symbol_table_index(&self, id: SymbolId) -> Option<u32> {
        let stride = self.format.record_size();
        let mut at = 0u32;
        for s in self.symbols.get(..id.index())? {
            at = at.checked_add(1 + s.aux.count(stride) as u32)?;
        }
        (id.index() < self.symbols.len()).then_some(at)
    }

    /// Every symbol's table index, in order.
    pub fn symbol_table_indices(&self) -> Vec<u32> {
        table_indices(&self.symbols, self.format.record_size())
    }

    /// The symbol whose primary record is at `index` in the symbol table.
    pub fn symbol_at_table_index(&self, index: u32) -> Option<SymbolId> {
        let indices = self.symbol_table_indices();
        indices
            .binary_search(&index)
            .ok()
            .map(|i| SymbolId(i as u32))
    }

    /// The symbol that defines section `id`: the first symbol in it with a
    /// section definition.
    pub fn section_symbol(&self, id: SectionId) -> Option<SymbolId> {
        self.symbols
            .iter()
            .position(|s| {
                s.section == SectionNumber::Section(id) && s.section_definition().is_some()
            })
            .map(|i| SymbolId(i as u32))
    }

    /// Sets each section definition's length and counts from its section:
    /// the size, and the numbers of relocations and line numbers (capped
    /// at 0xFFFF, as MSVC and LLVM write them). Checksums are left as they
    /// are (see [`checksum`](crate::checksum)).
    pub fn sync_section_definitions(&mut self) {
        let sections = &self.sections;
        for s in &mut self.symbols {
            let SectionNumber::Section(id) = s.section else {
                continue;
            };
            let (Some(section), Aux::Section(d)) = (sections.get(id.index()), &mut s.aux) else {
                continue;
            };
            d.length = section.size();
            d.relocations = u16::try_from(section.relocations.len()).unwrap_or(u16::MAX);
            d.line_numbers = u16::try_from(section.line_numbers.len()).unwrap_or(u16::MAX);
        }
    }

    /// Appends a section, and gives its id.
    pub fn push_section(&mut self, section: Section<'data>) -> SectionId {
        self.forget_layout();
        self.sections.push(section);
        SectionId(self.sections.len() as u32 - 1)
    }

    /// Appends a symbol, and gives its id.
    pub fn push_symbol(&mut self, symbol: Symbol<'data>) -> SymbolId {
        self.forget_layout();
        self.symbols.push(symbol);
        SymbolId(self.symbols.len() as u32 - 1)
    }

    /// Inserts a section at `at` (at most the number of sections),
    /// renumbering the sections after it and every reference to them. The
    /// new section's own references are taken as they are, in the new
    /// numbering.
    pub fn insert_section(&mut self, at: SectionId, section: Section<'data>) -> SectionId {
        let at = at.0.min(self.sections.len() as u32);
        self.remap(
            |s| Some(if s.0 >= at { SectionId(s.0 + 1) } else { s }),
            Some,
        );
        self.sections.insert(at as usize, section);
        self.forget_layout();
        SectionId(at)
    }

    /// Inserts a symbol at `at` (at most the number of symbols),
    /// renumbering the symbols after it and every reference to them. The
    /// new symbol's own references are taken as they are, in the new
    /// numbering.
    pub fn insert_symbol(&mut self, at: SymbolId, symbol: Symbol<'data>) -> SymbolId {
        let at = at.0.min(self.symbols.len() as u32);
        self.remap(Some, |s| {
            Some(if s.0 >= at { SymbolId(s.0 + 1) } else { s })
        });
        self.symbols.insert(at as usize, symbol);
        self.forget_layout();
        SymbolId(at)
    }

    /// Removes the sections for which `keep_section` is false and the
    /// symbols for which `keep_symbol` is false, renumbering the rest and
    /// every reference to them.
    ///
    /// Refused, with the object unchanged, if something that stays refers
    /// to something that goes: a symbol defined in a removed section, a
    /// relocation to a removed symbol, an auxiliary record naming either.
    /// (A symbol removed with its section, such as a removed COMDAT's
    /// section symbol and leader, is no such reference; to keep a leader
    /// as an undefined external, make it one first.)
    pub fn retain(
        &mut self,
        mut keep_section: impl FnMut(SectionId, &Section<'data>) -> bool,
        mut keep_symbol: impl FnMut(SymbolId, &Symbol<'data>) -> bool,
    ) -> Result<Renumbering, EditError> {
        let stride = self.format.record_size();
        let mut sections = Vec::with_capacity(self.sections.len());
        let mut next = 0u32;
        for (i, s) in self.sections.iter().enumerate() {
            sections.push(keep_section(SectionId(i as u32), s).then(|| {
                next += 1;
                SectionId(next - 1)
            }));
        }
        let mut symbols = Vec::with_capacity(self.symbols.len());
        let mut next = 0u32;
        for (i, s) in self.symbols.iter().enumerate() {
            symbols.push(keep_symbol(SymbolId(i as u32), s).then(|| {
                next += 1;
                SymbolId(next - 1)
            }));
        }
        self.check_references(&sections, &symbols)?;

        let old_indices = table_indices(&self.symbols, stride);
        self.remap(
            |s| sections.get(s.index()).copied().flatten(),
            |s| symbols.get(s.index()).copied().flatten(),
        );
        let mut i = 0;
        self.sections.retain(|_| {
            i += 1;
            sections[i - 1].is_some()
        });
        let mut i = 0;
        self.symbols.retain(|_| {
            i += 1;
            symbols[i - 1].is_some()
        });
        self.forget_layout();
        Ok(Renumbering {
            sections,
            symbols,
            old_indices,
            new_indices: table_indices(&self.symbols, stride),
        })
    }

    /// Removes the sections for which `remove` is true (see
    /// [`retain`](Self::retain)), with the symbols defined in them.
    pub fn remove_sections(
        &mut self,
        mut remove: impl FnMut(SectionId, &Section<'data>) -> bool,
    ) -> Result<Renumbering, EditError> {
        let removed: Vec<bool> = self
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| remove(SectionId(i as u32), s))
            .collect();
        self.retain(
            |id, _| !removed[id.index()],
            |_, s| !s.section.id().is_some_and(|id| removed[id.index()]),
        )
    }

    /// Removes the symbols for which `remove` is true (see
    /// [`retain`](Self::retain)).
    pub fn remove_symbols(
        &mut self,
        mut remove: impl FnMut(SymbolId, &Symbol<'data>) -> bool,
    ) -> Result<Renumbering, EditError> {
        self.retain(|_, _| true, |id, s| !remove(id, s))
    }

    /// Checks that nothing kept refers to something removed (or to
    /// something that doesn't exist).
    fn check_references(
        &self,
        sections: &[Option<SectionId>],
        symbols: &[Option<SymbolId>],
    ) -> Result<(), EditError> {
        let kept_section = |id: SectionId| sections.get(id.index()).copied().flatten().is_some();
        let kept_symbol = |id: SymbolId| symbols.get(id.index()).copied().flatten().is_some();
        for (i, s) in self.sections.iter().enumerate() {
            if sections[i].is_none() {
                continue;
            }
            let section = SectionId(i as u32);
            let lines = s.line_numbers.iter().filter_map(|l| match *l {
                LineNumber::Function(id) => Some(id),
                LineNumber::Line { .. } => None,
            });
            for symbol in s.relocations.iter().map(|r| r.symbol).chain(lines) {
                if !kept_symbol(symbol) {
                    return Err(EditError::SectionRefersToRemovedSymbol { section, symbol });
                }
            }
        }
        for (i, s) in self.symbols.iter().enumerate() {
            if symbols[i].is_none() {
                continue;
            }
            let symbol = SymbolId(i as u32);
            if let SectionNumber::Section(section) = s.section
                && !kept_section(section)
            {
                return Err(EditError::SymbolInRemovedSection { symbol, section });
            }
            let (targets, section): ([Option<SymbolId>; 2], Option<SectionId>) = match &s.aux {
                Aux::Function(f) => ([f.tag, f.next], f.line_numbers.map(|l| l.section)),
                Aux::BeginEnd(b) => ([b.next, None], None),
                Aux::WeakExternal(w) => ([Some(w.tag), None], None),
                Aux::Section(d) => ([None, None], d.number),
                Aux::ClrToken(c) => ([Some(c.symbol), None], None),
                Aux::None | Aux::File(_) | Aux::Raw(_) => ([None, None], None),
            };
            for target in targets.into_iter().flatten() {
                if !kept_symbol(target) {
                    return Err(EditError::AuxRefersToRemovedSymbol { symbol, target });
                }
            }
            if let Some(section) = section
                && !kept_section(section)
            {
                return Err(EditError::AuxRefersToRemovedSection { symbol, section });
            }
        }
        Ok(())
    }

    /// Rewrites every reference through `section` and `symbol` (which the
    /// caller has checked give `Some` for every reference that stays).
    fn remap(
        &mut self,
        section: impl Fn(SectionId) -> Option<SectionId>,
        symbol: impl Fn(SymbolId) -> Option<SymbolId>,
    ) {
        let sym = |id: &mut SymbolId| {
            if let Some(new) = symbol(*id) {
                *id = new;
            }
        };
        let sec = |id: &mut SectionId| {
            if let Some(new) = section(*id) {
                *id = new;
            }
        };
        for s in &mut self.sections {
            for r in &mut s.relocations {
                sym(&mut r.symbol);
            }
            for l in &mut s.line_numbers {
                if let LineNumber::Function(id) = l {
                    sym(id);
                }
            }
        }
        for s in &mut self.symbols {
            if let SectionNumber::Section(id) = &mut s.section {
                sec(id);
            }
            match &mut s.aux {
                Aux::Function(f) => {
                    if let Some(id) = &mut f.tag {
                        sym(id);
                    }
                    if let Some(id) = &mut f.next {
                        sym(id);
                    }
                    if let Some(l) = &mut f.line_numbers {
                        sec(&mut l.section);
                    }
                }
                Aux::BeginEnd(b) => {
                    if let Some(id) = &mut b.next {
                        sym(id);
                    }
                }
                Aux::WeakExternal(w) => sym(&mut w.tag),
                Aux::Section(d) => {
                    if let Some(id) = &mut d.number {
                        sec(id);
                    }
                }
                Aux::ClrToken(c) => sym(&mut c.symbol),
                Aux::None | Aux::File(_) | Aux::Raw(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
