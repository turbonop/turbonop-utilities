//! The integration tests, as one binary (one link, not one per file).

use turbonop_global_alloc as _;

mod assembly;
mod cli;
mod common;
mod functions;
mod profile;
mod protocol;
mod saved_symbols;
mod serve;
