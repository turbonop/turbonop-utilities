//! What each input is, and whether its machine fits the library's.
//!
//! **`lib.exe`** (measured, 14.52):
//!
//! - It takes COFF objects (big-obj too), import objects, `/GL` objects,
//!   resource files (`.res`, stored whole), and libraries. Anything else,
//!   LLVM bitcode included, is fatal: LNK1107 "invalid or corrupt file:
//!   cannot read at 0x*size*", or LNK1136 for an empty file.
//! - The library's machine is `/MACHINE`'s, or else the first input
//!   object's (in command-line order), or else the first library member's.
//!   With none of these (only `.res` files), it warns (LNK4068) and uses
//!   x64.
//! - Each object named on the command line must have that machine, or
//!   none: LNK1112 otherwise. Library members are not checked. ARM64EC and
//!   ARM64X libraries also take x64, ARM64, and ARM64EC objects; an ARM64EC
//!   object in any other library is LNK1392.
//!
//! **`llvm-lib`** (`LibDriver.cpp`) checks every COFF object and bitcode
//! file, library members included, against `/machine` or the first one's
//! machine, takes the same ARM64EC and ARM64X mixtures, and won't infer
//! ARM64EC.

use super::args::Flavor;
use super::diag::{Diag, Kind};

/// What an input is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Library,
    /// A COFF object (regular or big-obj), with its machine.
    Object(u16),
    /// A short import object, with its machine.
    Import(u16),
    /// A `/GL` object, with its machine.
    Ltcg(u16),
    /// An LLVM bitcode file (`llvm-lib` only).
    Bitcode,
    /// A resource file (`.res`), stored whole.
    Resource,
}

impl InputKind {
    /// The machine this input requires the library to have, if any.
    pub fn machine(self) -> Option<u16> {
        match self {
            InputKind::Object(m) | InputKind::Import(m) | InputKind::Ltcg(m) if m != 0 => Some(m),
            _ => None,
        }
    }
}

/// The 32-byte header that starts a `.res` file: an empty resource entry.
const RES_HEADER: [u8; 32] = [
    0, 0, 0, 0, 0x20, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];

/// LLVM bitcode, bare or in its wrapper.
fn is_bitcode(data: &[u8]) -> bool {
    data.starts_with(b"BC\xC0\xDE") || data.starts_with(&[0xDE, 0xC0, 0x17, 0x0B])
}

/// Classifies `data`, an input named `name`; an input the tool won't take
/// is the error.
pub fn classify(flavor: Flavor, name: &str, data: &[u8]) -> Result<InputKind, Diag> {
    if turbonop_archive::Archive::is_archive(data) {
        return Ok(InputKind::Library);
    }
    if turbonop_pe::import_object::ImportObject::is_import_object(data)
        && let Ok(i) = turbonop_pe::import_object::ImportObject::parse(data)
    {
        return Ok(InputKind::Import(i.machine));
    }
    if turbonop_pe::ltcg::LtcgObject::is_ltcg(data)
        && let Ok(l) = turbonop_pe::ltcg::LtcgObject::parse(data)
    {
        return Ok(InputKind::Ltcg(l.machine));
    }
    if data.starts_with(&RES_HEADER) {
        return Ok(InputKind::Resource);
    }
    if let Ok(f) = turbonop_pe::PeFile::parse_object(data) {
        return Ok(InputKind::Object(f.machine()));
    }
    if flavor == Flavor::LlvmLib && is_bitcode(data) {
        return Ok(InputKind::Bitcode);
    }
    Err(match flavor {
        Flavor::Lib if data.is_empty() => Diag::fatal(1136, "invalid or corrupt file").on(name),
        Flavor::Lib => Diag::fatal(
            1107,
            format!("invalid or corrupt file: cannot read at 0x{:X}", data.len()),
        )
        .on(name),
        Flavor::LlvmLib => Diag::llvm(
            Kind::Fatal,
            "not a COFF object, bitcode, archive, import library or resource file",
        )
        .on(name),
    })
}

pub const X86: u16 = 0x014C;
pub const X64: u16 = 0x8664;
pub const ARM64: u16 = 0xAA64;
pub const ARM64EC: u16 = 0xA641;
pub const ARM64X: u16 = 0xA64E;

/// A machine's name in `lib.exe`'s messages.
pub fn lib_name(m: u16) -> String {
    match m {
        X86 => "x86".into(),
        X64 => "x64".into(),
        ARM64 => "ARM64".into(),
        ARM64EC => "ARM64EC".into(),
        ARM64X => "ARM64X".into(),
        0x0EBC => "EBC".into(),
        0x01C4 => "ARM".into(),
        0x0200 => "IA64".into(),
        _ => format!("0x{m:X}"),
    }
}

/// A machine's name in `llvm-lib`'s messages (LLVM's `machineToStr`).
pub fn llvm_name(m: u16) -> String {
    match m {
        X86 => "x86".into(),
        X64 => "x64".into(),
        ARM64 => "arm64".into(),
        ARM64EC => "arm64ec".into(),
        ARM64X => "arm64x".into(),
        0x01C4 => "arm".into(),
        0x0166 => "mips".into(),
        _ => format!("{m}"),
    }
}

/// Whether a library for `library` takes an input for `input`.
pub fn fits(library: u16, input: u16) -> bool {
    library == input
        || match library {
            ARM64 => input == ARM64X,
            ARM64EC | ARM64X => matches!(input, X64 | ARM64 | ARM64EC | ARM64X),
            _ => false,
        }
}

/// The library's machine, as the inputs are checked against it: for
/// `lib.exe`, each object on the command line in order ([`lib_object`]),
/// then each library member ([`lib_member`]); for `llvm-lib`, every input
/// and member in order ([`llvm`]).
///
/// [`lib_object`]: MachineCheck::lib_object
/// [`lib_member`]: MachineCheck::lib_member
/// [`llvm`]: MachineCheck::llvm
pub struct MachineCheck {
    /// The machine, once known.
    pub machine: Option<u16>,
    /// Where it came from, for `llvm-lib`'s messages.
    source: String,
}

impl MachineCheck {
    pub fn new(machine: Option<u16>, flavor: Flavor) -> MachineCheck {
        let source = match (machine, flavor) {
            (Some(m), Flavor::LlvmLib) => format!(" (from '/machine:{}' flag)", llvm_name(m)),
            _ => String::new(),
        };
        MachineCheck { machine, source }
    }

    /// Checks an object named on the command line (`lib.exe`).
    pub fn lib_object(&mut self, name: &str, kind: InputKind) -> Result<(), Diag> {
        let Some(m) = kind.machine() else {
            return Ok(());
        };
        // Measured: an ARM64EC object is refused even as the first, without
        // /MACHINE: lib.exe never infers ARM64EC.
        if m == ARM64EC && !matches!(self.machine, Some(ARM64EC | ARM64X)) {
            return Err(Diag::fatal(
                1392,
                "Incompatible object type found. Did you mean /machine:arm64x or \
                 /machine:arm64ec?",
            )
            .on(name));
        }
        let target = *self.machine.get_or_insert(m);
        if !fits(target, m) {
            return Err(Diag::fatal(
                1112,
                format!(
                    "module machine type '{}' conflicts with target machine type '{}'",
                    lib_name(m),
                    lib_name(target)
                ),
            )
            .on(name));
        }
        Ok(())
    }

    /// Checks any input or member (`llvm-lib`), named `name`.
    pub fn llvm(&mut self, name: &str, kind: InputKind) -> Result<(), Diag> {
        // llvm-lib reads bitcode's triple; turbolib passes bitcode through
        // without reading one, so it isn't checked.
        let InputKind::Object(m) = kind else {
            return Ok(());
        };
        if m == 0 {
            return Ok(());
        }
        match self.machine {
            None => {
                if m == ARM64EC {
                    return Err(Diag::llvm(
                        Kind::Fatal,
                        "file machine type arm64ec conflicts with inferred library machine \
                         type, use /machine:arm64ec or /machine:arm64x",
                    )
                    .on(name));
                }
                self.machine = Some(m);
                self.source = format!(" (inferred from earlier file '{name}')");
                Ok(())
            }
            Some(target) if fits(target, m) => Ok(()),
            Some(target) => Err(Diag::llvm(
                Kind::Fatal,
                format!(
                    "file machine type {} conflicts with library machine type {}{}",
                    llvm_name(m),
                    llvm_name(target),
                    self.source
                ),
            )
            .on(name)),
        }
    }
}

#[cfg(test)]
mod tests;
