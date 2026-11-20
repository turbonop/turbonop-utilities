//! Demangling: Microsoft C++ names ([`msvc`]), the Itanium C++ ABI's
//! ([`itanium`], whose demangler also takes Rust and Swift names), Rust's
//! v0 and legacy manglings ([`rust`]), and Swift's ([`swift`]); and
//! [`Demangler`], which takes a name mangled any of those ways, from a
//! binary of any format.

#![no_std]

extern crate alloc;

pub mod itanium;
pub mod msvc;
pub mod rust;
pub mod swift;

use alloc::string::String;

/// A demangler for every scheme: Microsoft's (`?f@@YAXXZ`), the Itanium
/// C++ ABI's (`_Z1fv`, with Mach-O's extra `_` too: `__Z1fv`), Rust's (v0
/// and legacy), and Swift's, whichever mangled a name.
#[derive(Default)]
pub struct Demangler {
    msvc: msvc::Demangler,
    itanium: itanium::Demangler,
}

impl Demangler {
    /// A demangler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `name` demangled to `out` (a Microsoft name as
    /// `msvc_flags`, [`msvc::flags`], say); `false` (appending nothing) if
    /// no scheme mangled it.
    pub fn demangle_into(&mut self, name: &str, msvc_flags: u32, out: &mut String) -> bool {
        if name.starts_with('?') {
            return self.msvc.demangle_into(name, msvc_flags, out);
        }
        if self.itanium.demangle_into(name, out) {
            return true;
        }
        // (Mach-O's names, and x86 Windows' C names, have a `_` before the
        // name as mangled.)
        match name.strip_prefix('_') {
            Some(rest) if !rest.is_empty() && rest.starts_with(['_', '$']) => {
                self.itanium.demangle_into(rest, out)
            }
            _ => false,
        }
    }

    /// `name` demangled (see [`Demangler::demangle_into`]), if a scheme
    /// mangled it.
    pub fn demangle(&mut self, name: &str, msvc_flags: u32) -> Option<String> {
        let mut out = String::new();
        self.demangle_into(name, msvc_flags, &mut out)
            .then_some(out)
    }

    /// `name` as its source names it: demangled, or else (a C name) as is,
    /// a Mach-O symbol's without the `_` its format puts before it
    /// (`macho`).
    pub fn source_name(&mut self, name: &str, msvc_flags: u32, macho: bool) -> String {
        if let Some(d) = self.demangle(name, msvc_flags) {
            return d;
        }
        match name.strip_prefix('_') {
            Some(c) if macho => String::from(c),
            _ => String::from(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scheme() {
        let mut d = Demangler::new();
        let f = msvc::flags::NO_PTR64;
        assert_eq!(
            d.demangle("?f@@YAXXZ", f).as_deref(),
            Some("void __cdecl f(void)")
        );
        assert_eq!(d.demangle("_Z1fv", f).as_deref(), Some("f()"));
        // (Mach-O's.)
        assert_eq!(d.demangle("__Z1fv", f).as_deref(), Some("f()"));
        assert!(
            d.demangle("_RNvCs1234_7mycrate3foo", f)
                .is_some_and(|n| n.ends_with("mycrate::foo"))
        );
        assert!(d.demangle("main", f).is_none());
        assert_eq!(d.source_name("_main", f, true), "main");
        assert_eq!(d.source_name("_main", f, false), "_main");
    }
}
