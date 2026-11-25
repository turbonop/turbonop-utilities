//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

use turbonop_global_alloc as _;

mod build_version;
mod debugmap;
mod dylibs;
mod fixups;
mod objc;
mod readobj;
mod unwind_info;
