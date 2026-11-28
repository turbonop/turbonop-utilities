//! Function bodies (`FUNCTION_BLOCK`): every instruction record decoded
//! as LLVM's reader decodes it (value IDs relative to the next
//! instruction's, forward references with their types, each result's
//! type), with the calls and globals each function refers to.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::collections::{HashMap, HashSet};

use turbonop_core::{Error, Result};

use crate::bitstream::{BlockInfo, Cursor, Entry, idx, low32};
use crate::ir::{
    Constant, ConstantKind, Module, ValueRef, read_constants, sign_rotated, type_index,
};
use crate::metadata::{Metadata, MetadataNode, MetadataView, Subprogram};
use crate::tables::*;
use crate::types::{Type, TypeTable, chars};

/// An instruction's opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Opcode {
    Ret,
    Br,
    Switch,
    IndirectBr,
    Invoke,
    Resume,
    Unreachable,
    CleanupRet,
    CatchRet,
    CatchSwitch,
    CallBr,
    FNeg,
    Add,
    FAdd,
    Sub,
    FSub,
    Mul,
    FMul,
    UDiv,
    SDiv,
    FDiv,
    URem,
    SRem,
    FRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
    Alloca,
    Load,
    Store,
    GetElementPtr,
    Fence,
    AtomicCmpXchg,
    AtomicRmw,
    Trunc,
    ZExt,
    SExt,
    FpToUi,
    FpToSi,
    UiToFp,
    SiToFp,
    FpTrunc,
    FpExt,
    PtrToInt,
    PtrToAddr,
    IntToPtr,
    BitCast,
    AddrSpaceCast,
    CleanupPad,
    CatchPad,
    ICmp,
    FCmp,
    Phi,
    Call,
    Select,
    VaArg,
    ExtractElement,
    InsertElement,
    ShuffleVector,
    ExtractValue,
    InsertValue,
    LandingPad,
    Freeze,
    BitInsert,
    BitExtract,
    /// A binary, unary, or cast operator code the type doesn't allow (as
    /// LLVM would reject): the record code.
    Invalid(u32),
}

impl Opcode {
    /// Its name in LLVM's assembly.
    pub fn name(self) -> &'static str {
        use Opcode as O;
        match self {
            O::Ret => "ret",
            O::Br => "br",
            O::Switch => "switch",
            O::IndirectBr => "indirectbr",
            O::Invoke => "invoke",
            O::Resume => "resume",
            O::Unreachable => "unreachable",
            O::CleanupRet => "cleanupret",
            O::CatchRet => "catchret",
            O::CatchSwitch => "catchswitch",
            O::CallBr => "callbr",
            O::FNeg => "fneg",
            O::Add => "add",
            O::FAdd => "fadd",
            O::Sub => "sub",
            O::FSub => "fsub",
            O::Mul => "mul",
            O::FMul => "fmul",
            O::UDiv => "udiv",
            O::SDiv => "sdiv",
            O::FDiv => "fdiv",
            O::URem => "urem",
            O::SRem => "srem",
            O::FRem => "frem",
            O::Shl => "shl",
            O::LShr => "lshr",
            O::AShr => "ashr",
            O::And => "and",
            O::Or => "or",
            O::Xor => "xor",
            O::Alloca => "alloca",
            O::Load => "load",
            O::Store => "store",
            O::GetElementPtr => "getelementptr",
            O::Fence => "fence",
            O::AtomicCmpXchg => "cmpxchg",
            O::AtomicRmw => "atomicrmw",
            O::Trunc => "trunc",
            O::ZExt => "zext",
            O::SExt => "sext",
            O::FpToUi => "fptoui",
            O::FpToSi => "fptosi",
            O::UiToFp => "uitofp",
            O::SiToFp => "sitofp",
            O::FpTrunc => "fptrunc",
            O::FpExt => "fpext",
            O::PtrToInt => "ptrtoint",
            O::PtrToAddr => "ptrtoaddr",
            O::IntToPtr => "inttoptr",
            O::BitCast => "bitcast",
            O::AddrSpaceCast => "addrspacecast",
            O::CleanupPad => "cleanuppad",
            O::CatchPad => "catchpad",
            O::ICmp => "icmp",
            O::FCmp => "fcmp",
            O::Phi => "phi",
            O::Call => "call",
            O::Select => "select",
            O::VaArg => "va_arg",
            O::ExtractElement => "extractelement",
            O::InsertElement => "insertelement",
            O::ShuffleVector => "shufflevector",
            O::ExtractValue => "extractvalue",
            O::InsertValue => "insertvalue",
            O::LandingPad => "landingpad",
            O::Freeze => "freeze",
            O::BitInsert => "bitinsert",
            O::BitExtract => "bitextract",
            O::Invalid(_) => "<invalid>",
        }
    }

    /// Whether it ends a basic block.
    pub fn is_terminator(self) -> bool {
        use Opcode as O;
        matches!(
            self,
            O::Ret
                | O::Br
                | O::Switch
                | O::IndirectBr
                | O::Invoke
                | O::Resume
                | O::Unreachable
                | O::CleanupRet
                | O::CatchRet
                | O::CatchSwitch
                | O::CallBr
        )
    }

    /// Whether it calls (`call`, `invoke`, `callbr`).
    pub fn is_call(self) -> bool {
        matches!(self, Self::Call | Self::Invoke | Self::CallBr)
    }

    /// A cast's opcode (`CAST_*`).
    pub fn cast(code: u64) -> Option<Self> {
        use Opcode as O;
        Some(match code {
            0 => O::Trunc,
            1 => O::ZExt,
            2 => O::SExt,
            3 => O::FpToUi,
            4 => O::FpToSi,
            5 => O::UiToFp,
            6 => O::SiToFp,
            7 => O::FpTrunc,
            8 => O::FpExt,
            9 => O::PtrToInt,
            10 => O::IntToPtr,
            11 => O::BitCast,
            12 => O::AddrSpaceCast,
            13 => O::PtrToAddr,
            _ => return None,
        })
    }

    /// A binary operator's opcode (`BINOP_*`), on an integer or a
    /// floating-point type.
    pub fn binary(code: u64, fp: bool) -> Option<Self> {
        use Opcode as O;
        Some(match (code, fp) {
            (0, false) => O::Add,
            (0, true) => O::FAdd,
            (1, false) => O::Sub,
            (1, true) => O::FSub,
            (2, false) => O::Mul,
            (2, true) => O::FMul,
            (3, false) => O::UDiv,
            (4, false) => O::SDiv,
            (4, true) => O::FDiv,
            (5, false) => O::URem,
            (6, false) => O::SRem,
            (6, true) => O::FRem,
            (7, false) => O::Shl,
            (8, false) => O::LShr,
            (9, false) => O::AShr,
            (10, false) => O::And,
            (11, false) => O::Or,
            (12, false) => O::Xor,
            _ => return None,
        })
    }
}

/// An instruction's operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operand {
    /// A value: its ID (module-level IDs first, then the function's
    /// arguments, constants, and instructions).
    Value(u32),
    /// A basic block's index.
    Block(u32),
    /// Metadata: its ID.
    Metadata(u32),
}

/// An instruction's source location (`DEBUG_LOC`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DebugLoc {
    /// The line.
    pub line: u32,
    /// The column.
    pub column: u32,
    /// The scope's metadata ID.
    pub scope: Option<u32>,
    /// Where it was inlined (a location's metadata ID).
    pub inlined_at: Option<u32>,
}

/// An instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// Its opcode.
    pub opcode: Opcode,
    /// Its record's code (`FUNC_CODE_INST_*`).
    pub code: u32,
    /// The basic block it's in.
    pub block: u32,
    /// The value ID of its result, if it has one (not `void`).
    pub value: Option<u32>,
    /// Its result's type (an ID in the module's types or the body's
    /// [`extra_types`](FunctionBody::extra_types)), when known.
    pub ty: Option<u32>,
    /// Where its operands are in [`FunctionBody::operands`].
    pub operands: (u32, u32),
    /// Its record's size, in bits.
    pub bits: u32,
    /// Its source location.
    pub debug_loc: Option<DebugLoc>,
}

/// What a call calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Callee {
    /// A function (an index into [`Module::functions`]).
    Function(usize),
    /// An alias or ifunc (an index into [`Module::aliases`]).
    Alias(usize),
    /// A global variable (an index into [`Module::globals`]).
    Global(usize),
    /// Inline assembly.
    InlineAsm,
    /// A computed pointer.
    Indirect,
}

/// A call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Call {
    /// The instruction (an index into [`FunctionBody::instructions`]).
    pub instruction: u32,
    /// What it calls.
    pub callee: Callee,
    /// Its attribute list (an index into [`Module::attribute_lists`], plus
    /// one; 0 for none).
    pub attributes: u64,
}

/// What a function-local value ID names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Local {
    /// An argument (its index).
    Argument(u32),
    /// A constant (an index into [`FunctionBody::constants`]).
    Constant(u32),
    /// An instruction's result (an index into
    /// [`FunctionBody::instructions`]).
    Instruction(u32),
}

/// A function body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionBody<'d> {
    /// The function (an index into [`Module::functions`]), if known.
    pub function: Option<usize>,
    /// Where its block starts (its `ENTER_SUBBLOCK`), in bits from the
    /// stream's start.
    pub bit_offset: u64,
    /// Its block's size, in bits: header, records, sub-blocks, and end.
    pub bits: u64,
    /// The length its header gives, in 32-bit words.
    pub num_words: u32,
    /// The number of basic blocks (`DECLAREBLOCKS`).
    pub blocks: u64,
    /// The instructions.
    pub instructions: Vec<Instruction>,
    /// The instructions' operands.
    pub operands: Vec<Operand>,
    /// The call sites.
    pub calls: Vec<Call>,
    /// The function's constants.
    pub constants: Vec<Constant>,
    /// The module-level value count: the first function-local value ID.
    pub first_local: u32,
    /// What each function-local value ID names (from `first_local`).
    pub locals: Vec<Local>,
    /// Types the reader made for this function's values (numbered after
    /// the module's).
    pub extra_types: Vec<Type>,
    /// Names from its value symbol table: value ID and name.
    pub value_names: Vec<(u32, Vec<u8>)>,
    /// Basic blocks' names: index and name.
    pub block_names: Vec<(u32, Vec<u8>)>,
    /// The function's metadata attachments: kind and node ID.
    pub attachments: Vec<(u32, u32)>,
    /// Instructions' attachments: instruction, kind, node ID.
    pub instruction_attachments: Vec<(u32, u32, u32)>,
    /// The function's own metadata (its IDs follow the module's): where
    /// optimized code's debug information keeps its subprogram.
    pub metadata: Vec<MetadataNode<'d>>,
    /// Debug records (`#dbg_value`, `#dbg_declare`, ...).
    pub debug_records: u32,
    /// Records this reader doesn't know (skipped).
    pub unknown_records: u32,
    /// The module-level values (globals, functions, aliases) its
    /// instructions refer to, directly or through constants, sorted.
    pub referenced_globals: Vec<u32>,
    /// What went wrong reading it (the instructions before it are kept).
    pub error: Option<Error>,
}

impl<'d> FunctionBody<'d> {
    /// The function's index in [`Module::functions`].
    pub fn function_index(&self) -> Option<usize> {
        self.function
    }

    /// Instruction `i`'s operands.
    pub fn operands_of(&self, i: &Instruction) -> &[Operand] {
        let (start, len) = (i.operands.0 as usize, i.operands.1 as usize);
        self.operands.get(start..start + len).unwrap_or(&[])
    }

    /// The functions it calls directly (indexes into
    /// [`Module::functions`]), each once, in order of first call.
    pub fn callees(&self) -> Vec<usize> {
        let mut seen = HashSet::new();
        self.calls
            .iter()
            .filter_map(|c| match c.callee {
                Callee::Function(f) => Some(f),
                _ => None,
            })
            .filter(|f| seen.insert(*f))
            .collect()
    }

    /// The size of its instructions' records, in bits.
    pub fn instruction_bits(&self) -> u64 {
        self.instructions.iter().map(|i| u64::from(i.bits)).sum()
    }

    /// Its `!dbg` attachment: the subprogram's metadata ID.
    pub fn subprogram_id(&self, m: &Module<'_>) -> Option<u32> {
        let dbg = m.metadata.kind(b"dbg").unwrap_or(0);
        self.attachments
            .iter()
            .find(|(k, _)| *k == dbg)
            .map(|(_, n)| *n)
    }

    /// The module's metadata and the function's, to look things up in.
    pub fn metadata<'a>(&'a self, m: &'a Module<'d>) -> MetadataView<'a, 'd> {
        MetadataView {
            module: &m.metadata,
            local: &self.metadata,
        }
    }

    /// Its subprogram (`!dbg`): its name, linkage name, file, and line.
    pub fn subprogram<'a>(&'a self, m: &'a Module<'d>) -> Option<Subprogram<'a>> {
        self.metadata(m).subprogram(self.subprogram_id(m)?)
    }

    /// Type `id` as LLVM writes it (a module type, or one of this body's).
    pub fn type_name(&self, m: &Module<'_>, id: u32) -> String {
        if (id as usize) < m.types.len() {
            return m.types.name(id);
        }
        // The module's types, then this body's.
        let mut t = TypeTable {
            types: Vec::with_capacity(m.types.len() + self.extra_types.len()),
            declared: m.types.declared,
            announced: None,
        };
        t.types.extend(m.types.types.iter().cloned());
        t.types.extend(self.extra_types.iter().cloned());
        t.name(id)
    }
}

/// What every body's reader shares: the module's types and values.
pub(crate) struct Context<'m> {
    types: &'m TypeTable,
    index: HashMap<Type, u32>,
    module: &'m Module<'m>,
    relative: bool,
    typed_pointers: bool,
    alloca_address_space: u32,
}

impl<'m> Context<'m> {
    pub(crate) fn new<'d: 'm>(m: &'m Module<'d>) -> Self {
        // (Covariance: a module of 'd is one of 'm.)
        let module: &'m Module<'m> = m;
        Self {
            types: &module.types,
            index: type_index(&module.types),
            module,
            relative: module.version >= 1,
            typed_pointers: module.types.types.iter().any(|t| {
                matches!(
                    t,
                    Type::Pointer {
                        pointee: Some(_),
                        ..
                    }
                )
            }),
            alloca_address_space: alloca_space(&module.datalayout),
        }
    }
}

fn alloca_space(layout: &[u8]) -> u32 {
    layout
        .split(|&c| c == b'-')
        .find_map(|spec| {
            core::str::from_utf8(spec.strip_prefix(b"A")?)
                .ok()?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

const UNKNOWN: u32 = u32::MAX;

struct Reader<'c, 'm, 'd> {
    ctx: &'c Context<'m>,
    body: FunctionBody<'d>,
    /// Each function-local value's type.
    local_types: Vec<u32>,
    /// The next instruction's value ID.
    next: u32,
    block: u32,
    last_loc: Option<DebugLoc>,
    /// Operand bundle values waiting for their call.
    bundles: Vec<Operand>,
}

impl<'d> Reader<'_, '_, 'd> {
    fn module_values(&self) -> u32 {
        low32(self.ctx.module.value_types.len() as u64)
    }

    // Types.

    fn ty(&self, id: u32) -> Option<&Type> {
        let base = self.ctx.types.len();
        match (id as usize).checked_sub(base) {
            None => self.ctx.types.get(id),
            Some(i) => self.body.extra_types.get(i),
        }
    }

    fn intern(&mut self, t: Type) -> u32 {
        if let Some(&i) = self.ctx.index.get(&t) {
            return i;
        }
        let base = self.ctx.types.len();
        if let Some(p) = self.body.extra_types.iter().position(|x| *x == t) {
            return low32((base + p) as u64);
        }
        self.body.extra_types.push(t);
        low32((base + self.body.extra_types.len() - 1) as u64)
    }

    fn is_fp(&self, id: u32) -> bool {
        let scalar = match self.ty(id) {
            Some(Type::Vector { element, .. }) => *element,
            _ => id,
        };
        matches!(
            self.ty(scalar),
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

    fn contained(&self, id: u32, i: usize) -> u32 {
        let t = match self.ty(id) {
            Some(Type::Pointer { pointee, .. }) => pointee.filter(|_| i == 0),
            Some(Type::Array { element, .. } | Type::Vector { element, .. }) => {
                (i == 0).then_some(*element)
            }
            Some(Type::Struct { elements, .. }) => elements.get(i).copied(),
            Some(Type::Function { ret, params, .. }) => {
                if i == 0 {
                    Some(*ret)
                } else {
                    params.get(i - 1).copied()
                }
            }
            _ => None,
        };
        t.unwrap_or(UNKNOWN)
    }

    fn vector_shape(&self, id: u32) -> Option<(u64, bool)> {
        match self.ty(id)? {
            Type::Vector { len, scalable, .. } => Some((*len, *scalable)),
            _ => None,
        }
    }

    fn pointer_to(&mut self, pointee: u32, address_space: u32) -> u32 {
        let t = Type::Pointer {
            pointee: self.ctx.typed_pointers.then_some(pointee),
            address_space,
        };
        self.intern(t)
    }

    fn function_type(&self, id: u32) -> Option<(u32, Vec<u32>, bool)> {
        match self.ty(id)? {
            Type::Function {
                vararg,
                ret,
                params,
            } => Some((*ret, params.clone(), *vararg)),
            _ => None,
        }
    }

    // Values.

    fn value_type(&self, v: u32) -> u32 {
        let first = self.module_values();
        match v.checked_sub(first) {
            None => self
                .ctx
                .module
                .value_types
                .get(v as usize)
                .copied()
                .unwrap_or(UNKNOWN),
            Some(i) => self.local_types.get(i as usize).copied().unwrap_or(UNKNOWN),
        }
    }

    /// A value operand's ID (relative to the next instruction's in
    /// version 1 and later).
    fn rel(&self, v: u64) -> u32 {
        if self.ctx.relative {
            self.next.wrapping_sub(low32(v))
        } else {
            low32(v)
        }
    }

    /// A value and its type (read from the record for a forward
    /// reference), at `ops[*slot]`.
    fn value_type_pair(&self, ops: &[u64], slot: &mut usize) -> Option<(u32, u32)> {
        let v = self.rel(*ops.get(*slot)?);
        *slot += 1;
        if v < self.next {
            return Some((v, self.value_type(v)));
        }
        let ty = low32(*ops.get(*slot)?);
        *slot += 1;
        Some((v, ty))
    }

    fn value(&self, ops: &[u64], slot: usize) -> Option<u32> {
        Some(self.rel(*ops.get(slot)?))
    }

    /// The constant value `v` is, module-level or the function's.
    fn constant(&self, v: u32) -> Option<&Constant> {
        let first = self.module_values();
        match v.checked_sub(first) {
            None => self.ctx.module.constant(v),
            Some(i) => match self.body.locals.get(i as usize)? {
                Local::Constant(c) => self.body.constants.get(*c as usize),
                _ => None,
            },
        }
    }

    fn constant_int(&self, v: u32) -> Option<i64> {
        match &self.constant(v)?.kind {
            ConstantKind::Integer(i) => Some(*i),
            ConstantKind::Null => Some(0),
            _ => None,
        }
    }

    fn callee(&self, v: u32) -> crate::function::Callee {
        let mut v = v;
        for _ in 0..16 {
            if v < self.module_values() {
                match self.ctx.module.value(v) {
                    Some(ValueRef::Function(f)) => return Callee::Function(f),
                    Some(ValueRef::Alias(a)) => return Callee::Alias(a),
                    Some(ValueRef::Global(g)) => return Callee::Global(g),
                    _ => {}
                }
            }
            let Some(c) = self.constant(v) else {
                return Callee::Indirect;
            };
            match &c.kind {
                ConstantKind::InlineAsm { .. } => return Callee::InlineAsm,
                // Old bitcode calls functions through bitcasts.
                ConstantKind::Cast {
                    op: CAST_BITCAST | CAST_ADDRSPACECAST,
                    operand,
                    ..
                } => v = *operand,
                ConstantKind::NoCfi(f) | ConstantKind::DsoLocalEquivalent(f) => v = *f,
                _ => return Callee::Indirect,
            }
        }
        Callee::Indirect
    }

    // Records.

    fn push(
        &mut self,
        opcode: Opcode,
        code: u32,
        ty: Option<u32>,
        produces: bool,
        operands: Vec<Operand>,
        bits: u64,
    ) {
        let start = low32(self.body.operands.len() as u64);
        let len = low32(operands.len() as u64);
        self.body.operands.extend(operands);
        let index = low32(self.body.instructions.len() as u64);
        let value = produces.then(|| {
            let v = self.next;
            self.next = self.next.wrapping_add(1);
            self.local_types.push(ty.unwrap_or(UNKNOWN));
            self.body.locals.push(Local::Instruction(index));
            v
        });
        self.body.instructions.push(Instruction {
            opcode,
            code,
            block: self.block,
            value,
            ty,
            operands: (start, len),
            bits: low32(bits),
            debug_loc: None,
        });
        if opcode.is_terminator() {
            self.block = self.block.wrapping_add(1);
        }
    }

    /// The arguments of a call-like record, from `slot`: fixed parameters
    /// by the function type, then type and value pairs for varargs.
    fn arguments(
        &self,
        ops: &[u64],
        mut slot: usize,
        fty: Option<&(u32, Vec<u32>, bool)>,
        out: &mut Vec<Operand>,
    ) {
        let Some((_, params, vararg)) = fty else {
            // Without a type, read what's there as values.
            while let Some(v) = self.value(ops, slot) {
                out.push(Operand::Value(v));
                slot += 1;
            }
            return;
        };
        for p in params {
            let Some(&raw) = ops.get(slot) else { return };
            slot += 1;
            match self.ty(*p) {
                Some(Type::Label) => out.push(Operand::Block(low32(raw))),
                Some(Type::Metadata) => out.push(Operand::Metadata(self.rel(raw))),
                _ => out.push(Operand::Value(self.rel(raw))),
            }
        }
        if *vararg {
            while let Some((v, _)) = self.value_type_pair(ops, &mut slot) {
                out.push(Operand::Value(v));
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn record(&mut self, code: u32, ops: &[u64], bits: u64) {
        use Opcode as O;
        let op = |i: usize| ops.get(i).copied().unwrap_or(0);
        let mut operands = Vec::new();
        let mut slot = 0usize;
        macro_rules! pair {
            () => {{
                match self.value_type_pair(ops, &mut slot) {
                    Some((v, t)) => {
                        operands.push(Operand::Value(v));
                        t
                    }
                    None => UNKNOWN,
                }
            }};
        }
        macro_rules! pop {
            () => {{
                if let Some(v) = self.value(ops, slot) {
                    operands.push(Operand::Value(v));
                }
                #[allow(unused_assignments)]
                {
                    slot += 1;
                }
            }};
        }
        match code {
            FUNC_CODE_DECLAREBLOCKS => {
                self.body.blocks = op(0);
            }
            FUNC_CODE_INST_UNOP => {
                let t = pair!();
                let opcode = if op(slot) == u64::from(UNOP_FNEG) && self.is_fp(t) {
                    O::FNeg
                } else {
                    O::Invalid(code)
                };
                self.push(opcode, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_BINOP => {
                let t = pair!();
                pop!();
                let opcode = O::binary(op(slot), self.is_fp(t)).unwrap_or(O::Invalid(code));
                self.push(opcode, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_CAST => {
                pair!();
                let dest = low32(op(slot));
                let opcode = O::cast(op(slot + 1)).unwrap_or(O::Invalid(code));
                self.push(opcode, code, Some(dest), true, operands, bits);
            }
            FUNC_CODE_INST_GEP | FUNC_CODE_INST_GEP_OLD | FUNC_CODE_INST_INBOUNDS_GEP_OLD => {
                let mut source = None;
                if code == FUNC_CODE_INST_GEP {
                    source = Some(low32(op(1)));
                    slot = 2;
                }
                let base = pair!();
                let mut vector = self.vector_shape(base);
                let mut indices = Vec::new();
                while let Some((v, t)) = self.value_type_pair(ops, &mut slot) {
                    operands.push(Operand::Value(v));
                    indices.push(v);
                    if vector.is_none() {
                        vector = self.vector_shape(t);
                    }
                }
                let scalar_base = if self.vector_shape(base).is_some() {
                    self.contained(base, 0)
                } else {
                    base
                };
                let address_space = match self.ty(scalar_base) {
                    Some(Type::Pointer { address_space, .. }) => *address_space,
                    _ => 0,
                };
                let ptr = if self.ctx.typed_pointers {
                    // The indexed type, through the source type.
                    let mut t = source.unwrap_or_else(|| self.contained(scalar_base, 0));
                    for &i in indices.iter().skip(1) {
                        t = match self.ty(t) {
                            Some(Type::Struct { .. }) => {
                                let n = self.constant_int(i).unwrap_or(0);
                                self.contained(t, usize::try_from(n).unwrap_or(0))
                            }
                            _ => self.contained(t, 0),
                        };
                    }
                    self.pointer_to(t, address_space)
                } else {
                    self.pointer_to(UNKNOWN, address_space)
                };
                let ty = match vector {
                    Some((len, scalable)) => self.intern(Type::Vector {
                        len,
                        element: ptr,
                        scalable,
                    }),
                    None => ptr,
                };
                self.push(O::GetElementPtr, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_EXTRACTVAL => {
                let mut t = pair!();
                for &i in ops.get(slot..).unwrap_or(&[]) {
                    t = match self.ty(t) {
                        Some(Type::Struct { .. }) => self.contained(t, idx(i)),
                        _ => self.contained(t, 0),
                    };
                }
                self.push(O::ExtractValue, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_INSERTVAL => {
                let t = pair!();
                pair!();
                self.push(O::InsertValue, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_SELECT => {
                let t = pair!();
                pop!();
                pop!();
                self.push(O::Select, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_VSELECT => {
                let t = pair!();
                pop!();
                pair!();
                self.push(O::Select, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_EXTRACTELT => {
                let t = pair!();
                pair!();
                let e = self.contained(t, 0);
                self.push(O::ExtractElement, code, Some(e), true, operands, bits);
            }
            FUNC_CODE_INST_INSERTELT => {
                let t = pair!();
                pop!();
                pair!();
                self.push(O::InsertElement, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_BITINSERT => {
                let t = pair!();
                pair!();
                pair!();
                self.push(O::BitInsert, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_BITEXTRACT => {
                let t = low32(op(0));
                slot = 1;
                pair!();
                pair!();
                self.push(O::BitExtract, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_SHUFFLEVEC => {
                let t = pair!();
                pop!();
                let mask = pair!();
                let ty = match self.vector_shape(mask) {
                    Some((len, scalable)) => {
                        let element = self.contained(t, 0);
                        Some(self.intern(Type::Vector {
                            len,
                            element,
                            scalable,
                        }))
                    }
                    None => None,
                };
                self.push(O::ShuffleVector, code, ty, true, operands, bits);
            }
            FUNC_CODE_INST_CMP | FUNC_CODE_INST_CMP2 => {
                let t = pair!();
                pop!();
                let opcode = if self.is_fp(t) { O::FCmp } else { O::ICmp };
                let i1 = self.intern(Type::Integer(1));
                let ty = match self.vector_shape(t) {
                    Some((len, scalable)) => self.intern(Type::Vector {
                        len,
                        element: i1,
                        scalable,
                    }),
                    None => i1,
                };
                self.push(opcode, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_RET => {
                if !ops.is_empty() {
                    pair!();
                }
                self.push(O::Ret, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_BR => {
                operands.push(Operand::Block(low32(op(0))));
                if ops.len() >= 3 {
                    operands.push(Operand::Block(low32(op(1))));
                    if let Some(v) = self.value(ops, 2) {
                        operands.push(Operand::Value(v));
                    }
                }
                self.push(O::Br, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_CLEANUPRET => {
                pop!();
                if ops.len() >= 2 {
                    operands.push(Operand::Block(low32(op(1))));
                }
                self.push(O::CleanupRet, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_CATCHRET => {
                pop!();
                operands.push(Operand::Block(low32(op(1))));
                self.push(O::CatchRet, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_CATCHSWITCH => {
                pop!();
                let n = idx(op(1)).min(ops.len());
                for i in 0..n {
                    if let Some(&b) = ops.get(2 + i) {
                        operands.push(Operand::Block(low32(b)));
                    }
                }
                if let Some(&b) = ops.get(2 + n) {
                    operands.push(Operand::Block(low32(b)));
                }
                let token = self.intern(Type::Token);
                self.push(O::CatchSwitch, code, Some(token), true, operands, bits);
            }
            FUNC_CODE_INST_CATCHPAD | FUNC_CODE_INST_CLEANUPPAD => {
                pop!();
                let n = op(1).min(ops.len() as u64);
                slot = 2;
                for _ in 0..n {
                    pair!();
                }
                let token = self.intern(Type::Token);
                let opcode = if code == FUNC_CODE_INST_CATCHPAD {
                    O::CatchPad
                } else {
                    O::CleanupPad
                };
                self.push(opcode, code, Some(token), true, operands, bits);
            }
            FUNC_CODE_INST_SWITCH => {
                self.switch(ops, &mut operands);
                self.push(O::Switch, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_INDIRECTBR => {
                if let Some(v) = self.value(ops, 1) {
                    operands.push(Operand::Value(v));
                }
                for &b in ops.get(2..).unwrap_or(&[]) {
                    operands.push(Operand::Block(low32(b)));
                }
                self.push(O::IndirectBr, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_INVOKE => {
                let cc = op(1);
                operands.push(Operand::Block(low32(op(2))));
                operands.push(Operand::Block(low32(op(3))));
                slot = 4;
                let mut fty = None;
                if (cc >> 13) & 1 != 0 {
                    fty = Some(low32(op(4)));
                    slot = 5;
                }
                self.call_like(O::Invoke, code, ops, slot, fty, operands, bits);
            }
            FUNC_CODE_INST_CALLBR => {
                let cc = op(1);
                operands.push(Operand::Block(low32(op(2))));
                let n = idx(op(3)).min(ops.len());
                for i in 0..n {
                    operands.push(Operand::Block(low32(op(4 + i))));
                }
                slot = 4 + n;
                let mut fty = None;
                if (cc >> CALL_EXPLICIT_TYPE) & 1 != 0 {
                    fty = Some(low32(op(slot)));
                    slot += 1;
                }
                self.call_like(O::CallBr, code, ops, slot, fty, operands, bits);
            }
            FUNC_CODE_INST_CALL => {
                let cc = op(1);
                slot = 2;
                if (cc >> CALL_FMF) & 1 != 0 {
                    slot += 1;
                }
                let mut fty = None;
                if (cc >> CALL_EXPLICIT_TYPE) & 1 != 0 {
                    fty = Some(low32(op(slot)));
                    slot += 1;
                }
                self.call_like(O::Call, code, ops, slot, fty, operands, bits);
            }
            FUNC_CODE_INST_RESUME => {
                pair!();
                self.push(O::Resume, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_UNREACHABLE => {
                self.push(O::Unreachable, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_PHI => {
                let t = low32(op(0));
                let n = ops.len().saturating_sub(1) / 2;
                for i in 0..n {
                    let raw = op(1 + i * 2);
                    let v = if self.ctx.relative {
                        self.next.wrapping_sub(low32(sign_rotated(raw) as u64))
                    } else {
                        low32(raw)
                    };
                    operands.push(Operand::Value(v));
                    operands.push(Operand::Block(low32(op(2 + i * 2))));
                }
                self.push(O::Phi, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_LANDINGPAD | FUNC_CODE_INST_LANDINGPAD_OLD => {
                let t = low32(op(0));
                slot = 1;
                if code == FUNC_CODE_INST_LANDINGPAD_OLD {
                    pair!();
                }
                slot += 1;
                let n = op(slot).min(ops.len() as u64);
                slot += 1;
                for _ in 0..n {
                    slot += 1;
                    pair!();
                }
                self.push(O::LandingPad, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_ALLOCA => {
                let packed = op(3);
                let explicit = packed & (1 << 6) != 0;
                let ty = if explicit {
                    low32(op(0))
                } else {
                    self.contained(low32(op(0)), 0)
                };
                // The size is an absolute value ID.
                operands.push(Operand::Value(low32(op(2))));
                let address_space = if ops.len() == 5 {
                    low32(op(4))
                } else {
                    self.ctx.alloca_address_space
                };
                let ptr = self.pointer_to(ty, address_space);
                self.push(O::Alloca, code, Some(ptr), true, operands, bits);
            }
            FUNC_CODE_INST_LOAD => {
                let p = pair!();
                let ty = if ops.len() == slot + 3 {
                    low32(op(slot))
                } else {
                    self.contained(p, 0)
                };
                self.push(O::Load, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_LOADATOMIC => {
                let p = pair!();
                let ty = if ops.len() >= slot + 5 {
                    low32(op(slot))
                } else {
                    self.contained(p, 0)
                };
                self.push(O::Load, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_STORE | FUNC_CODE_INST_STOREATOMIC => {
                pair!();
                pair!();
                self.push(O::Store, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_STORE_OLD | FUNC_CODE_INST_STOREATOMIC_OLD => {
                pair!();
                pop!();
                self.push(O::Store, code, None, false, operands, bits);
            }
            FUNC_CODE_INST_CMPXCHG_OLD => {
                let p = pair!();
                pop!();
                pop!();
                let cmp = self.contained(p, 0);
                let ty = if ops.len() < 8 {
                    cmp
                } else {
                    let i1 = self.intern(Type::Integer(1));
                    self.intern(Type::Struct {
                        name: None,
                        packed: false,
                        elements: alloc::vec![cmp, i1],
                        opaque: false,
                    })
                };
                self.push(O::AtomicCmpXchg, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_CMPXCHG => {
                pair!();
                let cmp = pair!();
                pop!();
                let i1 = self.intern(Type::Integer(1));
                let ty = self.intern(Type::Struct {
                    name: None,
                    packed: false,
                    elements: alloc::vec![cmp, i1],
                    opaque: false,
                });
                self.push(O::AtomicCmpXchg, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_ATOMICRMW_OLD => {
                let p = pair!();
                pop!();
                let ty = self.contained(p, 0);
                self.push(O::AtomicRmw, code, Some(ty), true, operands, bits);
            }
            FUNC_CODE_INST_ATOMICRMW => {
                pair!();
                let t = pair!();
                self.push(O::AtomicRmw, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_INST_FENCE => self.push(O::Fence, code, None, false, operands, bits),
            FUNC_CODE_INST_VAARG => {
                if let Some(v) = self.value(ops, 1) {
                    operands.push(Operand::Value(v));
                }
                self.push(O::VaArg, code, Some(low32(op(2))), true, operands, bits);
            }
            FUNC_CODE_INST_FREEZE => {
                let t = pair!();
                self.push(O::Freeze, code, Some(t), true, operands, bits);
            }
            FUNC_CODE_OPERAND_BUNDLE => {
                slot = 1;
                while let Some(&raw) = ops.get(slot) {
                    if low32(raw) == OB_METADATA {
                        let Some(&m) = ops.get(slot + 1) else { break };
                        self.bundles
                            .push(Operand::Metadata(self.next.wrapping_sub(low32(m))));
                        slot += 2;
                    } else {
                        let Some((v, _)) = self.value_type_pair(ops, &mut slot) else {
                            break;
                        };
                        self.bundles.push(Operand::Value(v));
                    }
                }
            }
            FUNC_CODE_DEBUG_LOC => {
                let loc = DebugLoc {
                    line: low32(op(0)),
                    column: low32(op(1)),
                    scope: op(2).checked_sub(1).map(low32),
                    inlined_at: op(3).checked_sub(1).map(low32),
                };
                self.last_loc = Some(loc);
                if let Some(i) = self.body.instructions.last_mut() {
                    i.debug_loc = Some(loc);
                }
            }
            FUNC_CODE_DEBUG_LOC_AGAIN => {
                if let Some(i) = self.body.instructions.last_mut() {
                    i.debug_loc = self.last_loc;
                }
            }
            FUNC_CODE_DEBUG_RECORD_VALUE
            | FUNC_CODE_DEBUG_RECORD_DECLARE
            | FUNC_CODE_DEBUG_RECORD_ASSIGN
            | FUNC_CODE_DEBUG_RECORD_VALUE_SIMPLE
            | FUNC_CODE_DEBUG_RECORD_LABEL
            | FUNC_CODE_DEBUG_RECORD_DECLARE_VALUE => self.body.debug_records += 1,
            FUNC_CODE_BLOCKADDR_USERS => {}
            _ => self.body.unknown_records += 1,
        }
    }

    /// A call, invoke, or callbr: the callee at `slot`, then the
    /// arguments; operand bundles before it are its operands too.
    #[allow(clippy::too_many_arguments)]
    fn call_like(
        &mut self,
        opcode: Opcode,
        code: u32,
        ops: &[u64],
        mut slot: usize,
        fty: Option<u32>,
        mut operands: Vec<Operand>,
        bits: u64,
    ) {
        let Some((callee, callee_ty)) = self.value_type_pair(ops, &mut slot) else {
            self.push(opcode, code, None, false, operands, bits);
            return;
        };
        operands.push(Operand::Value(callee));
        // Old bitcode: the type through the callee's pointer type.
        let fty = fty.or_else(|| {
            let t = self.contained(callee_ty, 0);
            (t != UNKNOWN).then_some(t)
        });
        let parts = fty.and_then(|t| self.function_type(t));
        self.arguments(ops, slot, parts.as_ref(), &mut operands);
        operands.append(&mut self.bundles);
        let ret = parts.as_ref().map(|p| p.0);
        let produces = ret.is_none_or(|r| !matches!(self.ty(r), Some(Type::Void)));
        let index = low32(self.body.instructions.len() as u64);
        let target = self.callee(callee);
        self.body.calls.push(Call {
            instruction: index,
            callee: target,
            attributes: ops.first().copied().unwrap_or(0),
        });
        self.push(opcode, code, ret, produces, operands, bits);
    }

    fn switch(&self, ops: &[u64], operands: &mut Vec<Operand>) {
        const MAGIC: u64 = 0x4B5;
        let op = |i: usize| ops.get(i).copied().unwrap_or(0);
        if ops.first().is_some_and(|&v| v >> 16 == MAGIC) {
            // The case-range format: [magic, type, cond, default, cases,
            // per case: items, per item: single?, words, value(s); dest].
            let width = match self.ty(low32(op(1))) {
                Some(Type::Integer(w)) => *w,
                _ => 64,
            };
            if let Some(v) = self.value(ops, 2) {
                operands.push(Operand::Value(v));
            }
            operands.push(Operand::Block(low32(op(3))));
            let cases = op(4).min(ops.len() as u64);
            let mut i = 5usize;
            for _ in 0..cases {
                let items = op(i).min(ops.len() as u64);
                i += 1;
                for _ in 0..items {
                    let single = op(i) != 0;
                    i += 1;
                    for _ in 0..if single { 1 } else { 2 } {
                        let words = if width > 64 {
                            let n = idx(op(i));
                            i += 1;
                            n
                        } else {
                            1
                        };
                        i = i.saturating_add(words);
                    }
                    if i >= ops.len() {
                        return;
                    }
                }
                operands.push(Operand::Block(low32(op(i))));
                i += 1;
            }
            return;
        }
        if let Some(v) = self.value(ops, 1) {
            operands.push(Operand::Value(v));
        }
        operands.push(Operand::Block(low32(op(2))));
        let cases = ops.len().saturating_sub(3) / 2;
        for c in 0..cases {
            // Case values are absolute value IDs.
            operands.push(Operand::Value(low32(op(3 + c * 2))));
            operands.push(Operand::Block(low32(op(4 + c * 2))));
        }
    }

    fn value_symtab(&mut self, c: &mut Cursor<'_>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            let a = match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => continue,
                Entry::Record(a) => a,
            };
            ops.clear();
            let head = c.read_record(a, &mut ops)?;
            let Some((&id, name)) = ops.split_first() else {
                continue;
            };
            match head.code {
                VST_CODE_ENTRY => self.body.value_names.push((low32(id), chars(name))),
                VST_CODE_BBENTRY => self.body.block_names.push((low32(id), chars(name))),
                _ => {}
            }
        }
    }

    fn attachments(&mut self, c: &mut Cursor<'_>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            let a = match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => continue,
                Entry::Record(a) => a,
            };
            ops.clear();
            let head = c.read_record(a, &mut ops)?;
            if head.code != METADATA_ATTACHMENT {
                continue;
            }
            if ops.len() % 2 == 1 {
                // An instruction's: [instruction, (kind, node)*].
                let i = low32(ops[0]);
                for p in ops[1..].chunks(2) {
                    self.body
                        .instruction_attachments
                        .push((i, low32(p[0]), low32(p[1])));
                }
            } else {
                for p in ops.chunks(2) {
                    self.body.attachments.push((low32(p[0]), low32(p[1])));
                }
            }
        }
    }

    /// The globals each operand reaches, through constants.
    fn referenced_globals(&self) -> Vec<u32> {
        let first = self.module_values();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut stack: Vec<u32> = self
            .body
            .operands
            .iter()
            .filter_map(|o| match o {
                Operand::Value(v) => Some(*v),
                _ => None,
            })
            .collect();
        while let Some(v) = stack.pop() {
            if !seen.insert(v) {
                continue;
            }
            if v < first {
                match self.ctx.module.value(v) {
                    Some(ValueRef::Constant(c)) => {
                        if let Some(c) = self.ctx.module.constants.get(c) {
                            stack.extend(c.operands());
                        }
                    }
                    Some(_) => out.push(v),
                    None => {}
                }
            } else if let Some(c) = self.constant(v) {
                stack.extend(c.operands());
            }
        }
        out.sort_unstable();
        out
    }
}

/// Reads the body whose block starts at bit `start` (its `ENTER_SUBBLOCK`;
/// its header at `header`), of function `function`.
pub(crate) fn read_body<'d>(
    ctx: &Context<'_>,
    data: &'d [u8],
    info: &Arc<BlockInfo>,
    function: Option<usize>,
    start: u64,
    header: u64,
) -> FunctionBody<'d> {
    let first = low32(ctx.module.value_types.len() as u64);
    let mut r = Reader {
        ctx,
        body: FunctionBody {
            function,
            bit_offset: start,
            first_local: first,
            ..FunctionBody::default()
        },
        local_types: Vec::new(),
        next: first,
        block: 0,
        last_loc: None,
        bundles: Vec::new(),
    };
    // The arguments come first.
    if let Some(f) = function.and_then(|f| ctx.module.functions.get(f))
        && let Some((_, params, _)) = r.function_type(f.function_type)
    {
        for (i, p) in params.iter().enumerate() {
            r.local_types.push(*p);
            r.body.locals.push(Local::Argument(low32(i as u64)));
        }
        r.next = first.wrapping_add(low32(params.len() as u64));
    }
    if let Err(e) = r.read(data, info, header) {
        r.body.error = Some(e);
    }
    r.body.referenced_globals = r.referenced_globals();
    r.body
}

impl FunctionBody<'_> {
    /// How many arguments the function has.
    pub fn arguments(&self) -> usize {
        self.locals
            .iter()
            .filter(|l| matches!(l, Local::Argument(_)))
            .count()
    }
}

impl<'d> Reader<'_, '_, 'd> {
    fn read(&mut self, data: &'d [u8], info: &Arc<BlockInfo>, header: u64) -> Result<()> {
        let mut c = Cursor::new(data, header)?;
        c.set_block_info(Arc::clone(info));
        let h = c.enter_block(FUNCTION_BLOCK_ID)?;
        self.body.num_words = h.num_words;
        let mut ops = Vec::new();
        loop {
            let before = c.position();
            match c.advance_with(true)? {
                Entry::EndBlock => {
                    self.body.bits = c.position() - self.body.bit_offset;
                    return Ok(());
                }
                Entry::SubBlock(id) => self.sub_block(&mut c, id)?,
                Entry::Record(DEFINE_ABBREV) => c.read_abbrev()?,
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    let bits = c.position() - before;
                    self.record(head.code, &ops, bits);
                }
            }
        }
    }

    fn sub_block(&mut self, c: &mut Cursor<'d>, id: u32) -> Result<()> {
        match id {
            CONSTANTS_BLOCK_ID => {
                c.enter_block(id)?;
                let i32_type = self.intern(Type::Integer(32));
                let mut value_types = Vec::new();
                let mut constants = Vec::new();
                read_constants(c, i32_type, &mut value_types, &mut |k| constants.push(k))?;
                for (k, t) in constants.into_iter().zip(value_types) {
                    let i = low32(self.body.constants.len() as u64);
                    self.body.constants.push(k);
                    self.body.locals.push(Local::Constant(i));
                    self.local_types.push(t);
                    self.next = self.next.wrapping_add(1);
                }
                Ok(())
            }
            VALUE_SYMTAB_BLOCK_ID => {
                c.enter_block(id)?;
                self.value_symtab(c)
            }
            METADATA_ATTACHMENT_ID => {
                c.enter_block(id)?;
                self.attachments(c)
            }
            METADATA_BLOCK_ID => {
                c.enter_block(id)?;
                let mut md = Metadata::default();
                let r = md.read_block(c);
                self.body.metadata.extend(md.nodes);
                r
            }
            _ => c.skip_block(),
        }
    }
}
