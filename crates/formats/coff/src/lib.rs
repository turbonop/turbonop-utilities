//! COFF objects as values: read an object into an editable model, change
//! it, and write it back.
//!
//! [`Object`] holds an object's headers, [`Section`]s (data, relocations,
//! line numbers), and [`Symbol`]s, each with its auxiliary records in a
//! typed form ([`Aux`]). Both headers are covered: regular
//! (`IMAGE_FILE_HEADER`) and big-obj (`ANON_OBJECT_HEADER_BIGOBJ`), and an
//! object can be converted from one to the other. Import objects and other
//! anonymous objects (`/GL`) are recognized ([`kind`]) and passed through
//! as they are ([`File`]).
//!
//! An object read and written back unchanged is the same file, byte for
//! byte, whichever tool wrote it: the reader records the layout (where
//! each part was, any padding, the string table as it was) and the writer
//! follows it while every part keeps its size. Otherwise the writer lays
//! the object out afresh, as MSVC or LLVM would ([`Style`]).
//!
//! Edits ([`Object::retain`], [`Object::insert_symbol`], ...) renumber
//! every reference the model holds; see [`edit`] for what they don't
//! see.

#![no_std]
// COFF counts, sizes, and offsets are 32-bit, and the writer checks that
// an object's fit before it narrows them.
#![allow(clippy::cast_possible_truncation)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

mod auxiliary;
mod checksum;
pub mod edit;
mod kind;
mod layout;
mod object;
mod read;
mod strings;
#[cfg(test)]
mod testutil;
mod write;

pub use checksum::checksum;
pub use edit::{EditError, Renumbering};
pub use kind::{BIGOBJ_CLASS_ID, File, Kind, kind};
pub use object::{
    Aux, BeginEnd, BigObjHeader, ClrToken, Format, FunctionDefinition, LineNumber, LineNumberRef,
    Name, Object, Relocation, Section, SectionData, SectionDefinition, SectionId, SectionNumber,
    Style, Symbol, SymbolId, WeakExternal, comdat_select, storage_class, symbol_type, weak_search,
};
