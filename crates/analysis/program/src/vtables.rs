//! The virtual function tables of a program (MSVC's `` `vftable' ``s): each
//! one's slots, in order, and the functions they point to, by name.
//!
//! The slots are the pointer slots the image's base relocations name, so a
//! table is read without its type: from public symbols alone (a stripped
//! PDB's, as Microsoft's symbol server serves), where a table's symbol has
//! no size. Its length is then inferred: the slots that follow it, one
//! after another, each holding an address in code, up to the next symbol
//! (or another COM object's `QueryInterface`, which starts a table). That
//! is an upper bound: a table with no symbol of its own (a lambda's
//! `std::function`'s, in an image built without RTTI) that follows one
//! directly is read as more of it.
//!
//! Identical functions folded into one (`/OPT:ICF`) leave a slot with
//! several names. Names of the table's own class come first; when more than
//! one of them is left, the slot is one of them, and which one the image
//! doesn't say.

use turbonop_image::PointerSlot;
use turbonop_size::symbols::{Symbol, SymbolKind};

use crate::Program;

/// A virtual function table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vtable {
    /// Its symbol's name (``const CC2::XObjHandler10::`vftable' ``).
    pub name: String,
    /// Other names of the same table (tables folded into it).
    pub aliases: Vec<String>,
    /// The class whose table it is (`CC2::XObjHandler10`), and, for a
    /// class with more than one, the base it is for (`IObjFile2`).
    pub class: String,
    /// For a class with more than one table: the base this one is for.
    pub for_base: Option<String>,
    /// Its address, in the image's convention (an RVA for PE).
    pub rva: u64,
    /// The section it is in.
    pub section: String,
    /// Whether its length is the debug information's (else inferred).
    pub sized: bool,
    /// Its slots, in order.
    pub slots: Vec<Slot>,
}

/// One slot of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// From the start of the table.
    pub offset: u64,
    /// The address it holds (in the image's convention: an RVA for PE).
    pub target: u64,
    /// The names of what is there: of the table's own class first (`own`
    /// of them), then the rest.
    pub names: Vec<String>,
    /// How many of `names` are of the table's class.
    pub own: usize,
}

impl Slot {
    /// The one name the slot certainly has, if the image says.
    pub fn name(&self) -> Option<&str> {
        match (self.own, self.names.len()) {
            (1, _) | (0, 1) => self.names.first().map(String::as_str),
            _ => None,
        }
    }
}

/// The class a table's name is of, and the base it is for: ``const
/// A::B::`vftable'{for `C'} `` is `A::B`'s, for `C`.
pub fn class_of(name: &str) -> Option<(String, Option<String>)> {
    let name = name.strip_prefix("const ").unwrap_or(name);
    let at = name.find("::`vftable'")?;
    let class = name[..at].to_string();
    let rest = &name[at + "::`vftable'".len()..];
    let base = rest
        .strip_prefix("{for `")
        .and_then(|r| r.strip_suffix("'}"))
        .map(str::to_string);
    Some((class, base))
}

/// Whether `name` (a function's, demangled) is a member of `class`.
fn is_member(name: &str, class: &str) -> bool {
    let want = format!("{class}::");
    name.match_indices(&want).any(|(i, _)| {
        // Preceded by a space, a backtick, or nothing: `A::B::f`, not
        // `XA::B::f`; and not a template argument's (`<A::B::f>`).
        let before = name[..i].chars().last();
        let qualified = matches!(before, None | Some(' ' | '`' | '*' | '&'));
        let head = &name[..i];
        let open = head.matches('<').count();
        qualified && open <= head.matches('>').count()
    })
}

/// Whether `name` is a COM `QueryInterface` (or an adjustor thunk to one).
fn is_query_interface(name: &str) -> bool {
    name.contains("::QueryInterface(") || name.contains("::QueryInterface`adjustor")
}

/// A section, for telling code from data.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    /// Where it starts.
    pub addr: u64,
    /// Its size (in memory, or in the file if more).
    pub size: u64,
    /// Whether it holds code.
    pub executable: bool,
}

/// Whether a symbol's kind can be a table's.
fn is_table_kind(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Data | SymbolKind::Public)
}

/// The tables among `symbols`, their slots read from `slots` (sorted by
/// address), code told by `sections`.
pub fn find(symbols: &[Symbol], slots: &[PointerSlot], sections: &[(String, Span)]) -> Vec<Vtable> {
    // Every name at each address (folded ones too), for naming targets.
    let mut at: Vec<(u64, &Symbol)> = symbols.iter().map(|s| (s.addr, s)).collect();
    at.sort_by_key(|(a, s)| (*a, s.folded));
    // Where symbols start (any of them), for bounding unsized tables.
    let mut starts: Vec<u64> = symbols
        .iter()
        .filter(|s| !s.folded)
        .map(|s| s.addr)
        .collect();
    starts.sort_unstable();
    starts.dedup();
    let section_of = |addr: u64| {
        sections
            .iter()
            .find(|(_, s)| addr >= s.addr && addr - s.addr < s.size.max(1))
    };
    let in_code = |addr: u64| section_of(addr).is_some_and(|(_, s)| s.executable);

    // The tables, one per address: the unfolded name, the rest aliases.
    let mut tables: Vec<&Symbol> = symbols
        .iter()
        .filter(|s| is_table_kind(s.kind) && class_of(&s.name).is_some())
        .collect();
    tables.sort_by(|a, b| (a.addr, a.folded, &a.name).cmp(&(b.addr, b.folded, &b.name)));
    let mut out: Vec<Vtable> = Vec::new();
    for t in tables {
        if let Some(last) = out.last_mut().filter(|v| v.rva == t.addr) {
            last.aliases.push(t.name.clone());
            continue;
        }
        let Some((class, for_base)) = class_of(&t.name) else {
            continue;
        };
        let size = t.size.vm.max(t.size.file);
        let sized = size > 0;
        let section = section_of(t.addr);
        let end = if sized {
            t.addr + size
        } else {
            // The next symbol, else the section's end.
            let next = starts.partition_point(|&a| a <= t.addr);
            let section_end = section.map_or(u64::MAX, |(_, s)| s.addr + s.size);
            starts
                .get(next)
                .copied()
                .unwrap_or(u64::MAX)
                .min(section_end)
        };
        let mut found = Vec::new();
        let mut expect = t.addr;
        for slot in &slots[slots.partition_point(|p| p.addr < t.addr)..] {
            let w = u64::from(slot.width);
            if slot.addr != expect || slot.addr + w > end {
                break;
            }
            let Some(target) = slot.target else { break };
            if !in_code(target) {
                break;
            }
            expect += w;
            // Its names: the table's class's first.
            let first = at.partition_point(|(a, _)| *a < target);
            let mut own = Vec::new();
            let mut other = Vec::new();
            for (_, s) in at[first..].iter().take_while(|(a, _)| *a == target) {
                if !matches!(
                    s.kind,
                    SymbolKind::Function
                        | SymbolKind::PrimaryBlock
                        | SymbolKind::Thunk
                        | SymbolKind::Public
                ) {
                    continue;
                }
                let list = if is_member(&s.name, &class) {
                    &mut own
                } else {
                    &mut other
                };
                if !list.contains(&s.name) {
                    list.push(s.name.clone());
                }
            }
            // An inferred table ends where another begins: a COM object's
            // `QueryInterface` is always its table's first slot.
            if !sized
                && !found.is_empty()
                && own.iter().chain(&other).any(|n| is_query_interface(n))
            {
                break;
            }
            let n = own.len();
            own.extend(other);
            found.push(Slot {
                offset: slot.addr - t.addr,
                target,
                names: own,
                own: n,
            });
        }
        out.push(Vtable {
            name: t.name.clone(),
            aliases: Vec::new(),
            class,
            for_base,
            rva: t.addr,
            section: section.map(|(n, _)| n.clone()).unwrap_or_default(),
            sized,
            slots: found,
        });
    }
    out
}

impl Program<'_> {
    /// Its virtual function tables, by address.
    ///
    /// # Errors
    ///
    /// If the debug information is damaged.
    pub fn vtables(&self) -> Result<Vec<Vtable>, crate::Error> {
        let symbols = self.symbols()?;
        let sections: Vec<(String, Span)> = self
            .model()?
            .sections
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    Span {
                        addr: s.addr,
                        size: s.size.vm.max(s.size.file),
                        executable: s.executable(),
                    },
                )
            })
            .collect();
        let mut slots = self.image.pointer_slots();
        slots.sort_by_key(|p| p.addr);
        Ok(find(symbols, &slots, &sections))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbonop_size::Size;

    fn sym(addr: u64, size: u64, kind: SymbolKind, name: &str, folded: bool) -> Symbol {
        Symbol {
            addr,
            size: Size {
                file: size,
                vm: size,
            },
            kind,
            folded,
            name: name.into(),
            compiland: None,
            file_static: false,
        }
    }

    fn slot(addr: u64, target: u64) -> PointerSlot {
        PointerSlot {
            addr,
            width: 8,
            target: Some(target),
        }
    }

    fn sections() -> Vec<(String, Span)> {
        vec![
            (
                ".text".into(),
                Span {
                    addr: 0x1000,
                    size: 0x1000,
                    executable: true,
                },
            ),
            (
                ".rdata".into(),
                Span {
                    addr: 0x2000,
                    size: 0x1000,
                    executable: false,
                },
            ),
        ]
    }

    #[test]
    fn classes_of_table_names() {
        assert_eq!(
            class_of("const CC2::XObjHandler10::`vftable'"),
            Some(("CC2::XObjHandler10".into(), None))
        );
        assert_eq!(
            class_of("const CObjFileT<IMAGE_FILE>::`vftable'{for `IObjFile2'}"),
            Some(("CObjFileT<IMAGE_FILE>".into(), Some("IObjFile2".into())))
        );
        assert_eq!(class_of("Square::area"), None);
    }

    #[test]
    fn members_by_their_qualified_names() {
        let f = "public: virtual long __cdecl CC2::XObjHandler10::Init(struct IUnknown *)";
        assert!(is_member(f, "CC2::XObjHandler10"));
        assert!(!is_member(f, "XObjHandler10"));
        assert!(!is_member(f, "CC2::XObjHandler1"));
        assert!(!is_member("void f<A::B>(void)", "A"));
    }

    /// A stripped PDB's table: no size, so its slots are those that follow
    /// it holding addresses in code, up to the next symbol (or the first
    /// that doesn't).
    #[test]
    fn an_unsized_table_runs_to_the_first_slot_not_in_code() {
        let symbols = [
            sym(0x2000, 0, SymbolKind::Public, "const C::`vftable'", false),
            sym(
                0x1000,
                0,
                SymbolKind::Public,
                "public: virtual long C::QueryInterface(void)",
                false,
            ),
            sym(
                0x1010,
                0,
                SymbolKind::Public,
                "public: virtual long C::Init(int)",
                false,
            ),
            // Folded: two of C's, one of another class.
            sym(
                0x1020,
                0,
                SymbolKind::Public,
                "public: virtual long C::Stub1(void)",
                false,
            ),
            sym(
                0x1020,
                0,
                SymbolKind::Public,
                "public: virtual long C::Stub2(void)",
                true,
            ),
            sym(
                0x1020,
                0,
                SymbolKind::Public,
                "public: virtual long D::Other(void)",
                true,
            ),
            sym(0x1030, 0, SymbolKind::Public, "long helper(void)", false),
            sym(0x2040, 0, SymbolKind::Public, "const D::`vftable'", false),
        ];
        let slots = [
            slot(0x2000, 0x1000),
            slot(0x2008, 0x1010),
            slot(0x2010, 0x1020),
            slot(0x2018, 0x1030),
            // The next table's locator: data, so C's table ends before it.
            slot(0x2020, 0x2800),
            slot(0x2040, 0x1020),
        ];
        let t = find(&symbols, &slots, &sections());
        assert_eq!(t.len(), 2);
        let c = &t[0];
        assert!(!c.sized);
        assert_eq!(c.class, "C");
        assert_eq!(c.section, ".rdata");
        let names: Vec<Option<&str>> = c.slots.iter().map(Slot::name).collect();
        assert_eq!(
            names,
            [
                Some("public: virtual long C::QueryInterface(void)"),
                Some("public: virtual long C::Init(int)"),
                None,
                Some("long helper(void)"),
            ]
        );
        // The folded slot: C's two names first, then D's.
        assert_eq!(c.slots[2].own, 2);
        assert_eq!(c.slots[2].names.len(), 3);
        assert_eq!(c.slots[2].offset, 0x10);
        // D's table: the folded slot is D's, by its class.
        assert_eq!(t[1].slots.len(), 1);
        assert_eq!(
            t[1].slots[0].name(),
            Some("public: virtual long D::Other(void)")
        );
    }

    /// A table with no symbol right after an unsized one: it reads as
    /// more of the first, unless its first slot is a `QueryInterface`.
    #[test]
    fn an_unsized_table_ends_at_another_objects_query_interface() {
        let symbols = [
            sym(0x2000, 0, SymbolKind::Public, "const C::`vftable'", false),
            sym(
                0x1000,
                0,
                SymbolKind::Public,
                "public: virtual long C::QueryInterface(struct _GUID const &,void * *)",
                false,
            ),
            sym(
                0x1010,
                0,
                SymbolKind::Public,
                "public: virtual unsigned long C::AddRef(void)",
                false,
            ),
            sym(
                0x1020,
                0,
                SymbolKind::Public,
                "public: virtual long E::QueryInterface(struct _GUID const &,void * *)",
                false,
            ),
        ];
        let slots = [
            slot(0x2000, 0x1000),
            slot(0x2008, 0x1010),
            slot(0x2010, 0x1020),
        ];
        let t = find(&symbols, &slots, &sections());
        assert_eq!(t[0].slots.len(), 2);
    }

    #[test]
    fn a_sized_table_and_its_aliases() {
        let symbols = [
            sym(0x2000, 16, SymbolKind::Data, "A::`vftable'", false),
            sym(0x2000, 0, SymbolKind::Data, "B::`vftable'", true),
            sym(0x1000, 8, SymbolKind::Function, "A::f", false),
            sym(0x1008, 8, SymbolKind::Function, "A::g", false),
        ];
        // A third slot past the table's size isn't its.
        let slots = [
            slot(0x2000, 0x1000),
            slot(0x2008, 0x1008),
            slot(0x2010, 0x1000),
        ];
        let t = find(&symbols, &slots, &sections());
        assert_eq!(t.len(), 1);
        assert!(t[0].sized);
        assert_eq!(t[0].aliases, ["B::`vftable'"]);
        assert_eq!(t[0].slots.len(), 2);
        assert_eq!(t[0].slots[1].name(), Some("A::g"));
    }
}
