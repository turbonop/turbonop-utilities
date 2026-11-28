//! Metadata (`METADATA_BLOCK`, `METADATA_KIND_BLOCK`): strings, nodes,
//! named metadata, and the debug-information records, of which the
//! compile units, files, and subprograms are interpreted (enough to name
//! each function's source).

use alloc::borrow::Cow;
use alloc::vec::Vec;

use turbonop_core::Result;

use crate::bitstream::{BitReader, Cursor, Entry, idx, low32};
use crate::tables::*;
use crate::types::chars;

/// One metadata entry, by metadata ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataNode<'d> {
    /// A string (`!"..."`).
    String(Cow<'d, [u8]>),
    /// A value as metadata (`i32 1`, `ptr @g`): its type and value ID.
    Value {
        /// The type.
        ty: u32,
        /// The value ID.
        value: u32,
    },
    /// A node: a generic tuple (`METADATA_NODE`, `METADATA_DISTINCT_NODE`),
    /// a location, or a debug-information record (`DIFile`,
    /// `DISubprogram`, ...), with the record's operands as written
    /// (metadata references are IDs plus one; 0 is null).
    Node {
        /// The record code (`METADATA_*`).
        code: u32,
        /// The operands.
        ops: Vec<u64>,
    },
}

/// A file (`DIFile`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiFile<'a> {
    /// The file name.
    pub filename: &'a [u8],
    /// The directory.
    pub directory: &'a [u8],
}

/// A compile unit (`DICompileUnit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileUnit<'a> {
    /// Its metadata ID.
    pub id: u32,
    /// The source language (`DW_LANG_*`).
    pub language: u64,
    /// The main file.
    pub file: Option<DiFile<'a>>,
    /// The producer (`clang version ...`).
    pub producer: &'a [u8],
    /// Whether it was optimized.
    pub optimized: bool,
    /// The emission kind (0 none, 1 full, 2 line tables only, 3 directives
    /// only).
    pub emission_kind: u64,
}

/// A subprogram (`DISubprogram`): a function's debug information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subprogram<'a> {
    /// Its metadata ID.
    pub id: u32,
    /// Its name.
    pub name: &'a [u8],
    /// Its linkage (mangled) name.
    pub linkage_name: &'a [u8],
    /// Its file.
    pub file: Option<DiFile<'a>>,
    /// Its line.
    pub line: u64,
    /// Whether it's a definition (not a declaration).
    pub definition: bool,
    /// Its compile unit's metadata ID.
    pub unit: Option<u32>,
}

/// A module's metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata<'d> {
    /// Every entry, by metadata ID.
    pub nodes: Vec<MetadataNode<'d>>,
    /// Named metadata (`!llvm.dbg.cu = !{...}`): name and node IDs.
    pub named: Vec<(Vec<u8>, Vec<u32>)>,
    /// Metadata kinds (`!dbg`, `!tbaa`, ...): ID and name.
    pub kinds: Vec<(u32, Vec<u8>)>,
    /// Global objects' attachments (`METADATA_GLOBAL_DECL_ATTACHMENT`):
    /// value ID, and kind and node pairs.
    pub global_attachments: Vec<(u32, Vec<(u32, u32)>)>,
}

/// Whether a metadata record defines an entry (takes the next ID), as
/// LLVM's `MetadataLoader` numbers them.
fn defines_node(code: u32) -> bool {
    matches!(
        code,
        METADATA_VALUE
            | METADATA_NODE
            | METADATA_DISTINCT_NODE
            | METADATA_LOCATION
            | METADATA_OLD_NODE
            | METADATA_OLD_FN_NODE
            | METADATA_GENERIC_DEBUG..=METADATA_MACRO_FILE
            | METADATA_GLOBAL_VAR_EXPR
            | METADATA_LABEL
            | METADATA_STRING_TYPE
            | METADATA_COMMON_BLOCK..=METADATA_LAYERLOCLIST
    )
}

/// A metadata reference (ID plus one; 0 for null).
fn reference(v: u64) -> Option<u32> {
    v.checked_sub(1).map(low32)
}

/// Metadata to look things up in: a module's, and a function's own (its
/// IDs follow the module's).
#[derive(Clone, Copy, Debug)]
pub struct MetadataView<'a, 'd> {
    /// The module's.
    pub module: &'a Metadata<'d>,
    /// The function's.
    pub local: &'a [MetadataNode<'d>],
}

impl<'a, 'd> MetadataView<'a, 'd> {
    /// Entry `id`.
    pub fn get(&self, id: u32) -> Option<&'a MetadataNode<'d>> {
        let n = self.module.nodes.len();
        match (id as usize).checked_sub(n) {
            None => self.module.nodes.get(id as usize),
            Some(i) => self.local.get(i),
        }
    }

    /// The string entry `id` is.
    pub fn string(&self, id: u32) -> Option<&'a [u8]> {
        match self.get(id)? {
            MetadataNode::String(s) => Some(s),
            _ => None,
        }
    }

    /// The string an operand (an ID plus one) refers to; empty for null.
    fn string_ref(&self, v: u64) -> &'a [u8] {
        reference(v).and_then(|id| self.string(id)).unwrap_or(&[])
    }

    /// Node `id`'s record code and operands.
    pub fn node(&self, id: u32) -> Option<(u32, &'a [u64])> {
        match self.get(id)? {
            MetadataNode::Node { code, ops } => Some((*code, ops)),
            _ => None,
        }
    }

    /// The `DIFile` node `id` is.
    pub fn file(&self, id: u32) -> Option<DiFile<'a>> {
        let (code, ops) = self.node(id)?;
        (code == METADATA_FILE).then(|| DiFile {
            filename: self.string_ref(ops.get(1).copied().unwrap_or(0)),
            directory: self.string_ref(ops.get(2).copied().unwrap_or(0)),
        })
    }

    fn file_ref(&self, v: u64) -> Option<DiFile<'a>> {
        self.file(reference(v)?)
    }

    /// The `DISubprogram` node `id` is.
    pub fn subprogram(&self, id: u32) -> Option<Subprogram<'a>> {
        let (code, r) = self.node(id)?;
        if code != METADATA_SUBPROGRAM || r.len() < 9 {
            return None;
        }
        let op = |i: usize| r.get(i).copied().unwrap_or(0);
        let has_sp_flags = op(0) & 4 != 0;
        let (definition, offset_b) = if has_sp_flags {
            // DISPFlagDefinition.
            (op(9) & 8 != 0, 0)
        } else {
            (op(8) != 0, if r.len() >= 19 { 3 } else { 2 })
        };
        let has_unit = op(0) & 2 != 0;
        Some(Subprogram {
            id,
            name: self.string_ref(op(2)),
            linkage_name: self.string_ref(op(3)),
            file: self.file_ref(op(4)),
            line: op(5),
            definition,
            unit: if has_unit {
                reference(op(12 + offset_b))
            } else {
                None
            },
        })
    }

    /// The compile unit node `id` is.
    pub fn compile_unit(&self, id: u32) -> Option<CompileUnit<'a>> {
        let (code, r) = self.node(id)?;
        if code != METADATA_COMPILE_UNIT {
            return None;
        }
        let op = |i: usize| r.get(i).copied().unwrap_or(0);
        Some(CompileUnit {
            id,
            language: op(1) & !(1u64 << 63),
            file: self.file_ref(op(2)),
            producer: self.string_ref(op(3)),
            optimized: op(4) != 0,
            emission_kind: op(8),
        })
    }

    fn ids(&self) -> impl Iterator<Item = u32> + use<> {
        (0..(self.module.nodes.len() + self.local.len()) as u64).map(low32)
    }

    /// The compile units, in metadata order.
    pub fn compile_units(&self) -> Vec<CompileUnit<'a>> {
        self.ids().filter_map(|i| self.compile_unit(i)).collect()
    }

    /// Every subprogram, in metadata order.
    pub fn subprograms(&self) -> Vec<Subprogram<'a>> {
        self.ids().filter_map(|i| self.subprogram(i)).collect()
    }

    /// Every file, in metadata order.
    pub fn files(&self) -> Vec<DiFile<'a>> {
        self.ids().filter_map(|i| self.file(i)).collect()
    }
}

impl<'d> Metadata<'d> {
    /// The module's metadata alone, to look things up in.
    pub fn view(&self) -> MetadataView<'_, 'd> {
        MetadataView {
            module: self,
            local: &[],
        }
    }

    /// Entry `id`.
    pub fn get(&self, id: u32) -> Option<&MetadataNode<'d>> {
        self.nodes.get(id as usize)
    }

    /// The string entry `id` is.
    pub fn string(&self, id: u32) -> Option<&[u8]> {
        self.view().string(id)
    }

    /// Node `id`'s record code and operands.
    pub fn node(&self, id: u32) -> Option<(u32, &[u64])> {
        self.view().node(id)
    }

    /// The ID of metadata kind `name` (`dbg` is 0).
    pub fn kind(&self, name: &[u8]) -> Option<u32> {
        self.kinds.iter().find(|(_, n)| n == name).map(|(k, _)| *k)
    }

    /// The `DIFile` node `id` is.
    pub fn file(&self, id: u32) -> Option<DiFile<'_>> {
        self.view().file(id)
    }

    /// The `DISubprogram` node `id` is.
    pub fn subprogram(&self, id: u32) -> Option<Subprogram<'_>> {
        self.view().subprogram(id)
    }

    /// The compile units, in metadata order.
    pub fn compile_units(&self) -> Vec<CompileUnit<'_>> {
        self.view().compile_units()
    }

    /// Every subprogram at module level, in metadata order (functions'
    /// own metadata can hold theirs: see
    /// [`FunctionBody::metadata`](crate::FunctionBody::metadata)).
    pub fn subprograms(&self) -> Vec<Subprogram<'_>> {
        self.view().subprograms()
    }

    /// Reads a `METADATA_BLOCK` (entered).
    pub(crate) fn read_block(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        let mut name: Option<Vec<u8>> = None;
        loop {
            let a = match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => continue,
                Entry::Record(a) => a,
            };
            ops.clear();
            let head = c.read_record(a, &mut ops)?;
            match head.code {
                METADATA_STRINGS => {
                    let count = ops.first().copied().unwrap_or(0);
                    let offset = ops.get(1).copied().unwrap_or(0);
                    let blob = head.blob.unwrap_or(&[]);
                    self.strings(blob, count, offset);
                }
                METADATA_STRING_OLD => self
                    .nodes
                    .push(MetadataNode::String(Cow::Owned(chars(&ops)))),
                METADATA_NAME => name = Some(chars(&ops)),
                METADATA_NAMED_NODE => {
                    if let Some(n) = name.take() {
                        self.named
                            .push((n, ops.iter().map(|&v| low32(v)).collect()));
                    }
                }
                METADATA_KIND => {
                    if let Some((&k, n)) = ops.split_first() {
                        self.kinds.push((low32(k), chars(n)));
                    }
                }
                METADATA_GLOBAL_DECL_ATTACHMENT => {
                    if let Some((&v, rest)) = ops.split_first() {
                        let pairs = rest
                            .chunks(2)
                            .filter(|p| p.len() == 2)
                            .map(|p| (low32(p[0]), low32(p[1])))
                            .collect();
                        self.global_attachments.push((low32(v), pairs));
                    }
                }
                METADATA_VALUE => self.nodes.push(MetadataNode::Value {
                    ty: low32(ops.first().copied().unwrap_or(0)),
                    value: low32(ops.get(1).copied().unwrap_or(0)),
                }),
                code if defines_node(code) => self.nodes.push(MetadataNode::Node {
                    code,
                    ops: ops.clone(),
                }),
                _ => {}
            }
        }
    }

    /// `METADATA_STRINGS`: `count` VBR6 lengths in a bitstream, then the
    /// characters at `offset`.
    fn strings(&mut self, blob: &'d [u8], count: u64, offset: u64) {
        let offset = idx(offset).min(blob.len());
        let mut lengths = BitReader::new(&blob[..offset]);
        let mut chars = &blob[offset..];
        for _ in 0..count {
            // Each length takes bits: the count can't outrun them.
            let Ok(n) = lengths.vbr32(6) else { break };
            let n = (n as usize).min(chars.len());
            self.nodes
                .push(MetadataNode::String(Cow::Borrowed(&chars[..n])));
            chars = &chars[n..];
        }
    }

    /// Reads a `METADATA_KIND_BLOCK` (entered).
    pub(crate) fn read_kinds(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            let a = match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => continue,
                Entry::Record(a) => a,
            };
            ops.clear();
            let head = c.read_record(a, &mut ops)?;
            if head.code == METADATA_KIND
                && let Some((&k, n)) = ops.split_first()
            {
                self.kinds.push((low32(k), chars(n)));
            }
        }
    }
}
