//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

use turbonop_global_alloc as _;

mod access;
mod access_cases;
mod api;
mod apx;
mod forms;
mod golden;
