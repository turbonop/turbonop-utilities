//! `.cil$sy`: each function's scopes and local symbols. A stream per IL
//! unit ([`super::IlUnits`]), in the units' order, each ending with tag
//! 0x06; a function's record ([`super::FunctionIl::symbols_offset`]) gives
//! where its stream starts. The records are those of `.cil$gl` (a local
//! variable's is a global's, its scope's number where a global has 0),
//! and three of their own: a scope (0x03), a scope's start (0x0D), and a
//! local static (0x07). See docs/ltcg-il.md.

use alloc::vec::Vec;
use turbonop_core::Result;

use super::Reader;
use super::globals::{Constant, NamedType, Traits, attribute_list, tag, traits};

/// A scope: the function's own (kind 0x1F), a block's (0x1E), a label
/// (0, named), and others (`try`, `__try`, a coroutine's: 0x08 to 0x48
/// seen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope<'data> {
    pub offset: usize,
    /// Its depth, it seems (1 the function's).
    pub level: u8,
    pub id: u32,
    pub kind: u8,
    /// A label's name (none for others).
    pub name: &'data [u8],
    /// Two bytes (what they count is not known: 1 and 1 for most).
    pub counts: [u8; 2],
}

/// What a local symbol is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind<'data> {
    /// A parameter or automatic variable (tag 0x01, or 0x02 an aggregate).
    Variable {
        /// 3 a parameter, 1 an automatic variable (as seen).
        class: u8,
        size: u32,
        type_index: u32,
        machine_type: u8,
        type_class: u8,
        /// An aggregate's alignment.
        alignment: Option<u8>,
        traits: Traits<'data>,
    },
    /// A temporary the compiler made (tag 0x1A), with no name.
    Unnamed {
        size: u32,
        type_index: u32,
        machine_type: u8,
        type_class: u8,
        traits: Traits<'data>,
    },
    /// A local static (tag 0x07): the global variable it is (by its id;
    /// its record in `.cil$gl`, if anything refers to it).
    Static {
        size: u32,
        type_index: u32,
        machine_type: u8,
        global: u32,
    },
}

/// A local symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local<'data> {
    pub offset: usize,
    /// The number of the scope it is in (its [`LocalItem::Begin`]).
    pub scope: u8,
    pub id: u32,
    pub flags: u8,
    /// Its name: undecorated for parameters and automatic variables,
    /// decorated for a local static.
    pub name: &'data [u8],
    pub kind: LocalKind<'data>,
}

/// One of a function's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalItem<'data> {
    Scope(Scope<'data>),
    /// A scope's start: its number (1 the parameters', 2 the function
    /// body's, and on in order).
    Begin(u8),
    Local(Local<'data>),
    Type(NamedType<'data>),
    Constant(Constant<'data>),
}

/// One function's stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionLocals<'data> {
    /// Where it starts and ends (past its end tag) in `.cil$sy`.
    pub offset: usize,
    pub end: usize,
    pub items: Vec<LocalItem<'data>>,
}

impl<'data> FunctionLocals<'data> {
    /// Its local symbols.
    pub fn locals(&self) -> impl Iterator<Item = &Local<'data>> + '_ {
        self.items.iter().filter_map(|i| match i {
            LocalItem::Local(l) => Some(l),
            _ => None,
        })
    }

    /// Its function scope's id (the first scope).
    pub fn scope_id(&self) -> Option<u32> {
        self.items.iter().find_map(|i| match i {
            LocalItem::Scope(s) => Some(s.id),
            _ => None,
        })
    }
}

/// `.cil$sy`, read.
#[derive(Clone, Debug)]
pub struct Locals<'data> {
    pub functions: Vec<FunctionLocals<'data>>,
    /// Whether every stream was walked to the section's end; else where
    /// the first record not understood starts (the streams before it are
    /// read).
    pub walked: core::result::Result<(), usize>,
}

impl Default for Locals<'_> {
    fn default() -> Self {
        Self {
            functions: Vec::new(),
            walked: Ok(()),
        }
    }
}

impl<'data> Locals<'data> {
    pub fn parse(sy: &'data [u8]) -> Result<Self> {
        let mut out = Self::default();
        let mut r = Reader::new(sy, 0);
        while r.p < sy.len() {
            let start = r.p;
            match function(&mut r) {
                Some(items) => out.functions.push(FunctionLocals {
                    offset: start,
                    end: r.p,
                    items,
                }),
                None => {
                    out.walked = Err(r.p.max(start));
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The stream that starts at `offset`.
    pub fn at(&self, offset: u32) -> Option<&FunctionLocals<'data>> {
        let i = self
            .functions
            .binary_search_by_key(&(offset as usize), |f| f.offset)
            .ok()?;
        self.functions.get(i)
    }
}

/// A function's stream, from `r.p` to past its end tag.
fn function<'d>(r: &mut Reader<'d>) -> Option<Vec<LocalItem<'d>>> {
    let mut items = Vec::new();
    loop {
        let at = r.p;
        let t = *r.d.get(at)?;
        let item = match t {
            tag::END => {
                r.p += 1;
                return Some(items);
            }
            0x03 => {
                r.p += 1;
                let level = r.u8()?;
                let id = r.id()?;
                let kind = r.u8()?;
                let name = r.sz()?;
                let counts = [r.u8()?, r.u8()?];
                LocalItem::Scope(Scope {
                    offset: at,
                    level,
                    id,
                    kind,
                    name,
                    counts,
                })
            }
            tag::BEGIN => {
                r.p += 1;
                LocalItem::Begin(r.u8()?)
            }
            tag::SCALAR | tag::AGGREGATE => {
                r.p += 1;
                let scope = r.u8()?;
                let id = r.id()?;
                let flags = r.u8()?;
                let name = r.sz()?;
                let (machine_type, type_class) = value_type(r)?;
                let _zero = r.u8()?;
                let class = r.u8()?;
                let _storage = r.u8()?;
                let size = r.c32()?;
                let mut traits = traits(r, false)?;
                let type_index = r.c32()?;
                let alignment = if t == tag::AGGREGATE {
                    Some(r.u8()?)
                } else {
                    None
                };
                attribute_list(r, &mut traits)?;
                LocalItem::Local(Local {
                    offset: at,
                    scope,
                    id,
                    flags,
                    name,
                    kind: LocalKind::Variable {
                        class,
                        size,
                        type_index,
                        machine_type,
                        type_class,
                        alignment,
                        traits,
                    },
                })
            }
            tag::UNNAMED => {
                r.p += 1;
                let scope = r.u8()?;
                let id = r.id()?;
                let (machine_type, type_class) = value_type(r)?;
                let size = r.c32()?;
                let mut traits = traits(r, false)?;
                let type_index = r.c32()?;
                attribute_list(r, &mut traits)?;
                LocalItem::Local(Local {
                    offset: at,
                    scope,
                    id,
                    flags: 0,
                    name: &[],
                    kind: LocalKind::Unnamed {
                        size,
                        type_index,
                        machine_type,
                        type_class,
                        traits,
                    },
                })
            }
            tag::LOCAL_STATIC => {
                r.p += 1;
                let scope = r.u8()?;
                let id = r.id()?;
                let name = r.sz()?;
                let (machine_type, _class) = value_type(r)?;
                let _zero = r.u8()?;
                let size = r.c32()?;
                let _storage = r.take(2)?;
                let type_index = r.c32()?;
                let global = r.id()?;
                let _unknown = r.u8()?;
                LocalItem::Local(Local {
                    offset: at,
                    scope,
                    id,
                    flags: 0,
                    name,
                    kind: LocalKind::Static {
                        size,
                        type_index,
                        machine_type,
                        global,
                    },
                })
            }
            tag::TYPE => LocalItem::Type(super::globals::named_type_at(r, at)?),
            tag::CONSTANT => LocalItem::Constant(super::globals::constant_at(r, at)?),
            _ => return None,
        };
        items.push(item);
    }
}

fn value_type(r: &mut Reader<'_>) -> Option<(u8, u8)> {
    let mt = r.u8()?;
    if mt & 0x40 != 0 {
        r.u8()?;
    }
    Some((mt, r.u8()?))
}
