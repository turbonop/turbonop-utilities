//! Java class files and JARs.
//!
//! - [`ClassFile`]: a class file's version (Java 1.1's to Java 27's, and
//!   preview features), the whole [`constant_pool`] (every tag, dynamic
//!   constants, modules and packages, method handles and types), access
//!   flags ([`access`]), this and super classes, interfaces, fields,
//!   methods, and every standard [`attributes`] (Code with its exception
//!   table and nested attributes, StackMapTable frames ([`stackmap`]),
//!   [`annotations`] of every kind with type annotations and defaults,
//!   Module, Record, BootstrapMethods, ...), with where each part lies.
//! - [`types`]: descriptors and generic signatures, parsed and written as
//!   Java writes them.
//! - [`bytecode`]: every JVM instruction decoded, `wide` and the switches'
//!   padding included; [`javap`] writes a class as `javap -v -p` does,
//!   constant pool references resolved to names.
//! - [`jar`]: JARs (on `turbonop-zip`): the [`manifest`] (main and
//!   per-entry sections, continuation lines), the multi-release layout,
//!   signature files, and each class's size; classes read on every core.
//!
//! Everything is read from the borrowed bytes and every read is checked:
//! damaged input gives errors, never a panic.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod access;
pub mod annotations;
pub mod attributes;
pub mod bytecode;
mod class;
pub mod constant_pool;
pub mod jar;
pub mod javap;
pub mod manifest;
mod reader;
pub mod stackmap;
pub mod types;

pub use attributes::{Attribute, AttributeValue, AttributesExt, Code};
pub use class::{
    ClassFile, ClassLayout, LATEST_MAJOR_VERSION, MAGIC, Member, PREVIEW_MINOR_VERSION, is_class,
    java_version_name,
};
pub use constant_pool::{Constant, ConstantPool};
pub use jar::Jar;
pub use manifest::Manifest;

#[cfg(test)]
mod tests {
    #[test]
    fn types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<crate::ClassFile<'_>>();
        check::<crate::Jar<'_>>();
    }
}
