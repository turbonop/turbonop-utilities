//! CodeView symbol records.

use turbonop_core::{Reader, Result};

use crate::types::TypeIndex;

/// `COMPILESYM3`'s flags: the language (`iLanguage`) is the low 8 bits.
pub const COMPILE3_LANGUAGE_MASK: u32 = 0xFF;

/// `DEFRANGESYMSUBFIELDREGISTER`'s `offParent`: the low
/// `CV_OFFSET_PARENT_LENGTH_LIMIT` bits of its 32-bit field.
pub const CV_OFFSET_PARENT_LENGTH_LIMIT: u32 = 12;
/// The mask of `offParent`.
pub const OFFSET_PARENT_MASK: u32 = (1 << CV_OFFSET_PARENT_LENGTH_LIMIT) - 1;
use crate::types::{Numeric, read_numeric};

/// Symbol record kinds: every kind cvinfo.h's `SYM_ENUM_e` names, and the
/// later ones LLVM's `CodeViewSymbols.def` adds (`S_FRAMEREG` on).
#[allow(missing_docs)]
pub mod kind {
    macro_rules! kinds {
        ($($name:ident = $value:literal, $text:literal;)*) => {
            $(pub const $name: u16 = $value;)*

            /// The kind's name, as cvinfo.h spells it (`S_???` for a kind
            /// it doesn't name).
            pub fn name(kind: u16) -> &'static str {
                match kind {
                    $($name => $text,)*
                    _ => "S_???",
                }
            }

            /// Every named kind, in order of value.
            pub const ALL: &[u16] = &[$($name),*];
        };
    }

    kinds! {
        S_COMPILE = 0x0001, "S_COMPILE";
        S_REGISTER_16T = 0x0002, "S_REGISTER_16t";
        S_CONSTANT_16T = 0x0003, "S_CONSTANT_16t";
        S_UDT_16T = 0x0004, "S_UDT_16t";
        S_SSEARCH = 0x0005, "S_SSEARCH";
        S_END = 0x0006, "S_END";
        S_SKIP = 0x0007, "S_SKIP";
        S_CVRESERVE = 0x0008, "S_CVRESERVE";
        S_OBJNAME_ST = 0x0009, "S_OBJNAME_ST";
        S_ENDARG = 0x000A, "S_ENDARG";
        S_COBOLUDT_16T = 0x000B, "S_COBOLUDT_16t";
        S_MANYREG_16T = 0x000C, "S_MANYREG_16t";
        S_RETURN = 0x000D, "S_RETURN";
        S_ENTRYTHIS = 0x000E, "S_ENTRYTHIS";
        S_BPREL16 = 0x0100, "S_BPREL16";
        S_LDATA16 = 0x0101, "S_LDATA16";
        S_GDATA16 = 0x0102, "S_GDATA16";
        S_PUB16 = 0x0103, "S_PUB16";
        S_LPROC16 = 0x0104, "S_LPROC16";
        S_GPROC16 = 0x0105, "S_GPROC16";
        S_THUNK16 = 0x0106, "S_THUNK16";
        S_BLOCK16 = 0x0107, "S_BLOCK16";
        S_WITH16 = 0x0108, "S_WITH16";
        S_LABEL16 = 0x0109, "S_LABEL16";
        S_CEXMODEL16 = 0x010A, "S_CEXMODEL16";
        S_VFTABLE16 = 0x010B, "S_VFTABLE16";
        S_REGREL16 = 0x010C, "S_REGREL16";
        S_BPREL32_16T = 0x0200, "S_BPREL32_16t";
        S_LDATA32_16T = 0x0201, "S_LDATA32_16t";
        S_GDATA32_16T = 0x0202, "S_GDATA32_16t";
        S_PUB32_16T = 0x0203, "S_PUB32_16t";
        S_LPROC32_16T = 0x0204, "S_LPROC32_16t";
        S_GPROC32_16T = 0x0205, "S_GPROC32_16t";
        S_THUNK32_ST = 0x0206, "S_THUNK32_ST";
        S_BLOCK32_ST = 0x0207, "S_BLOCK32_ST";
        S_WITH32_ST = 0x0208, "S_WITH32_ST";
        S_LABEL32_ST = 0x0209, "S_LABEL32_ST";
        S_CEXMODEL32 = 0x020A, "S_CEXMODEL32";
        S_VFTABLE32_16T = 0x020B, "S_VFTABLE32_16t";
        S_REGREL32_16T = 0x020C, "S_REGREL32_16t";
        S_LTHREAD32_16T = 0x020D, "S_LTHREAD32_16t";
        S_GTHREAD32_16T = 0x020E, "S_GTHREAD32_16t";
        S_SLINK32 = 0x020F, "S_SLINK32";
        S_LPROCMIPS_16T = 0x0300, "S_LPROCMIPS_16t";
        S_GPROCMIPS_16T = 0x0301, "S_GPROCMIPS_16t";
        S_PROCREF_ST = 0x0400, "S_PROCREF_ST";
        S_DATAREF_ST = 0x0401, "S_DATAREF_ST";
        S_ALIGN = 0x0402, "S_ALIGN";
        S_LPROCREF_ST = 0x0403, "S_LPROCREF_ST";
        S_OEM = 0x0404, "S_OEM";
        S_REGISTER_ST = 0x1001, "S_REGISTER_ST";
        S_CONSTANT_ST = 0x1002, "S_CONSTANT_ST";
        S_UDT_ST = 0x1003, "S_UDT_ST";
        S_COBOLUDT_ST = 0x1004, "S_COBOLUDT_ST";
        S_MANYREG_ST = 0x1005, "S_MANYREG_ST";
        S_BPREL32_ST = 0x1006, "S_BPREL32_ST";
        S_LDATA32_ST = 0x1007, "S_LDATA32_ST";
        S_GDATA32_ST = 0x1008, "S_GDATA32_ST";
        S_PUB32_ST = 0x1009, "S_PUB32_ST";
        S_LPROC32_ST = 0x100A, "S_LPROC32_ST";
        S_GPROC32_ST = 0x100B, "S_GPROC32_ST";
        S_VFTABLE32 = 0x100C, "S_VFTABLE32";
        S_REGREL32_ST = 0x100D, "S_REGREL32_ST";
        S_LTHREAD32_ST = 0x100E, "S_LTHREAD32_ST";
        S_GTHREAD32_ST = 0x100F, "S_GTHREAD32_ST";
        S_LPROCMIPS_ST = 0x1010, "S_LPROCMIPS_ST";
        S_GPROCMIPS_ST = 0x1011, "S_GPROCMIPS_ST";
        S_FRAMEPROC = 0x1012, "S_FRAMEPROC";
        S_COMPILE2_ST = 0x1013, "S_COMPILE2_ST";
        S_MANYREG2_ST = 0x1014, "S_MANYREG2_ST";
        S_LPROCIA64_ST = 0x1015, "S_LPROCIA64_ST";
        S_GPROCIA64_ST = 0x1016, "S_GPROCIA64_ST";
        S_LOCALSLOT_ST = 0x1017, "S_LOCALSLOT_ST";
        S_PARAMSLOT_ST = 0x1018, "S_PARAMSLOT_ST";
        S_ANNOTATION = 0x1019, "S_ANNOTATION";
        S_GMANPROC_ST = 0x101A, "S_GMANPROC_ST";
        S_LMANPROC_ST = 0x101B, "S_LMANPROC_ST";
        S_RESERVED1 = 0x101C, "S_RESERVED1";
        S_RESERVED2 = 0x101D, "S_RESERVED2";
        S_RESERVED3 = 0x101E, "S_RESERVED3";
        S_RESERVED4 = 0x101F, "S_RESERVED4";
        S_LMANDATA_ST = 0x1020, "S_LMANDATA_ST";
        S_GMANDATA_ST = 0x1021, "S_GMANDATA_ST";
        S_MANFRAMEREL_ST = 0x1022, "S_MANFRAMEREL_ST";
        S_MANREGISTER_ST = 0x1023, "S_MANREGISTER_ST";
        S_MANSLOT_ST = 0x1024, "S_MANSLOT_ST";
        S_MANMANYREG_ST = 0x1025, "S_MANMANYREG_ST";
        S_MANREGREL_ST = 0x1026, "S_MANREGREL_ST";
        S_MANMANYREG2_ST = 0x1027, "S_MANMANYREG2_ST";
        S_MANTYPREF = 0x1028, "S_MANTYPREF";
        S_UNAMESPACE_ST = 0x1029, "S_UNAMESPACE_ST";
        S_OBJNAME = 0x1101, "S_OBJNAME";
        S_THUNK32 = 0x1102, "S_THUNK32";
        S_BLOCK32 = 0x1103, "S_BLOCK32";
        S_WITH32 = 0x1104, "S_WITH32";
        S_LABEL32 = 0x1105, "S_LABEL32";
        S_REGISTER = 0x1106, "S_REGISTER";
        S_CONSTANT = 0x1107, "S_CONSTANT";
        S_UDT = 0x1108, "S_UDT";
        S_COBOLUDT = 0x1109, "S_COBOLUDT";
        S_MANYREG = 0x110A, "S_MANYREG";
        S_BPREL32 = 0x110B, "S_BPREL32";
        S_LDATA32 = 0x110C, "S_LDATA32";
        S_GDATA32 = 0x110D, "S_GDATA32";
        S_PUB32 = 0x110E, "S_PUB32";
        S_LPROC32 = 0x110F, "S_LPROC32";
        S_GPROC32 = 0x1110, "S_GPROC32";
        S_REGREL32 = 0x1111, "S_REGREL32";
        S_LTHREAD32 = 0x1112, "S_LTHREAD32";
        S_GTHREAD32 = 0x1113, "S_GTHREAD32";
        S_LPROCMIPS = 0x1114, "S_LPROCMIPS";
        S_GPROCMIPS = 0x1115, "S_GPROCMIPS";
        S_COMPILE2 = 0x1116, "S_COMPILE2";
        S_MANYREG2 = 0x1117, "S_MANYREG2";
        S_LPROCIA64 = 0x1118, "S_LPROCIA64";
        S_GPROCIA64 = 0x1119, "S_GPROCIA64";
        S_LOCALSLOT = 0x111A, "S_LOCALSLOT";
        S_PARAMSLOT = 0x111B, "S_PARAMSLOT";
        S_LMANDATA = 0x111C, "S_LMANDATA";
        S_GMANDATA = 0x111D, "S_GMANDATA";
        S_MANFRAMEREL = 0x111E, "S_MANFRAMEREL";
        S_MANREGISTER = 0x111F, "S_MANREGISTER";
        S_MANSLOT = 0x1120, "S_MANSLOT";
        S_MANMANYREG = 0x1121, "S_MANMANYREG";
        S_MANREGREL = 0x1122, "S_MANREGREL";
        S_MANMANYREG2 = 0x1123, "S_MANMANYREG2";
        S_UNAMESPACE = 0x1124, "S_UNAMESPACE";
        S_PROCREF = 0x1125, "S_PROCREF";
        S_DATAREF = 0x1126, "S_DATAREF";
        S_LPROCREF = 0x1127, "S_LPROCREF";
        S_ANNOTATIONREF = 0x1128, "S_ANNOTATIONREF";
        S_TOKENREF = 0x1129, "S_TOKENREF";
        S_GMANPROC = 0x112A, "S_GMANPROC";
        S_LMANPROC = 0x112B, "S_LMANPROC";
        S_TRAMPOLINE = 0x112C, "S_TRAMPOLINE";
        S_MANCONSTANT = 0x112D, "S_MANCONSTANT";
        S_ATTR_FRAMEREL = 0x112E, "S_ATTR_FRAMEREL";
        S_ATTR_REGISTER = 0x112F, "S_ATTR_REGISTER";
        S_ATTR_REGREL = 0x1130, "S_ATTR_REGREL";
        S_ATTR_MANYREG = 0x1131, "S_ATTR_MANYREG";
        S_SEPCODE = 0x1132, "S_SEPCODE";
        S_LOCAL_2005 = 0x1133, "S_LOCAL_2005";
        S_DEFRANGE_2005 = 0x1134, "S_DEFRANGE_2005";
        S_DEFRANGE2_2005 = 0x1135, "S_DEFRANGE2_2005";
        S_SECTION = 0x1136, "S_SECTION";
        S_COFFGROUP = 0x1137, "S_COFFGROUP";
        S_EXPORT = 0x1138, "S_EXPORT";
        S_CALLSITEINFO = 0x1139, "S_CALLSITEINFO";
        S_FRAMECOOKIE = 0x113A, "S_FRAMECOOKIE";
        S_DISCARDED = 0x113B, "S_DISCARDED";
        S_COMPILE3 = 0x113C, "S_COMPILE3";
        S_ENVBLOCK = 0x113D, "S_ENVBLOCK";
        S_LOCAL = 0x113E, "S_LOCAL";
        S_DEFRANGE = 0x113F, "S_DEFRANGE";
        S_DEFRANGE_SUBFIELD = 0x1140, "S_DEFRANGE_SUBFIELD";
        S_DEFRANGE_REGISTER = 0x1141, "S_DEFRANGE_REGISTER";
        S_DEFRANGE_FRAMEPOINTER_REL = 0x1142, "S_DEFRANGE_FRAMEPOINTER_REL";
        S_DEFRANGE_SUBFIELD_REGISTER = 0x1143, "S_DEFRANGE_SUBFIELD_REGISTER";
        S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE = 0x1144, "S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE";
        S_DEFRANGE_REGISTER_REL = 0x1145, "S_DEFRANGE_REGISTER_REL";
        S_LPROC32_ID = 0x1146, "S_LPROC32_ID";
        S_GPROC32_ID = 0x1147, "S_GPROC32_ID";
        S_LPROCMIPS_ID = 0x1148, "S_LPROCMIPS_ID";
        S_GPROCMIPS_ID = 0x1149, "S_GPROCMIPS_ID";
        S_LPROCIA64_ID = 0x114A, "S_LPROCIA64_ID";
        S_GPROCIA64_ID = 0x114B, "S_GPROCIA64_ID";
        S_BUILDINFO = 0x114C, "S_BUILDINFO";
        S_INLINESITE = 0x114D, "S_INLINESITE";
        S_INLINESITE_END = 0x114E, "S_INLINESITE_END";
        S_PROC_ID_END = 0x114F, "S_PROC_ID_END";
        S_DEFRANGE_HLSL = 0x1150, "S_DEFRANGE_HLSL";
        S_GDATA_HLSL = 0x1151, "S_GDATA_HLSL";
        S_LDATA_HLSL = 0x1152, "S_LDATA_HLSL";
        S_FILESTATIC = 0x1153, "S_FILESTATIC";
        S_LOCAL_DPC_GROUPSHARED = 0x1154, "S_LOCAL_DPC_GROUPSHARED";
        S_LPROC32_DPC = 0x1155, "S_LPROC32_DPC";
        S_LPROC32_DPC_ID = 0x1156, "S_LPROC32_DPC_ID";
        S_DEFRANGE_DPC_PTR_TAG = 0x1157, "S_DEFRANGE_DPC_PTR_TAG";
        S_DPC_SYM_TAG_MAP = 0x1158, "S_DPC_SYM_TAG_MAP";
        S_ARMSWITCHTABLE = 0x1159, "S_ARMSWITCHTABLE";
        S_CALLEES = 0x115A, "S_CALLEES";
        S_CALLERS = 0x115B, "S_CALLERS";
        S_POGODATA = 0x115C, "S_POGODATA";
        S_INLINESITE2 = 0x115D, "S_INLINESITE2";
        S_HEAPALLOCSITE = 0x115E, "S_HEAPALLOCSITE";
        S_MOD_TYPEREF = 0x115F, "S_MOD_TYPEREF";
        S_REF_MINIPDB = 0x1160, "S_REF_MINIPDB";
        S_PDBMAP = 0x1161, "S_PDBMAP";
        S_GDATA_HLSL32 = 0x1162, "S_GDATA_HLSL32";
        S_LDATA_HLSL32 = 0x1163, "S_LDATA_HLSL32";
        S_GDATA_HLSL32_EX = 0x1164, "S_GDATA_HLSL32_EX";
        S_LDATA_HLSL32_EX = 0x1165, "S_LDATA_HLSL32_EX";
        S_FRAMEREG = 0x1166, "S_FRAMEREG";
        S_FASTLINK = 0x1167, "S_FASTLINK";
        S_INLINEES = 0x1168, "S_INLINEES";
        S_HOTPATCHFUNC = 0x1169, "S_HOTPATCHFUNC";
        S_BPREL32_INDIR = 0x1170, "S_BPREL32_INDIR";
        S_REGREL32_INDIR = 0x1171, "S_REGREL32_INDIR";
        S_GPROC32EX = 0x1172, "S_GPROC32EX";
        S_LPROC32EX = 0x1173, "S_LPROC32EX";
        S_GPROC32EX_ID = 0x1174, "S_GPROC32EX_ID";
        S_LPROC32EX_ID = 0x1175, "S_LPROC32EX_ID";
        S_STATICLOCAL = 0x1176, "S_STATICLOCAL";
        S_DEFRANGE_REGISTER_REL_INDIR = 0x1177, "S_DEFRANGE_REGISTER_REL_INDIR";
        S_BPREL32_ENCTMP = 0x1178, "S_BPREL32_ENCTMP";
        S_REGREL32_ENCTMP = 0x1179, "S_REGREL32_ENCTMP";
        S_BPREL32_INDIR_ENCTMP = 0x117A, "S_BPREL32_INDIR_ENCTMP";
        S_REGREL32_INDIR_ENCTMP = 0x117B, "S_REGREL32_INDIR_ENCTMP";
        S_ASSOCIATION = 0x117C, "S_ASSOCIATION";
        S_SOURCELINK = 0x117E, "S_SOURCELINK";
        S_DEFRANGE_CONSTVAL_ON_ENTRY = 0x117F, "S_DEFRANGE_CONSTVAL_ON_ENTRY";
        S_DEFRANGE_GLOBALSYM_ON_ENTRY = 0x1180, "S_DEFRANGE_GLOBALSYM_ON_ENTRY";
        S_ALTOBJNAME = 0x1181, "S_ALTOBJNAME";
    }

    /// Whether a symbol opens a scope closed by S_END / S_PROC_ID_END /
    /// S_INLINESITE_END.
    pub fn opens_scope(kind: u16) -> bool {
        matches!(
            kind,
            S_GPROC32
                | S_LPROC32
                | S_GPROC32_ID
                | S_LPROC32_ID
                | S_BLOCK32
                | S_THUNK32
                | S_SEPCODE
                | S_INLINESITE
                | S_INLINESITE2
                | S_LPROC32_DPC
                | S_LPROC32_DPC_ID
                | S_GMANPROC
                | S_LMANPROC
        )
    }

    pub fn closes_scope(kind: u16) -> bool {
        matches!(kind, S_END | S_PROC_ID_END | S_INLINESITE_END)
    }
}

/// A symbol record: kind, the bytes after the kind, and its offset in its
/// stream (which other records use to refer to it).
#[derive(Clone, Copy, Debug)]
pub struct SymbolRecord<'a> {
    pub offset: u32,
    pub kind: u16,
    pub data: &'a [u8],
}

/// Iterates over a buffer of symbol records starting at `base`.
pub struct SymbolIter<'a> {
    r: Reader<'a>,
    base: u32,
}

impl<'a> SymbolIter<'a> {
    pub fn new(data: &'a [u8], base: u32) -> Self {
        Self {
            r: Reader::new(data),
            base,
        }
    }
}

impl<'a> Iterator for SymbolIter<'a> {
    type Item = SymbolRecord<'a>;

    fn next(&mut self) -> Option<SymbolRecord<'a>> {
        let rest = self.r.remaining();
        if rest.len() < 4 {
            return None;
        }
        let offset = self.base + self.r.position() as u32;
        let len = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
        let Some(body) = rest.get(2..2 + len).filter(|_| len >= 2) else {
            // (Left past the length, as reading it would.)
            self.r.skip(2).ok()?;
            return None;
        };
        self.r.skip(2 + len).ok()?;
        Some(SymbolRecord {
            offset,
            kind: u16::from_le_bytes([body[0], body[1]]),
            data: &body[2..],
        })
    }
}

/// A decoded symbol. Variants hold borrowed names.
#[derive(Clone, Copy, Debug)]
pub enum Symbol<'a> {
    Public {
        flags: u32,
        offset: u32,
        segment: u16,
        name: &'a [u8],
    },
    Procedure {
        global: bool,
        id: bool,
        parent: u32,
        end: u32,
        next: u32,
        len: u32,
        debug_start: u32,
        debug_end: u32,
        ty: TypeIndex,
        offset: u32,
        segment: u16,
        flags: u8,
        name: &'a [u8],
    },
    Data {
        global: bool,
        thread: bool,
        ty: TypeIndex,
        offset: u32,
        segment: u16,
        name: &'a [u8],
    },
    Udt {
        ty: TypeIndex,
        name: &'a [u8],
    },
    Constant {
        ty: TypeIndex,
        value: Numeric,
        name: &'a [u8],
    },
    /// S_PROCREF, S_LPROCREF, S_DATAREF: a reference to a symbol in a module.
    Reference {
        kind: u16,
        sum_name: u32,
        symbol_offset: u32,
        module: u16,
        name: &'a [u8],
    },
    RegisterRelative {
        offset: i32,
        ty: TypeIndex,
        register: u16,
        name: &'a [u8],
    },
    BpRelative {
        offset: i32,
        ty: TypeIndex,
        name: &'a [u8],
    },
    Register {
        ty: TypeIndex,
        register: u16,
        name: &'a [u8],
    },
    Local {
        ty: TypeIndex,
        flags: u16,
        name: &'a [u8],
    },
    Block {
        parent: u32,
        end: u32,
        len: u32,
        offset: u32,
        segment: u16,
        name: &'a [u8],
    },
    Label {
        offset: u32,
        segment: u16,
        flags: u8,
        name: &'a [u8],
    },
    Thunk {
        parent: u32,
        end: u32,
        next: u32,
        offset: u32,
        segment: u16,
        len: u16,
        ordinal: u8,
        name: &'a [u8],
    },
    ObjName {
        signature: u32,
        name: &'a [u8],
    },
    Compile3 {
        flags: u32,
        machine: u16,
        frontend: [u16; 4],
        backend: [u16; 4],
        version: &'a [u8],
    },
    FrameProc {
        frame_size: u32,
        pad_size: u32,
        pad_offset: u32,
        callee_saved_size: u32,
        eh_offset: u32,
        eh_section: u16,
        flags: u32,
    },
    Section {
        section: u16,
        alignment: u8,
        rva: u32,
        size: u32,
        characteristics: u32,
        name: &'a [u8],
    },
    CoffGroup {
        size: u32,
        characteristics: u32,
        offset: u32,
        segment: u16,
        name: &'a [u8],
    },
    BuildInfo {
        id: TypeIndex,
    },
    /// S_SEPCODE: a block of a function's code separated from its body
    /// (by PGO, into a cold region, say).
    SeparatedCode {
        parent: u32,
        end: u32,
        len: u32,
        flags: u32,
        offset: u32,
        parent_offset: u32,
        segment: u16,
        parent_segment: u16,
    },
    /// S_ANNOTATION: strings the code attached to an address (the
    /// `__annotation` intrinsic).
    Annotation {
        offset: u32,
        segment: u16,
        /// The strings, as stored (up to the record's end).
        strings: &'a [u8],
        count: u16,
    },
    InlineSite {
        parent: u32,
        end: u32,
        inlinee: TypeIndex,
        annotations: &'a [u8],
    },
    /// S_DEFRANGE_*: where a preceding S_LOCAL lives over a range of code.
    DefRange {
        kind: u16,
        data: &'a [u8],
    },
    End {
        kind: u16,
    },
    /// S_POGODATA (`POGOINFO`): what PGO's training counted of the
    /// function it is in.
    PogoData {
        /// How many times the function was called.
        invocations: u32,
        /// How many of its instructions ran, in all.
        dynamic_instructions: u64,
        /// How many instructions it has.
        instructions: u32,
        /// How many of them ran at all (were "live").
        live_instructions: u32,
    },
    Other {
        kind: u16,
        data: &'a [u8],
    },
}

impl<'a> SymbolRecord<'a> {
    /// Decodes the record into a typed view.
    pub fn parse(&self) -> Result<Symbol<'a>> {
        use kind::*;
        let mut r = Reader::new(self.data);
        let r = &mut r;
        let ty = |r: &mut Reader<'a>| r.u32_le().map(TypeIndex);
        Ok(match self.kind {
            S_PUB32 => Symbol::Public {
                flags: r.u32_le()?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                name: r.cstr()?,
            },
            S_GPROC32 | S_LPROC32 | S_GPROC32_ID | S_LPROC32_ID | S_LPROC32_DPC
            | S_LPROC32_DPC_ID => Symbol::Procedure {
                global: matches!(self.kind, S_GPROC32 | S_GPROC32_ID),
                id: matches!(self.kind, S_GPROC32_ID | S_LPROC32_ID | S_LPROC32_DPC_ID),
                parent: r.u32_le()?,
                end: r.u32_le()?,
                next: r.u32_le()?,
                len: r.u32_le()?,
                debug_start: r.u32_le()?,
                debug_end: r.u32_le()?,
                ty: ty(r)?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                flags: r.u8()?,
                name: r.cstr()?,
            },
            S_GDATA32 | S_LDATA32 | S_GTHREAD32 | S_LTHREAD32 => Symbol::Data {
                global: matches!(self.kind, S_GDATA32 | S_GTHREAD32),
                thread: matches!(self.kind, S_GTHREAD32 | S_LTHREAD32),
                ty: ty(r)?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                name: r.cstr()?,
            },
            S_UDT => Symbol::Udt {
                ty: ty(r)?,
                name: r.cstr()?,
            },
            S_CONSTANT => Symbol::Constant {
                ty: ty(r)?,
                value: read_numeric(r)?,
                name: r.cstr()?,
            },
            S_PROCREF | S_LPROCREF | S_DATAREF => Symbol::Reference {
                kind: self.kind,
                sum_name: r.u32_le()?,
                symbol_offset: r.u32_le()?,
                module: r.u16_le()?,
                name: r.cstr()?,
            },
            S_REGREL32 => Symbol::RegisterRelative {
                offset: r.u32_le()? as i32,
                ty: ty(r)?,
                register: r.u16_le()?,
                name: r.cstr()?,
            },
            S_BPREL32 => Symbol::BpRelative {
                offset: r.u32_le()? as i32,
                ty: ty(r)?,
                name: r.cstr()?,
            },
            S_REGISTER => Symbol::Register {
                ty: ty(r)?,
                register: r.u16_le()?,
                name: r.cstr()?,
            },
            S_LOCAL => Symbol::Local {
                ty: ty(r)?,
                flags: r.u16_le()?,
                name: r.cstr()?,
            },
            S_BLOCK32 => Symbol::Block {
                parent: r.u32_le()?,
                end: r.u32_le()?,
                len: r.u32_le()?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                name: r.cstr()?,
            },
            S_LABEL32 => Symbol::Label {
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                flags: r.u8()?,
                name: r.cstr()?,
            },
            S_THUNK32 => Symbol::Thunk {
                parent: r.u32_le()?,
                end: r.u32_le()?,
                next: r.u32_le()?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                len: r.u16_le()?,
                ordinal: r.u8()?,
                name: r.cstr()?,
            },
            S_OBJNAME => Symbol::ObjName {
                signature: r.u32_le()?,
                name: r.cstr()?,
            },
            S_COMPILE3 => Symbol::Compile3 {
                flags: r.u32_le()?,
                machine: r.u16_le()?,
                frontend: [r.u16_le()?, r.u16_le()?, r.u16_le()?, r.u16_le()?],
                backend: [r.u16_le()?, r.u16_le()?, r.u16_le()?, r.u16_le()?],
                version: r.cstr()?,
            },
            S_FRAMEPROC => Symbol::FrameProc {
                frame_size: r.u32_le()?,
                pad_size: r.u32_le()?,
                pad_offset: r.u32_le()?,
                callee_saved_size: r.u32_le()?,
                eh_offset: r.u32_le()?,
                eh_section: r.u16_le()?,
                flags: r.u32_le()?,
            },
            S_SECTION => Symbol::Section {
                section: r.u16_le()?,
                alignment: r.u8()?,
                rva: {
                    r.u8()?;
                    r.u32_le()?
                },
                size: r.u32_le()?,
                characteristics: r.u32_le()?,
                name: r.cstr()?,
            },
            S_COFFGROUP => Symbol::CoffGroup {
                size: r.u32_le()?,
                characteristics: r.u32_le()?,
                offset: r.u32_le()?,
                segment: r.u16_le()?,
                name: r.cstr()?,
            },
            S_BUILDINFO => Symbol::BuildInfo { id: ty(r)? },
            S_SEPCODE => Symbol::SeparatedCode {
                parent: r.u32_le()?,
                end: r.u32_le()?,
                len: r.u32_le()?,
                flags: r.u32_le()?,
                offset: r.u32_le()?,
                parent_offset: r.u32_le()?,
                segment: r.u16_le()?,
                parent_segment: r.u16_le()?,
            },
            S_ANNOTATION => {
                let offset = r.u32_le()?;
                let segment = r.u16_le()?;
                let count = r.u16_le()?;
                Symbol::Annotation {
                    offset,
                    segment,
                    strings: r.remaining(),
                    count,
                }
            }
            S_INLINESITE => Symbol::InlineSite {
                parent: r.u32_le()?,
                end: r.u32_le()?,
                inlinee: ty(r)?,
                annotations: r.remaining(),
            },
            // S_INLINESITE2 adds the number of invocations.
            S_INLINESITE2 => {
                let parent = r.u32_le()?;
                let end = r.u32_le()?;
                let inlinee = ty(r)?;
                let _invocations = r.u32_le()?;
                Symbol::InlineSite {
                    parent,
                    end,
                    inlinee,
                    annotations: r.remaining(),
                }
            }
            S_DEFRANGE..=S_DEFRANGE_REGISTER_REL => Symbol::DefRange {
                kind: self.kind,
                data: self.data,
            },
            S_END | S_PROC_ID_END | S_INLINESITE_END => Symbol::End { kind: self.kind },
            S_POGODATA => Symbol::PogoData {
                invocations: r.u32_le()?,
                dynamic_instructions: r.u64_le()?,
                instructions: r.u32_le()?,
                live_instructions: r.u32_le()?,
            },
            _ => Symbol::Other {
                kind: self.kind,
                data: self.data,
            },
        })
    }

    /// The symbol's name, if it has one.
    pub fn name(&self) -> Option<&'a [u8]> {
        Some(match self.parse().ok()? {
            Symbol::Public { name, .. }
            | Symbol::Procedure { name, .. }
            | Symbol::Data { name, .. }
            | Symbol::Udt { name, .. }
            | Symbol::Constant { name, .. }
            | Symbol::Reference { name, .. }
            | Symbol::RegisterRelative { name, .. }
            | Symbol::BpRelative { name, .. }
            | Symbol::Register { name, .. }
            | Symbol::Local { name, .. }
            | Symbol::Block { name, .. }
            | Symbol::Label { name, .. }
            | Symbol::Thunk { name, .. }
            | Symbol::ObjName { name, .. }
            | Symbol::Section { name, .. }
            | Symbol::CoffGroup { name, .. } => name,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record of `kind` with `body`, as it appears in a symbol stream.
    fn record(kind: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn inline_sites_and_dpc_procedures() {
        let mut body = Vec::new();
        for v in [0x10u32, 0x60, 0x1234] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        let annotations = [0x0Bu8, 0x02, 0x00, 0x00];
        let mut one = body.clone();
        one.extend_from_slice(&annotations);
        let mut two = body.clone();
        two.extend_from_slice(&5u32.to_le_bytes()); // invocations
        two.extend_from_slice(&annotations);
        for (k, b) in [(kind::S_INLINESITE, one), (kind::S_INLINESITE2, two)] {
            let data = record(k, &b);
            let rec = SymbolIter::new(&data, 0).next().unwrap();
            assert!(kind::opens_scope(rec.kind), "{}", kind::name(rec.kind));
            match rec.parse().unwrap() {
                Symbol::InlineSite {
                    parent,
                    end,
                    inlinee,
                    annotations: a,
                } => {
                    assert_eq!((parent, end, inlinee), (0x10, 0x60, TypeIndex(0x1234)));
                    assert_eq!(a, annotations);
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(kind::name(kind::S_INLINESITE2), "S_INLINESITE2");
        // A DPC procedure has a procedure's layout, and is local.
        let mut body = Vec::new();
        for v in [0u32, 0, 0, 0x20, 0, 0, 0x1001, 0x40] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&1u16.to_le_bytes());
        body.push(0);
        body.extend_from_slice(b"dpc");
        body.push(0);
        for k in [kind::S_LPROC32_DPC, kind::S_LPROC32_DPC_ID] {
            let data = record(k, &body);
            let rec = SymbolIter::new(&data, 0).next().unwrap();
            assert!(kind::opens_scope(rec.kind));
            match rec.parse().unwrap() {
                Symbol::Procedure {
                    global,
                    len,
                    name,
                    id,
                    ..
                } => {
                    assert!(!global);
                    assert_eq!(id, k == kind::S_LPROC32_DPC_ID);
                    assert_eq!((len, name), (0x20, &b"dpc"[..]));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn pogo_data() {
        let mut body = Vec::new();
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&0x1_0000_0002u64.to_le_bytes());
        body.extend_from_slice(&40u32.to_le_bytes());
        body.extend_from_slice(&31u32.to_le_bytes());
        let data = record(kind::S_POGODATA, &body);
        let rec = SymbolIter::new(&data, 0).next().unwrap();
        match rec.parse().unwrap() {
            Symbol::PogoData {
                invocations,
                dynamic_instructions,
                instructions,
                live_instructions,
            } => assert_eq!(
                (
                    invocations,
                    dynamic_instructions,
                    instructions,
                    live_instructions
                ),
                (7, 0x1_0000_0002, 40, 31)
            ),
            other => panic!("{other:?}"),
        }
        // Cut short: an error, not a guess.
        let data = record(kind::S_POGODATA, &body[..10]);
        let rec = SymbolIter::new(&data, 0).next().unwrap();
        assert!(rec.parse().is_err());
    }

    #[test]
    fn separated_code() {
        let mut body = Vec::new();
        for v in [0x10u32, 0x60, 0x40, 0x2, 0x1234, 0x100] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes());
        let data = record(kind::S_SEPCODE, &body);
        let rec = SymbolIter::new(&data, 0).next().unwrap();
        assert!(kind::opens_scope(rec.kind));
        assert_eq!(kind::name(rec.kind), "S_SEPCODE");
        match rec.parse().unwrap() {
            Symbol::SeparatedCode {
                parent,
                end,
                len,
                flags,
                offset,
                parent_offset,
                segment,
                parent_segment,
            } => {
                assert_eq!((parent, end, len, flags), (0x10, 0x60, 0x40, 2));
                assert_eq!((offset, parent_offset), (0x1234, 0x100));
                assert_eq!((segment, parent_segment), (1, 2));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rec.name(), None);
        // Truncated records are errors.
        for len in 0..body.len() {
            let data = record(kind::S_SEPCODE, &body[..len]);
            let rec = SymbolIter::new(&data, 0).next().unwrap();
            assert!(rec.parse().is_err(), "{len}");
        }
    }
}
