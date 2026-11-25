//! Where a symbol record refers to something outside itself: its type and
//! id indices, its addresses (which an object relocates), its string-table
//! and file-checksum offsets, and its links to other records.
//!
//! The counterpart of [`type_refs`](crate::type_refs) for symbols, and
//! exhaustive in the same way: a kind it doesn't know is an error
//! ([`FieldError::Unknown`]), never "no fields". Remapping a record's
//! indices, moving it to another object, or rebuilding the string table
//! and checksums it names all need every field, and a missed one corrupts
//! debug information silently.
//!
//! The layouts are cvinfo.h's (Microsoft's published copy), checked against
//! LLVM's `SymbolRecordMapping.cpp` and `TypeIndexDiscovery.cpp`; where they
//! disagree, the comment at the kind says which is followed and why. Kinds
//! after cvinfo.h's last (`S_LDATA_HLSL32_EX`) are LLVM's, and those whose
//! layout neither documents are unknown.

use crate::symbols::kind::*;
use crate::types::read_numeric;
use turbonop_core::Reader;

/// One field of a symbol record. Offsets are in the record's body (after
/// its kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    /// A type index (4 bytes).
    Type(u32),
    /// An id index (4 bytes): an `LF_FUNC_ID`, `LF_MFUNC_ID`,
    /// `LF_BUILDINFO`, ... In an object, ids and types share one index
    /// space; a PDB keeps ids in its IPI stream.
    Id(u32),
    /// An address: a 4-byte offset in a section, at `offset`, and the
    /// section's 2-byte number, at `section`. An object relocates the pair
    /// against a symbol (`SECREL` and `SECTION`).
    Address {
        /// Where the offset is.
        offset: u32,
        /// Where the section number is.
        section: u32,
    },
    /// A 4-byte offset in the string table (an object's
    /// `DEBUG_S_STRINGTABLE`, a PDB's `/names`).
    String(u32),
    /// A 4-byte offset of an entry in the file checksums
    /// (`DEBUG_S_FILECHKSMS`).
    Checksum(u32),
    /// Binary annotations, from here to the record's end. Their
    /// `BA_OP_ChangeFile` operands are file checksum offsets
    /// ([`annotations::files`](crate::annotations::files)), compressed, so
    /// changing one can change the record's length.
    Annotations(u32),
    /// A 4-byte offset of another record in the same symbol stream: a
    /// scope's parent, its end, or the next scope. Objects leave these 0
    /// (the linker fills them in).
    Link(u32),
    /// A 4-byte metadata token (managed code): neither an index nor an
    /// offset.
    Token(u32),
    /// A 4-byte image-relative address, which an object relocates
    /// (`ADDR32NB`): only frame data's base ([`c13::fields`](crate::c13::fields)).
    Rva(u32),
}

impl Field {
    /// The field's offset in the body (for an address, its offset part's).
    pub fn offset(self) -> u32 {
        match self {
            Field::Type(o)
            | Field::Id(o)
            | Field::String(o)
            | Field::Checksum(o)
            | Field::Annotations(o)
            | Field::Link(o)
            | Field::Token(o)
            | Field::Rva(o) => o,
            Field::Address { offset, .. } => offset,
        }
    }

    /// The field moved `by` bytes later (for offsets within a larger
    /// buffer than the record's body).
    #[must_use]
    pub fn shifted(self, by: u32) -> Field {
        match self {
            Field::Type(o) => Field::Type(o + by),
            Field::Id(o) => Field::Id(o + by),
            Field::String(o) => Field::String(o + by),
            Field::Checksum(o) => Field::Checksum(o + by),
            Field::Annotations(o) => Field::Annotations(o + by),
            Field::Link(o) => Field::Link(o + by),
            Field::Token(o) => Field::Token(o + by),
            Field::Rva(o) => Field::Rva(o + by),
            Field::Address { offset, section } => Field::Address {
                offset: offset + by,
                section: section + by,
            },
        }
    }
}

/// Why a symbol record's fields could not be found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// A kind whose layout isn't known.
    Unknown(u16),
    /// The record is shorter than its kind's fields.
    Malformed(u16),
    /// A kind from before 32-bit type indices (`*_16t`, and the 16:16
    /// kinds), whose type indices are 16 bits wide. Known, but not
    /// supported: no compiler since Visual C++ 6 writes them.
    Legacy(u16),
}

impl core::fmt::Display for FieldError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FieldError::Unknown(k) => write!(f, "unknown symbol record kind {k:#06x}"),
            FieldError::Malformed(k) => {
                write!(f, "malformed symbol record ({} {k:#06x})", name(*k))
            }
            FieldError::Legacy(k) => {
                write!(f, "16-bit symbol record kind {} ({k:#06x})", name(*k))
            }
        }
    }
}

/// `REFMINIPDB`'s flag that its first field is a type index (`fUDT`),
/// rather than a section.
const REF_MINIPDB_UDT: u16 = 1 << 2;

struct Out<'b, F> {
    body: &'b [u8],
    kind: u16,
    visit: F,
}

impl<F: FnMut(Field)> Out<'_, F> {
    fn fits(&self, at: usize, size: usize) -> Result<(), FieldError> {
        match at.checked_add(size) {
            Some(end) if end <= self.body.len() => Ok(()),
            _ => Err(FieldError::Malformed(self.kind)),
        }
    }

    fn four(&mut self, at: usize, field: Field) -> Result<(), FieldError> {
        self.fits(at, 4)?;
        (self.visit)(field);
        Ok(())
    }

    fn ty(&mut self, at: usize) -> Result<(), FieldError> {
        self.four(at, Field::Type(at as u32))
    }

    fn id(&mut self, at: usize) -> Result<(), FieldError> {
        self.four(at, Field::Id(at as u32))
    }

    fn string(&mut self, at: usize) -> Result<(), FieldError> {
        self.four(at, Field::String(at as u32))
    }

    fn checksum(&mut self, at: usize) -> Result<(), FieldError> {
        self.four(at, Field::Checksum(at as u32))
    }

    fn token(&mut self, at: usize) -> Result<(), FieldError> {
        self.four(at, Field::Token(at as u32))
    }

    /// Links at each of `at`.
    fn links(&mut self, at: &[usize]) -> Result<(), FieldError> {
        for &a in at {
            self.four(a, Field::Link(a as u32))?;
        }
        Ok(())
    }

    fn address(&mut self, offset: usize, section: usize) -> Result<(), FieldError> {
        self.fits(offset, 4)?;
        self.fits(section, 2)?;
        (self.visit)(Field::Address {
            offset: offset as u32,
            section: section as u32,
        });
        Ok(())
    }

    fn annotations(&mut self, at: usize) -> Result<(), FieldError> {
        self.fits(at, 0)?;
        (self.visit)(Field::Annotations(at as u32));
        Ok(())
    }

    fn u16_at(&self, at: usize) -> Result<u16, FieldError> {
        self.fits(at, 2)?;
        Ok(u16::from_le_bytes([self.body[at], self.body[at + 1]]))
    }

    fn u32_at(&self, at: usize) -> Result<u32, FieldError> {
        self.fits(at, 4)?;
        let b = &self.body[at..at + 4];
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Calls `visit` with each field of the record of kind `kind` whose body
/// (the bytes after its kind) is `body`, in the order they appear.
///
/// On an error, `visit` may already have seen the fields before the one
/// that didn't fit.
pub fn fields(kind: u16, body: &[u8], visit: impl FnMut(Field)) -> Result<(), FieldError> {
    let mut o = Out { body, kind, visit };
    match kind {
        // Records without fields: compiler and object information, the
        // environment, names, PDB-only references (`ibSym` is an offset in
        // another module's stream), scope ends, and frame facts.
        S_COMPILE
        | S_SKIP
        | S_CVRESERVE
        | S_OBJNAME_ST
        | S_ENDARG
        | S_RETURN
        | S_ENTRYTHIS
        | S_SLINK32
        | S_PROCREF_ST
        | S_DATAREF_ST
        | S_ALIGN
        | S_LPROCREF_ST
        | S_COMPILE2_ST
        | S_UNAMESPACE_ST
        | S_OBJNAME
        | S_COMPILE2
        | S_COMPILE3
        | S_ENVBLOCK
        | S_UNAMESPACE
        | S_PROCREF
        | S_DATAREF
        | S_LPROCREF
        | S_ANNOTATIONREF
        | S_TOKENREF
        | S_SECTION
        | S_EXPORT
        | S_FRAMECOOKIE
        | S_END
        | S_INLINESITE_END
        | S_PROC_ID_END
        | S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE
        | S_POGODATA
        | S_MOD_TYPEREF
        | S_PDBMAP => Ok(()),
        // tagValue(4) symRecordOffset(4), each: the offsets are from the
        // enclosing S_LPROC32_DPC, not the stream's start.
        S_DPC_SYM_TAG_MAP => Ok(()),

        // startsym(4) seg(2): the procedure's record, by offset.
        S_SSEARCH => o.links(&[0]),
        // idOem(16) typind(4) data
        S_OEM => o.ty(16),
        // off(4) seg(2) model(2) variant
        S_CEXMODEL32 => o.address(0, 4),

        // pParent(4) pEnd(4) pNext(4) off(4) seg(2) len(2) ord(1) name variant
        S_THUNK32 | S_THUNK32_ST => {
            o.links(&[0, 4, 8])?;
            o.address(12, 16)
        }
        // pParent(4) pEnd(4) len(4) off(4) seg(2) name
        S_BLOCK32 | S_BLOCK32_ST | S_WITH32 | S_WITH32_ST => {
            o.links(&[0, 4])?;
            o.address(12, 16)
        }
        // off(4) seg(2) flags(1) name
        S_LABEL32 | S_LABEL32_ST => o.address(0, 4),
        // typind(4) reg(2) name
        S_REGISTER | S_REGISTER_ST => o.ty(0),
        // typind(4) value(numeric) name
        S_CONSTANT | S_CONSTANT_ST => {
            o.ty(0)?;
            numeric_at(&o, 4)
        }
        // token(4) value(numeric) name. (LLVM maps it as S_CONSTANT, with a
        // type index; cvinfo.h: "Type index ... or metadata token", and
        // the managed kinds name metadata.)
        S_MANCONSTANT => {
            o.token(0)?;
            numeric_at(&o, 4)
        }
        // typind(4) name
        S_UDT | S_UDT_ST | S_COBOLUDT | S_COBOLUDT_ST => o.ty(0),
        // typind(4) count(1 or 2) regs name
        S_MANYREG | S_MANYREG_ST | S_MANYREG2 | S_MANYREG2_ST => o.ty(0),
        // off(4) typind(4) name
        S_BPREL32 | S_BPREL32_ST => o.ty(4),
        // typind(4) off(4) seg(2) name
        S_LDATA32 | S_LDATA32_ST | S_GDATA32 | S_GDATA32_ST | S_LTHREAD32 | S_LTHREAD32_ST
        | S_GTHREAD32 | S_GTHREAD32_ST => {
            o.ty(0)?;
            o.address(4, 8)
        }
        // token(4) off(4) seg(2) name. (cvinfo.h: "Type index, or Metadata
        // token if a managed symbol"; LLVM maps it as S_LDATA32.)
        S_LMANDATA | S_LMANDATA_ST | S_GMANDATA | S_GMANDATA_ST => {
            o.token(0)?;
            o.address(4, 8)
        }
        // flags(4) off(4) seg(2) name
        S_PUB32 | S_PUB32_ST => o.address(4, 8),
        // pParent(4) pEnd(4) pNext(4) len(4) DbgStart(4) DbgEnd(4)
        // typind(4) off(4) seg(2) flags(1) name
        S_LPROC32 | S_LPROC32_ST | S_GPROC32 | S_GPROC32_ST | S_LPROC32_DPC => {
            // (S_LPROC32_DPC names a type, as S_LPROC32 does; its _ID form
            // names an id. LLVM's TypeIndexDiscovery has S_LPROC32_DPC
            // naming an id, which cvinfo.h's pairing of the kinds and
            // S_LPROC32_DPC_ID's existence contradict.)
            o.links(&[0, 4, 8])?;
            o.ty(24)?;
            o.address(28, 32)
        }
        S_LPROC32_ID | S_GPROC32_ID | S_LPROC32_DPC_ID => {
            o.links(&[0, 4, 8])?;
            o.id(24)?;
            o.address(28, 32)
        }
        // root(4) path(4) off(4) seg(2)
        S_VFTABLE32 => {
            o.ty(0)?;
            o.ty(4)?;
            o.address(8, 12)
        }
        // off(4) typind(4) reg(2) name
        S_REGREL32 | S_REGREL32_ST => o.ty(4),
        // off(4) typind(4) offInUdt(4) reg(2) name (LLVM's layout; not in
        // cvinfo.h)
        S_REGREL32_INDIR => o.ty(4),
        // pParent(4) pEnd(4) pNext(4) len(4) DbgStart(4) DbgEnd(4)
        // regSave(4) fpSave(4) intOff(4) fpOff(4) typind(4) off(4) seg(2)
        // retReg(1) frameReg(1) name
        S_LPROCMIPS | S_LPROCMIPS_ST | S_GPROCMIPS | S_GPROCMIPS_ST => {
            o.links(&[0, 4, 8])?;
            o.ty(40)?;
            o.address(44, 48)
        }
        S_LPROCMIPS_ID | S_GPROCMIPS_ID => {
            o.links(&[0, 4, 8])?;
            o.id(40)?;
            o.address(44, 48)
        }
        // pParent(4) pEnd(4) pNext(4) len(4) DbgStart(4) DbgEnd(4)
        // typind(4) off(4) seg(2) retReg(2) flags(1) name
        S_LPROCIA64 | S_LPROCIA64_ST | S_GPROCIA64 | S_GPROCIA64_ST => {
            o.links(&[0, 4, 8])?;
            o.ty(24)?;
            o.address(28, 32)
        }
        S_LPROCIA64_ID | S_GPROCIA64_ID => {
            o.links(&[0, 4, 8])?;
            o.id(24)?;
            o.address(28, 32)
        }
        // cbFrame(4) cbPad(4) offPad(4) cbSaveRegs(4) offExHdlr(4)
        // sectExHdlr(2) flags(4)
        S_FRAMEPROC => o.address(16, 20),
        // iSlot(4) typind(4) name
        S_LOCALSLOT | S_LOCALSLOT_ST | S_PARAMSLOT | S_PARAMSLOT_ST => o.ty(4),
        // off(4) seg(2) csz(2) strings
        S_ANNOTATION => o.address(0, 4),
        // pParent(4) pEnd(4) pNext(4) len(4) DbgStart(4) DbgEnd(4)
        // token(4) off(4) seg(2) flags(1) retReg(2) name
        S_GMANPROC | S_GMANPROC_ST | S_LMANPROC | S_LMANPROC_ST => {
            o.links(&[0, 4, 8])?;
            o.token(24)?;
            o.address(28, 32)
        }
        // off(4) typind(4) attr: off(4) seg(2) flags(2); name
        S_ATTR_FRAMEREL => {
            o.ty(4)?;
            o.address(8, 12)
        }
        S_MANFRAMEREL | S_MANFRAMEREL_ST => {
            o.token(4)?;
            o.address(8, 12)
        }
        // typind(4) attr: off(4) seg(2) flags(2); reg(2) name
        // typind(4) attr: off(4) seg(2) flags(2); count regs name
        S_ATTR_REGISTER | S_ATTR_MANYREG => {
            o.ty(0)?;
            o.address(4, 8)
        }
        S_MANREGISTER | S_MANREGISTER_ST | S_MANMANYREG | S_MANMANYREG_ST | S_MANMANYREG2
        | S_MANMANYREG2_ST => {
            o.token(0)?;
            o.address(4, 8)
        }
        // iSlot(4) token(4) attr: off(4) seg(2) flags(2); name
        S_MANSLOT | S_MANSLOT_ST => {
            o.token(4)?;
            o.address(8, 12)
        }
        // off(4) typind(4) reg(2) attr: off(4) seg(2) flags(2); name
        S_ATTR_REGREL => {
            o.ty(4)?;
            o.address(10, 14)
        }
        S_MANREGREL | S_MANREGREL_ST => {
            o.token(4)?;
            o.address(10, 14)
        }
        // typind(4): "Index for type referenced by name from metadata"
        S_MANTYPREF => o.ty(0),
        // trampType(2) cbThunk(2) offThunk(4) offTarget(4) sectThunk(2)
        // sectTarget(2)
        S_TRAMPOLINE => {
            o.address(4, 12)?;
            o.address(8, 14)
        }
        // pParent(4) pEnd(4) length(4) scf(4) off(4) offParent(4) sect(2)
        // sectParent(2)
        S_SEPCODE => {
            o.links(&[0, 4])?;
            o.address(16, 24)?;
            o.address(20, 26)
        }
        // cb(4) characteristics(4) off(4) seg(2) name
        S_COFFGROUP => o.address(8, 12),
        // off(4) sect(2) reserved(2) typind(4): the call's signature
        // off(4) sect(2) cbInstr(2) typind(4): the allocated type
        S_CALLSITEINFO | S_HEAPALLOCSITE => {
            o.address(0, 4)?;
            o.ty(8)
        }
        // discarded(1) reserved(3) fileid(4) linenum(4) data: the original
        // records, with their type indices invalid (cvinfo.h), so not
        // reported.
        S_DISCARDED => o.checksum(4),
        // typind(4) flags(2) name
        S_LOCAL => o.ty(0),
        // typind(4) modOffset(4) flags(2) name
        S_FILESTATIC => {
            o.ty(0)?;
            o.string(4)
        }
        // program(4) range: offStart(4) isectStart(2) cbRange(2); gaps.
        // (`program` is "DIA program to evaluate the value of the symbol";
        // cvdump prints it as a name index, an offset in the string table.
        // LLVM keeps it as a plain integer.)
        S_DEFRANGE => {
            o.string(0)?;
            o.address(4, 8)
        }
        // program(4) offParent(4) range gaps
        S_DEFRANGE_SUBFIELD => {
            o.string(0)?;
            o.address(8, 12)
        }
        // reg(2) attr(2) range gaps
        // offFramePointer(4) range gaps
        S_DEFRANGE_REGISTER | S_DEFRANGE_FRAMEPOINTER_REL => o.address(4, 8),
        // reg(2) attr(2) offParent(4) range gaps
        // baseReg(2) flags(2) offBasePointer(4) range gaps
        S_DEFRANGE_SUBFIELD_REGISTER | S_DEFRANGE_REGISTER_REL => o.address(8, 12),
        // regType(2) flags(2) offsetParent(2) sizeInParent(2) range data
        S_DEFRANGE_HLSL | S_DEFRANGE_DPC_PTR_TAG => o.address(8, 12),
        // baseReg(2) flags(2) offBasePointer(4) offInUdt(4) range gaps
        // (LLVM's layout)
        S_DEFRANGE_REGISTER_REL_INDIR => o.address(12, 16),
        // id(4)
        S_BUILDINFO => o.id(0),
        // pParent(4) pEnd(4) inlinee(4) annotations
        S_INLINESITE => {
            o.links(&[0, 4])?;
            o.id(8)?;
            o.annotations(12)
        }
        // pParent(4) pEnd(4) inlinee(4) invocations(4) annotations. (Not in
        // LLVM's TypeIndexDiscovery, so lld would leave its inlinee
        // unmapped.)
        S_INLINESITE2 => {
            o.links(&[0, 4])?;
            o.id(8)?;
            o.annotations(16)
        }
        // typind(4) ...
        S_GDATA_HLSL
        | S_LDATA_HLSL
        | S_GDATA_HLSL32
        | S_LDATA_HLSL32
        | S_GDATA_HLSL32_EX
        | S_LDATA_HLSL32_EX
        | S_LOCAL_DPC_GROUPSHARED => o.ty(0),
        // offsetBase(4) sectBase(2) switchType(2) offsetBranch(4)
        // offsetTable(4) sectBranch(2) sectTable(2) cEntries(4)
        S_ARMSWITCHTABLE => {
            o.address(0, 4)?;
            o.address(8, 16)?;
            o.address(12, 18)
        }
        // count(4) funcs(4 × count) [invocations(4 × count)]. cvinfo.h's
        // `CV_typ_t`; LLVM's ids (and lld maps them so): functions, which
        // an object names by LF_FUNC_ID.
        S_CALLEES | S_CALLERS | S_INLINEES => {
            let n = o.u32_at(0)? as usize;
            let end = n
                .checked_mul(4)
                .and_then(|b| b.checked_add(4))
                .ok_or(FieldError::Malformed(kind))?;
            o.fits(0, end)?;
            for i in 0..n {
                o.id(4 + 4 * i)?;
            }
            Ok(())
        }
        // isectCoff or typind(4) imod(2) flags(2) name: a type index only
        // for a UDT's.
        S_REF_MINIPDB => {
            if o.u16_at(6)? & REF_MINIPDB_UDT != 0 {
                o.ty(0)
            } else {
                Ok(())
            }
        }
        // function(4) name: the function's id (LLVM emits
        // `getFuncIdForSubprogram`). Neither TypeIndexDiscovery nor
        // cvinfo.h has it; llvm-pdbutil prints it as a type.
        S_HOTPATCHFUNC => o.id(0),
        // kind(2) off(4) seg(2) (LLVM's layout)
        S_ASSOCIATION => o.address(2, 6),

        // Before 32-bit type indices.
        S_REGISTER_16T | S_CONSTANT_16T | S_UDT_16T | S_COBOLUDT_16T | S_MANYREG_16T
        | S_BPREL16 | S_LDATA16 | S_GDATA16 | S_PUB16 | S_LPROC16 | S_GPROC16 | S_THUNK16
        | S_BLOCK16 | S_WITH16 | S_LABEL16 | S_CEXMODEL16 | S_VFTABLE16 | S_REGREL16
        | S_BPREL32_16T | S_LDATA32_16T | S_GDATA32_16T | S_PUB32_16T | S_LPROC32_16T
        | S_GPROC32_16T | S_VFTABLE32_16T | S_REGREL32_16T | S_LTHREAD32_16T | S_GTHREAD32_16T
        | S_LPROCMIPS_16T | S_GPROCMIPS_16T => Err(FieldError::Legacy(kind)),

        // Named, but with no layout documented anywhere this follows:
        // cvinfo.h's reserved and 2005 kinds, and LLVM's later kinds that
        // it only names. S_FASTLINK is LLVM's "undocumented (also known as
        // S_REF_MINIPDB2)".
        S_RESERVED1
        | S_RESERVED2
        | S_RESERVED3
        | S_RESERVED4
        | S_LOCAL_2005
        | S_DEFRANGE_2005
        | S_DEFRANGE2_2005
        | S_FRAMEREG
        | S_FASTLINK
        | S_BPREL32_INDIR
        | S_GPROC32EX
        | S_LPROC32EX
        | S_GPROC32EX_ID
        | S_LPROC32EX_ID
        | S_STATICLOCAL
        | S_BPREL32_ENCTMP
        | S_REGREL32_ENCTMP
        | S_BPREL32_INDIR_ENCTMP
        | S_REGREL32_INDIR_ENCTMP
        | S_SOURCELINK
        | S_DEFRANGE_CONSTVAL_ON_ENTRY
        | S_DEFRANGE_GLOBALSYM_ON_ENTRY
        | S_ALTOBJNAME => Err(FieldError::Unknown(kind)),

        _ => Err(FieldError::Unknown(kind)),
    }
}

/// Checks that a numeric leaf (`S_CONSTANT`'s value) is whole at `at`.
fn numeric_at<F>(o: &Out<'_, F>, at: usize) -> Result<(), FieldError> {
    let rest = o.body.get(at..).ok_or(FieldError::Malformed(o.kind))?;
    read_numeric(&mut Reader::new(rest))
        .map(|_| ())
        .map_err(|_| FieldError::Malformed(o.kind))
}

/// The fields of a record, in order (see [`fields`]).
pub fn fields_of(kind: u16, body: &[u8]) -> Result<Vec<Field>, FieldError> {
    let mut out = Vec::new();
    fields(kind, body, |f| out.push(f))?;
    Ok(out)
}

/// The type and id index fields of a record, appended to `refs` in order:
/// the same [`Ref`](crate::type_refs::Ref)s that
/// [`type_refs::refs`](crate::type_refs::refs) gives for type records.
pub fn refs(
    kind: u16,
    body: &[u8],
    refs: &mut Vec<crate::type_refs::Ref>,
) -> Result<(), FieldError> {
    use crate::type_refs::Ref;
    fields(kind, body, |f| match f {
        Field::Type(offset) => refs.push(Ref { offset, id: false }),
        Field::Id(offset) => refs.push(Ref { offset, id: true }),
        _ => {}
    })
}

#[cfg(test)]
mod tests;
