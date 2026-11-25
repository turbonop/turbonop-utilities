//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

use turbonop_global_alloc as _;

mod decode;
mod manifest;
