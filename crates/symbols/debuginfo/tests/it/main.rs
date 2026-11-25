//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

use turbonop_global_alloc as _;

mod dwarf;
mod hostile_dwarf;
mod hostile_pdb;
mod locate;
mod options;
mod pdb;
mod split_dwarf;
mod values;
mod values_golden;
mod xz;
