//! The integration tests, as one binary (one link, not one per file).

use turbonop_global_alloc as _;

mod support;

mod context;
mod core_file;
mod linux_maps;
mod macho_core;
mod minidump;
mod registers;
mod space;
