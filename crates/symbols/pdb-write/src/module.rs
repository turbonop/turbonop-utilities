//! A module's stream: its symbols (renumbered, each 4-byte aligned, each
//! scope's `pParent` and `pEnd` set to where its records now are), its C13
//! subsections, and the records it gives the globals (data, typedefs and
//! constants at file scope; `S_PROCREF`s for its procedures), as link.exe
//! (and LLVM, in lld's `PDB.cpp`) route them.

use turbonop_codeview::symbol_refs;
use turbonop_codeview::symbols::kind::*;
use turbonop_codeview::type_refs::Ref;

/// A module being built.
#[derive(Debug, Default)]
pub struct ModuleStream {
    /// The symbols (the stream's first 4 bytes, the signature, aside:
    /// offsets here are the stream's minus 4).
    symbols: Vec<u8>,
    /// The C13 subsections.
    c13: Vec<u8>,
    /// Records for the globals stream.
    pub globals: Vec<Vec<u8>>,
    /// Open scopes: each one's record offset (in the stream).
    scopes: Vec<u32>,
    /// Kinds seen that are neither known to have no index fields nor
    /// known to have them (left as they were).
    pub unknown_kinds: Vec<u16>,
}

/// Whether a scope of `kind` has a `pNext` field (after `pEnd`).
fn has_next(kind: u16) -> bool {
    matches!(
        kind,
        S_GPROC32
            | S_LPROC32
            | S_GPROC32_ID
            | S_LPROC32_ID
            | S_LPROC32_DPC
            | S_LPROC32_DPC_ID
            | S_THUNK32
            | S_GMANPROC
            | S_LMANPROC
    )
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

impl ModuleStream {
    /// The offset (in the stream) the next record will have.
    fn next(&self) -> u32 {
        4 + u32::try_from(self.symbols.len()).expect("a module under 4 GB")
    }

    /// Adds the symbol records `data` (a `DEBUG_S_SYMBOLS` subsection's),
    /// with `map` renumbering a type (`false`) or id (`true`) index, for
    /// module `module` (0-based).
    pub fn add_symbols(
        &mut self,
        data: &[u8],
        map: &dyn Fn(u32, bool) -> u32,
        func_type: &dyn Fn(u32) -> u32,
        module: u16,
    ) {
        let mut at = 0usize;
        let mut refs: Vec<Ref> = Vec::new();
        while at + 4 <= data.len() {
            let len = usize::from(u16_at(data, at));
            let end = (at + 2 + len).min(data.len());
            if len < 2 {
                break;
            }
            let kind = u16_at(data, at + 2);
            let mut rec = data[at..end].to_vec();
            at = end;
            // Renumbered.
            refs.clear();
            // (A kind whose layout isn't known is kept as it was.)
            if symbol_refs::refs(kind, &rec[4..], &mut refs).is_err()
                && !self.unknown_kinds.contains(&kind)
            {
                self.unknown_kinds.push(kind);
            }
            for r in &refs {
                let o = 4 + r.offset as usize;
                let ti = u32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
                put32(&mut rec, o, map(ti, r.id));
            }
            // A procedure named by its id (S_GPROC32_ID) becomes one named
            // by its type, and S_PROC_ID_END an S_END, as link.exe writes
            // them (DIA reads only those).
            let kind = match kind {
                S_GPROC32_ID | S_LPROC32_ID | S_LPROC32_DPC_ID if rec.len() >= 32 => {
                    let id = u32::from_le_bytes([rec[28], rec[29], rec[30], rec[31]]);
                    put32(&mut rec, 28, func_type(id));
                    let k = match kind {
                        S_GPROC32_ID => S_GPROC32,
                        S_LPROC32_ID => S_LPROC32,
                        _ => S_LPROC32_DPC,
                    };
                    rec[2..4].copy_from_slice(&k.to_le_bytes());
                    k
                }
                S_PROC_ID_END => {
                    rec[2..4].copy_from_slice(&S_END.to_le_bytes());
                    S_END
                }
                k => k,
            };
            // Aligned to 4, the length with it.
            let n = rec.len().next_multiple_of(4);
            rec.resize(n, 0);
            let l = u16::try_from(n - 2).expect("a record under 64 KB");
            rec[..2].copy_from_slice(&l.to_le_bytes());
            let depth = self.scopes.len();
            match kind {
                S_GDATA32 | S_GTHREAD32 | S_PROCREF | S_LPROCREF | S_DATAREF if depth == 0 => {
                    self.globals.push(rec);
                    continue;
                }
                S_UDT | S_CONSTANT if depth == 0 => {
                    self.globals.push(rec);
                    continue;
                }
                S_LDATA32 | S_LTHREAD32 if depth == 0 => self.globals.push(rec.clone()),
                _ => {}
            }
            let offset = self.next();
            if opens_scope(kind) && rec.len() >= 12 {
                put32(&mut rec, 4, self.scopes.last().copied().unwrap_or(0));
                put32(&mut rec, 8, 0);
                if has_next(kind) && rec.len() >= 16 {
                    put32(&mut rec, 12, 0);
                }
                if depth == 0
                    && matches!(
                        kind,
                        S_GPROC32
                            | S_LPROC32
                            | S_GPROC32_ID
                            | S_LPROC32_ID
                            | S_LPROC32_DPC
                            | S_LPROC32_DPC_ID
                    )
                {
                    self.globals.push(procref(&rec, kind, offset, module));
                }
                self.scopes.push(offset);
            } else if closes_scope(kind)
                && let Some(open) = self.scopes.pop()
            {
                let o = (open - 4) as usize;
                put32(&mut self.symbols, o + 8, offset);
            }
            self.symbols.extend_from_slice(&rec);
        }
    }

    /// Adds a C13 subsection (`kind`, `data`), padded to 4.
    pub fn add_subsection(&mut self, kind: u32, data: &[u8]) {
        self.c13.extend_from_slice(&kind.to_le_bytes());
        self.c13
            .extend_from_slice(&u32::try_from(data.len()).expect("small").to_le_bytes());
        self.c13.extend_from_slice(data);
        while !self.c13.len().is_multiple_of(4) {
            self.c13.push(0);
        }
    }

    /// Adds one record built by the linker (`* Linker *`'s).
    pub fn add_record(&mut self, kind: u16, body: &[u8]) {
        let n = (4 + body.len()).next_multiple_of(4);
        let l = u16::try_from(n - 2).expect("small");
        self.symbols.extend_from_slice(&l.to_le_bytes());
        self.symbols.extend_from_slice(&kind.to_le_bytes());
        self.symbols.extend_from_slice(body);
        while !self.symbols.len().is_multiple_of(4) {
            self.symbols.push(0);
        }
    }

    /// Whether the module has nothing for a stream.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.c13.is_empty()
    }

    /// The symbols' size, with the signature (`SymByteSize`).
    pub fn sym_bytes(&self) -> u32 {
        self.next()
    }

    /// The C13 subsections' size.
    pub fn c13_bytes(&self) -> u32 {
        u32::try_from(self.c13.len()).expect("small")
    }

    /// The stream: signature (4, C13), symbols, C13 subsections, and an
    /// empty global references list.
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.symbols.len() + self.c13.len());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&self.symbols);
        out.extend_from_slice(&self.c13);
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }
}

/// `S_PROCREF` (or `S_LPROCREF`) for a procedure record `rec` at `offset`
/// in module `module`'s stream.
fn procref(rec: &[u8], kind: u16, offset: u32, module: u16) -> Vec<u8> {
    let body = &rec[4..];
    let r = turbonop_codeview::SymbolRecord {
        offset: 0,
        kind,
        data: body,
    };
    let name = r.name().unwrap_or_default();
    let refkind = if matches!(kind, S_GPROC32 | S_GPROC32_ID) {
        S_PROCREF
    } else {
        S_LPROCREF
    };
    let mut out = Vec::with_capacity(16 + name.len());
    let n = (4 + 4 + 4 + 2 + name.len() + 1).next_multiple_of(4);
    out.extend_from_slice(&u16::try_from(n - 2).expect("small").to_le_bytes());
    out.extend_from_slice(&refkind.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // SUC of the name
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&(module + 1).to_le_bytes());
    out.extend_from_slice(name);
    out.resize(n, 0);
    out
}
