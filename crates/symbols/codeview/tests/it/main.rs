//! The integration tests, as one binary.

use turbonop_global_alloc as _;

// The corpus example's checks, run on the committed fixtures.
#[allow(dead_code)]
#[path = "../../examples/corpus/check.rs"]
mod check;

mod objects;
