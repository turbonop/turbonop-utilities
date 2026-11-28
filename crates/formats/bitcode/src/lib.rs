//! LLVM bitcode: LLVM IR in its binary form, as `clang -flto` and
//! `-emit-llvm` write it, and as objects embed it (`.llvmbc`,
//! `__LLVM,__bitcode`) and archives of LTO objects hold it.
//!
//! Two layers:
//!
//! - [`bitstream`]: the container, a generic reader of any LLVM bitstream
//!   (IR, clang's serialized diagnostics, optimization remarks): the
//!   bitcode wrapper, abbreviations of every kind, `BLOCKINFO`, nested
//!   blocks, records. [`dump`] writes a stream as `llvm-bcanalyzer`
//!   does.
//! - [`Bitcode`]: the IR. Each module's identification, triple, data
//!   layout, types, attributes, globals, functions, aliases, ifuncs,
//!   comdats, constants, metadata (with the debug information that names
//!   sources and functions), operand bundle tags and sync scopes; every
//!   function body's instructions, decoded (relative value IDs, forward
//!   references, types), with what each calls and refers to and the size
//!   of its block; and the string and symbol tables (the `irsymtab` LTO
//!   links with).
//!
//! [`embedded`] finds bitcode in ELF, Mach-O, COFF, and WebAssembly
//! objects and in archives.
//!
//! Bitcode from other LLVM versions is read as LLVM reads it: records
//! and blocks this reader doesn't know are skipped (with their
//! abbreviations), and old records' layouts are understood. Malformed
//! input gives an error or less, never a panic: every read is
//! bounds-checked. Function bodies are decoded on every core.
//!
//! The record codes and names come from LLVM's headers and
//! llvm-bcanalyzer (see `tables` and LICENSE-LLVM).

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

pub mod bitstream;
pub mod dump;
pub mod embedded;
mod function;
mod ir;
mod metadata;
pub mod symtab;
pub mod tables;
pub mod types;

pub use bitstream::{Stream, StreamKind, Wrapper, is_bitcode};
pub use function::{Call, Callee, FunctionBody, Instruction, Opcode, Operand};
pub use ir::{
    Alias, AliasKind, Attribute, AttributeGroup, Bitcode, Comdat, Constant, ConstantKind,
    DllStorage, Function, GlobalVar, Identification, Linkage, Module, ThreadLocal, UnnamedAddr,
    ValueRef, Visibility,
};
pub use metadata::{CompileUnit, DiFile, Metadata, MetadataNode, MetadataView, Subprogram};
pub use symtab::{Symtab, SymtabComdat, SymtabModule, SymtabSymbol};
pub use types::{Type, TypeTable};

#[cfg(test)]
mod tests;
