//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

use turbonop_global_alloc as _;

mod common;

mod about;
mod batch;
mod checksec;
mod data;
mod debugmap;
mod deps;
mod diff;
mod dwarf;
mod info;
mod init;
mod layers;
mod licenses;
mod lines;
mod linkmap;
mod ltcg;
mod mcp;
mod mingw;
mod pdb;
mod query;
mod readobj;
mod relocations;
mod resources;
mod self_contained;
mod serve;
mod signature;
mod size;
mod snapshot;
mod symbolize;
mod targets;
mod types;
mod vtables;
mod xref;
