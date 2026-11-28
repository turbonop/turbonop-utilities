//! The type table (`TYPE_BLOCK_ID_NEW`): every type a module uses, by ID.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bitstream::low32;
use crate::tables::{
    TYPE_CODE_ARRAY, TYPE_CODE_BFLOAT, TYPE_CODE_BYTE, TYPE_CODE_DOUBLE, TYPE_CODE_FLOAT,
    TYPE_CODE_FP128, TYPE_CODE_FUNCTION, TYPE_CODE_FUNCTION_OLD, TYPE_CODE_HALF, TYPE_CODE_INTEGER,
    TYPE_CODE_LABEL, TYPE_CODE_METADATA, TYPE_CODE_NUMENTRY, TYPE_CODE_OPAQUE,
    TYPE_CODE_OPAQUE_POINTER, TYPE_CODE_POINTER, TYPE_CODE_PPC_FP128, TYPE_CODE_STRUCT_ANON,
    TYPE_CODE_STRUCT_NAME, TYPE_CODE_STRUCT_NAMED, TYPE_CODE_TARGET_TYPE, TYPE_CODE_TOKEN,
    TYPE_CODE_VECTOR, TYPE_CODE_VOID, TYPE_CODE_X86_AMX, TYPE_CODE_X86_FP80, TYPE_CODE_X86_MMX,
};

/// A type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// `void`.
    Void,
    /// `half`.
    Half,
    /// `bfloat`.
    BFloat,
    /// `float`.
    Float,
    /// `double`.
    Double,
    /// `x86_fp80`.
    X86Fp80,
    /// `fp128`.
    Fp128,
    /// `ppc_fp128`.
    PpcFp128,
    /// `label`.
    Label,
    /// `metadata`.
    Metadata,
    /// `x86_mmx` (old bitcode; LLVM now reads it as `<1 x i64>`).
    X86Mmx,
    /// `x86_amx`.
    X86Amx,
    /// `token`.
    Token,
    /// `iN`.
    Integer(u32),
    /// `bN` (the byte type).
    Byte(u32),
    /// A pointer: opaque (`ptr`), or, in old bitcode, typed (`T*`).
    Pointer {
        /// What it points to, for a typed pointer.
        pointee: Option<u32>,
        /// Its address space.
        address_space: u32,
    },
    /// A function type.
    Function {
        /// Whether it takes more arguments (`...`).
        vararg: bool,
        /// The return type.
        ret: u32,
        /// The parameter types.
        params: Vec<u32>,
    },
    /// A structure: literal (no name) or identified (named, possibly
    /// without a body: opaque).
    Struct {
        /// Its name, for an identified structure (possibly empty).
        name: Option<Vec<u8>>,
        /// Whether it's packed (`<{ }>`).
        packed: bool,
        /// Its element types.
        elements: Vec<u32>,
        /// Whether it has no body (`opaque`).
        opaque: bool,
    },
    /// `[N x T]`.
    Array {
        /// The number of elements.
        len: u64,
        /// The element type.
        element: u32,
    },
    /// `<N x T>` or `<vscale x N x T>`.
    Vector {
        /// The (minimum) number of elements.
        len: u64,
        /// The element type.
        element: u32,
        /// Whether it's scalable.
        scalable: bool,
    },
    /// A target extension type: `target("name", types..., ints...)`.
    Target {
        /// Its name.
        name: Vec<u8>,
        /// Its type parameters.
        types: Vec<u32>,
        /// Its integer parameters.
        ints: Vec<u64>,
    },
    /// A type record this reader doesn't know (its code).
    Unknown(u32),
}

/// A module's types, by ID: the type table's, then any the reader made
/// (as LLVM's reader does: `ptr` in an address space the table lacks, a
/// comparison's `<N x i1>`, a `cmpxchg`'s `{ T, i1 }`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeTable {
    /// The types.
    pub types: Vec<Type>,
    /// How many of them the type table has (the rest the reader made).
    pub declared: usize,
    /// The entry count the table announced (`NUMENTRY`).
    pub announced: Option<u64>,
}

/// The longest a type's name is written (longer is cut short with `...`).
pub const MAX_NAME: usize = 4096;

/// Whether `name` can be written bare after `%` or `@` (LLVM's rule).
fn is_bare(name: &[u8]) -> bool {
    !name.is_empty()
        && !name[0].is_ascii_digit()
        && name
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'$' | b'.' | b'_'))
}

/// A name as LLVM's assembly writes it after `prefix` (`%`, `@`, `$`):
/// bare, or quoted with `\XX` escapes.
pub fn llvm_name(prefix: char, name: &[u8]) -> String {
    let mut s = String::new();
    s.push(prefix);
    if is_bare(name) {
        s.push_str(&String::from_utf8_lossy(name));
        return s;
    }
    s.push('"');
    for &c in name {
        if (0x20..=0x7E).contains(&c) && c != b'"' && c != b'\\' {
            s.push(char::from(c));
        } else {
            s.push_str(&format!("\\{c:02X}"));
        }
    }
    s.push('"');
    s
}

impl TypeTable {
    /// Type `id`.
    pub fn get(&self, id: u32) -> Option<&Type> {
        self.types.get(id as usize)
    }

    /// The number of types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// The ID of `t`, adding it if the table hasn't it.
    pub(crate) fn intern(&mut self, t: Type) -> u32 {
        if let Some(i) = self.types.iter().position(|x| *x == t) {
            return low32(i as u64);
        }
        self.types.push(t);
        low32(self.types.len() as u64 - 1)
    }

    /// Whether `id` is a floating-point type or a vector of them.
    pub fn is_fp_or_fp_vector(&self, id: u32) -> bool {
        match self.get(id) {
            Some(Type::Vector { element, .. }) => self.is_fp(*element),
            _ => self.is_fp(id),
        }
    }

    /// Whether `id` is a floating-point type.
    pub fn is_fp(&self, id: u32) -> bool {
        matches!(
            self.get(id),
            Some(
                Type::Half
                    | Type::BFloat
                    | Type::Float
                    | Type::Double
                    | Type::X86Fp80
                    | Type::Fp128
                    | Type::PpcFp128
            )
        )
    }

    /// Whether `id` is `void`.
    pub fn is_void(&self, id: u32) -> bool {
        matches!(self.get(id), Some(Type::Void))
    }

    /// Whether `id` is a vector.
    pub fn is_vector(&self, id: u32) -> bool {
        matches!(self.get(id), Some(Type::Vector { .. }))
    }

    /// The `i`th type `id` contains, as LLVM's reader numbers them: a
    /// typed pointer's pointee, an array's or vector's element, a
    /// structure's elements, a function's return type then parameters.
    pub fn contained(&self, id: u32, i: usize) -> Option<u32> {
        match self.get(id)? {
            Type::Pointer { pointee, .. } => pointee.filter(|_| i == 0),
            Type::Array { element, .. } | Type::Vector { element, .. } => {
                (i == 0).then_some(*element)
            }
            Type::Struct { elements, .. } => elements.get(i).copied(),
            Type::Function { ret, params, .. } => {
                if i == 0 {
                    Some(*ret)
                } else {
                    params.get(i - 1).copied()
                }
            }
            Type::Target { types, .. } => types.get(i).copied(),
            _ => None,
        }
    }

    /// A function type's parts: return type, parameter types, vararg.
    pub fn function(&self, id: u32) -> Option<(u32, &[u32], bool)> {
        match self.get(id)? {
            Type::Function {
                vararg,
                ret,
                params,
            } => Some((*ret, params, *vararg)),
            _ => None,
        }
    }

    /// Whether `id` is a pointer (or a vector of them).
    pub fn is_pointer(&self, id: u32) -> bool {
        match self.get(id) {
            Some(Type::Pointer { .. }) => true,
            Some(Type::Vector { element, .. }) => {
                matches!(self.get(*element), Some(Type::Pointer { .. }))
            }
            _ => false,
        }
    }

    /// A pointer's address space.
    pub fn address_space(&self, id: u32) -> Option<u32> {
        match self.get(id)? {
            Type::Pointer { address_space, .. } => Some(*address_space),
            _ => None,
        }
    }

    /// Type `id` as LLVM's assembly writes it.
    pub fn name(&self, id: u32) -> String {
        let mut s = String::new();
        self.write(&mut s, id, 0);
        s
    }

    fn write(&self, s: &mut String, id: u32, depth: u32) {
        // Malformed tables can refer around in circles through wide types:
        // the name is cut short rather than grown without end.
        if depth > 64 || s.len() > MAX_NAME {
            if !s.ends_with("...") {
                s.push_str("...");
            }
            return;
        }
        let Some(t) = self.get(id) else {
            s.push_str(&format!("<type {id}>"));
            return;
        };
        let d = depth + 1;
        match t {
            Type::Void => s.push_str("void"),
            Type::Half => s.push_str("half"),
            Type::BFloat => s.push_str("bfloat"),
            Type::Float => s.push_str("float"),
            Type::Double => s.push_str("double"),
            Type::X86Fp80 => s.push_str("x86_fp80"),
            Type::Fp128 => s.push_str("fp128"),
            Type::PpcFp128 => s.push_str("ppc_fp128"),
            Type::Label => s.push_str("label"),
            Type::Metadata => s.push_str("metadata"),
            Type::X86Mmx => s.push_str("x86_mmx"),
            Type::X86Amx => s.push_str("x86_amx"),
            Type::Token => s.push_str("token"),
            Type::Integer(w) => s.push_str(&format!("i{w}")),
            Type::Byte(w) => s.push_str(&format!("b{w}")),
            Type::Pointer {
                pointee: None,
                address_space,
            } => {
                s.push_str("ptr");
                if *address_space != 0 {
                    s.push_str(&format!(" addrspace({address_space})"));
                }
            }
            Type::Pointer {
                pointee: Some(p),
                address_space,
            } => {
                self.write(s, *p, d);
                if *address_space != 0 {
                    s.push_str(&format!(" addrspace({address_space})"));
                }
                s.push('*');
            }
            Type::Function {
                vararg,
                ret,
                params,
            } => {
                self.write(s, *ret, d);
                s.push_str(" (");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.write(s, *p, d);
                }
                if *vararg {
                    if !params.is_empty() {
                        s.push_str(", ");
                    }
                    s.push_str("...");
                }
                s.push(')');
            }
            Type::Struct {
                name: Some(name), ..
            } if !name.is_empty() => s.push_str(&llvm_name('%', name)),
            Type::Struct { name: Some(_), .. } => {
                // An unnamed identified structure: numbered in table order.
                let n = self.types[..id as usize]
                    .iter()
                    .filter(|t| matches!(t, Type::Struct { name: Some(n), .. } if n.is_empty()))
                    .count();
                s.push_str(&format!("%{n}"));
            }
            Type::Struct {
                name: None,
                packed,
                elements,
                ..
            } => {
                if *packed {
                    s.push('<');
                }
                if elements.is_empty() {
                    s.push_str("{}");
                } else {
                    s.push_str("{ ");
                    for (i, e) in elements.iter().enumerate() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        self.write(s, *e, d);
                    }
                    s.push_str(" }");
                }
                if *packed {
                    s.push('>');
                }
            }
            Type::Array { len, element } => {
                s.push_str(&format!("[{len} x "));
                self.write(s, *element, d);
                s.push(']');
            }
            Type::Vector {
                len,
                element,
                scalable,
            } => {
                s.push('<');
                if *scalable {
                    s.push_str("vscale x ");
                }
                s.push_str(&format!("{len} x "));
                self.write(s, *element, d);
                s.push('>');
            }
            Type::Target { name, types, ints } => {
                s.push_str("target(\"");
                s.push_str(&String::from_utf8_lossy(name));
                s.push('"');
                for t in types {
                    s.push_str(", ");
                    self.write(s, *t, d);
                }
                for i in ints {
                    s.push_str(&format!(", {i}"));
                }
                s.push(')');
            }
            Type::Unknown(code) => s.push_str(&format!("<unknown type record {code}>")),
        }
    }

    /// A structure's body as LLVM writes it in a type definition
    /// (`%name = type { ... }`): the literal form, or `opaque`.
    pub fn body(&self, id: u32) -> Option<String> {
        match self.get(id)? {
            Type::Struct {
                name: Some(_),
                opaque: true,
                ..
            } => Some(String::from("opaque")),
            Type::Struct {
                name: Some(_),
                packed,
                elements,
                ..
            } => {
                let literal = TypeTable {
                    types: alloc::vec![Type::Struct {
                        name: None,
                        packed: *packed,
                        elements: elements.clone(),
                        opaque: false,
                    }],
                    declared: 1,
                    announced: None,
                };
                // Elements are named by this table.
                let mut s = String::new();
                if let Some(Type::Struct {
                    packed, elements, ..
                }) = literal.types.first()
                {
                    if *packed {
                        s.push('<');
                    }
                    if elements.is_empty() {
                        s.push_str("{}");
                    } else {
                        s.push_str("{ ");
                        for (i, e) in elements.iter().enumerate() {
                            if i > 0 {
                                s.push_str(", ");
                            }
                            self.write(&mut s, *e, 1);
                        }
                        s.push_str(" }");
                    }
                    if *packed {
                        s.push('>');
                    }
                }
                Some(s)
            }
            _ => None,
        }
    }

    /// Reads one type-table record into the table. `name` holds a pending
    /// `STRUCT_NAME`.
    pub(crate) fn record(&mut self, code: u32, ops: &[u64], name: &mut Option<Vec<u8>>) {
        let id = |v: u64| low32(v);
        let op = |i: usize| ops.get(i).copied().unwrap_or(0);
        let t = match code {
            TYPE_CODE_NUMENTRY => {
                self.announced = ops.first().copied();
                return;
            }
            TYPE_CODE_STRUCT_NAME => {
                *name = Some(ops.iter().map(|&v| v.to_le_bytes()[0]).collect());
                return;
            }
            TYPE_CODE_VOID => Type::Void,
            TYPE_CODE_HALF => Type::Half,
            TYPE_CODE_BFLOAT => Type::BFloat,
            TYPE_CODE_FLOAT => Type::Float,
            TYPE_CODE_DOUBLE => Type::Double,
            TYPE_CODE_X86_FP80 => Type::X86Fp80,
            TYPE_CODE_FP128 => Type::Fp128,
            TYPE_CODE_PPC_FP128 => Type::PpcFp128,
            TYPE_CODE_LABEL => Type::Label,
            TYPE_CODE_METADATA => Type::Metadata,
            TYPE_CODE_X86_MMX => Type::X86Mmx,
            TYPE_CODE_X86_AMX => Type::X86Amx,
            TYPE_CODE_TOKEN => Type::Token,
            TYPE_CODE_INTEGER => Type::Integer(id(op(0))),
            TYPE_CODE_BYTE => Type::Byte(id(op(0))),
            TYPE_CODE_POINTER => Type::Pointer {
                pointee: Some(id(op(0))),
                address_space: id(op(1)),
            },
            TYPE_CODE_OPAQUE_POINTER => Type::Pointer {
                pointee: None,
                address_space: id(op(0)),
            },
            TYPE_CODE_FUNCTION_OLD => Type::Function {
                vararg: op(0) != 0,
                ret: id(op(2)),
                params: ops.iter().skip(3).map(|&v| id(v)).collect(),
            },
            TYPE_CODE_FUNCTION => Type::Function {
                vararg: op(0) != 0,
                ret: id(op(1)),
                params: ops.iter().skip(2).map(|&v| id(v)).collect(),
            },
            TYPE_CODE_STRUCT_ANON => Type::Struct {
                name: None,
                packed: op(0) != 0,
                elements: ops.iter().skip(1).map(|&v| id(v)).collect(),
                opaque: false,
            },
            TYPE_CODE_STRUCT_NAMED => Type::Struct {
                name: Some(name.take().unwrap_or_default()),
                packed: op(0) != 0,
                elements: ops.iter().skip(1).map(|&v| id(v)).collect(),
                opaque: false,
            },
            TYPE_CODE_OPAQUE => Type::Struct {
                name: Some(name.take().unwrap_or_default()),
                packed: false,
                elements: Vec::new(),
                opaque: true,
            },
            TYPE_CODE_TARGET_TYPE => {
                let n = crate::bitstream::idx(op(0)).min(ops.len().saturating_sub(1));
                Type::Target {
                    name: name.take().unwrap_or_default(),
                    // (An empty record has neither.)
                    types: ops
                        .get(1..1 + n)
                        .unwrap_or(&[])
                        .iter()
                        .map(|&v| id(v))
                        .collect(),
                    ints: ops.get(1 + n..).unwrap_or(&[]).to_vec(),
                }
            }
            TYPE_CODE_ARRAY => Type::Array {
                len: op(0),
                element: id(op(1)),
            },
            TYPE_CODE_VECTOR => Type::Vector {
                len: op(0),
                element: id(op(1)),
                scalable: op(2) != 0,
            },
            other => Type::Unknown(other),
        };
        self.types.push(t);
        self.declared = self.types.len();
    }
}

/// The string of a record's characters (one per operand).
pub(crate) fn chars(ops: &[u64]) -> Vec<u8> {
    ops.iter().map(|&v| v.to_le_bytes()[0]).collect()
}

/// A name, borrowed from the string table or made from a record.
pub(crate) type Name<'d> = Cow<'d, [u8]>;
