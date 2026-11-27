//! A stand-in for Visual C++'s compiler back end, `c2.dll`, that logs the
//! conversation between `link.exe` and the real one: every call either
//! makes on the other's objects, with its arguments and results.
//!
//! Link-time code generation (`/GL` objects, `/LTCG`) is `link.exe` asking
//! `c2.dll` for an object handler (`DllGetObjHandler`), then a COM-style
//! conversation of interfaces both ways: the linker hands the back end its
//! objects and callbacks (`ILinkNotify2`, `IObjFile2`, `ICOFFObj`, ...),
//! the back end hands back its own (`IObjHandler10`, `IObjFile2`,
//! `IPublicSym5`, `ILinkDataRO`, ...). Their methods' names and order come
//! from the two binaries' public symbols (`hexcavator vtables -json`, read
//! at run time from beside this DLL), so every call is named.
//!
//! Use: copy a toolset's `bin\Hostx64\x64` somewhere, rename its `c2.dll`
//! `c2real.dll`, put this one (`c2.dll`) and `c2.dll.vtables.json` and
//! `link.exe.vtables.json` beside it, and link with that `link.exe`. The
//! log goes to `C2PROBE_LOG` (else `c2probe-<pid>.log` beside the DLL).
//!
//! The probe is Windows x64 only (where every method of these interfaces
//! takes integers and pointers only: see `probe`).

pub mod sig;

#[cfg(all(windows, target_arch = "x86_64"))]
mod probe;

use turbonop_global_alloc as _;
