//! `lib.exe`'s and `llvm-lib`'s options, parsed into one form.
//!
//! Both take options with `/` or `-`, in any case, with a value after `:`
//! (`/OUT:x.lib`; `=` is not a separator). Everything else is an input. The
//! rules differ in detail, and each is followed exactly as measured with
//! `lib.exe` 14.52 or read in LLVM's `LibDriver.cpp` and `Options.td`:
//!
//! - `lib.exe` warns about an option it doesn't know (LNK4044) and ignores
//!   it. An option that needs a value and has none is fatal (LNK1146), a
//!   value it can't parse is fatal (LNK1117, or LNK1147 for a number), and
//!   a `/MACHINE` it doesn't know is a warning (LNK4012). When an option is
//!   given twice, the last one wins.
//! - `llvm-lib` treats an unknown `/word` as an input file and ignores an
//!   unknown `-word` with a message. Options it accepts only for `lib.exe`'s
//!   sake (`/LTCG`, `/NODEFAULTLIB`, `/SUBSYSTEM`, `/VERBOSE`, `/NOLOGO`) do
//!   nothing.
//!
//! turbolib's own options are `/TURBOLIB:<option>` in both.

use super::args::Flavor;
use super::diag::{Diag, Kind, WarningPolicy};

/// A target machine, as `/MACHINE` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    X86,
    X64,
    /// 32-bit ARM (Thumb-2): `llvm-lib` only. `lib.exe` 14.52 refuses it
    /// (LNK1423).
    Arm,
    Arm64,
    Arm64Ec,
    Arm64X,
    /// EFI byte code: `lib.exe` only.
    Ebc,
    /// MIPS R4000: `llvm-lib` only.
    Mips,
}

impl Machine {
    /// The COFF machine number.
    pub fn number(self) -> u16 {
        match self {
            Machine::X86 => 0x014C,
            Machine::X64 => 0x8664,
            Machine::Arm => 0x01C4,
            Machine::Arm64 => 0xAA64,
            Machine::Arm64Ec => 0xA641,
            Machine::Arm64X => 0xA64E,
            Machine::Ebc => 0x0EBC,
            Machine::Mips => 0x0166,
        }
    }
}

/// What the command line asks for, in either tool's spelling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub nologo: bool,
    pub out: Option<String>,
    /// `/DEF` (with its file, if named): make an import library.
    pub def: Option<Option<String>>,
    /// `/defArm64Native:` (`llvm-lib`): the native ARM64 exports of an
    /// ARM64EC import library.
    pub def_arm64_native: Option<String>,
    pub exports: Vec<String>,
    pub extract: Vec<String>,
    pub remove: Vec<String>,
    pub include: Vec<String>,
    pub libpath: Vec<String>,
    /// `/LIST` (with its output file, if named).
    pub list: Option<Option<String>>,
    pub ltcg: bool,
    pub machine: Option<Machine>,
    pub name: Option<String>,
    /// `/NODEFAULTLIB` (`None`: all) and `/NODEFAULTLIB:x`.
    pub nodefaultlib: Vec<Option<String>>,
    /// `/SUBSYSTEM`'s value, as given.
    pub subsystem: Option<String>,
    pub verbose: bool,
    pub warnings: WarningPolicy,
    /// `/Brepro`: deterministic member headers.
    pub brepro: bool,
    /// `/ERRORREPORT`'s value. Accepted; turbolib reports nothing anywhere.
    pub errorreport: Option<String>,
    /// `/LINKREPRO:dir`, `/LINKREPROTARGET:file`, `/LINKREPROFULLPATHRSP:file`.
    pub linkrepro: Option<String>,
    pub linkrepro_target: Option<String>,
    pub linkrepro_fullpath_rsp: Option<String>,
    /// `llvm-lib`'s `/llvmlibthin`: a thin archive.
    pub thin: bool,
    /// `llvm-lib`'s `/llvmlibempty`: write a library even with no inputs.
    pub empty: bool,
    /// `llvm-lib`'s `/llvmlibindex[:no]`: write the index (the default).
    pub index: bool,
    /// `/?` or `/help`.
    pub help: bool,
    /// `/TURBOLIB:<option>`, in order.
    pub turbolib: Vec<String>,
    /// Inputs, in order.
    pub inputs: Vec<String>,
}

/// The options in `words`. Diagnostics that don't stop parsing are
/// returned with the options; a fatal one is the error.
pub fn parse(flavor: Flavor, words: &[String]) -> Result<(Options, Vec<Diag>), Diag> {
    let mut o = Options {
        index: true,
        ..Default::default()
    };
    let mut diags = Vec::new();
    for word in words {
        let Some(body) = word.strip_prefix(['/', '-']) else {
            o.inputs.push(word.clone());
            continue;
        };
        let (name, value) = match body.find(':') {
            Some(i) => (&body[..i], Some(&body[i + 1..])),
            None => (body, None),
        };
        let handled = match flavor {
            Flavor::Lib => lib_option(&mut o, &mut diags, word, name, value)?,
            Flavor::LlvmLib => llvm_option(&mut o, name, value)?,
        };
        if !handled {
            match flavor {
                Flavor::Lib => diags.push(Diag::warning(
                    4044,
                    format!("unrecognized option '{word}'; ignored"),
                )),
                Flavor::LlvmLib if word.starts_with('/') => o.inputs.push(word.clone()),
                Flavor::LlvmLib => diags.push(Diag::llvm(
                    Kind::Warning,
                    format!("ignoring unknown argument: {word}"),
                )),
            }
        }
    }
    Ok((o, diags))
}

/// `lib.exe`: whether `name` is an option it knows (and if so, applies it).
fn lib_option(
    o: &mut Options,
    diags: &mut Vec<Diag>,
    word: &str,
    name: &str,
    value: Option<&str>,
) -> Result<bool, Diag> {
    // The value an option must have: missing or empty is LNK1146.
    let need = |value: Option<&str>| -> Result<String, Diag> {
        match value {
            Some(v) if !v.is_empty() => Ok(v.to_string()),
            _ => Err(Diag::fatal(
                1146,
                format!("no argument specified with option '{word}'"),
            )),
        }
    };
    let syntax = || Diag::fatal(1117, format!("syntax error in option '{}'", &word[1..]));
    // A flag given a value: LNK4056, and the value is ignored.
    let flag = |diags: &mut Vec<Diag>, value: Option<&str>| {
        if value.is_some() {
            diags.push(Diag::warning(
                4056,
                format!("extra arguments ignored for option '{word}'"),
            ));
        }
    };
    let optional = |value: Option<&str>| value.filter(|v| !v.is_empty()).map(str::to_string);
    match name.to_ascii_uppercase().as_str() {
        "OUT" => o.out = Some(need(value)?),
        "NAME" => o.name = Some(need(value)?),
        "EXTRACT" => o.extract.push(need(value)?),
        "REMOVE" => o.remove.push(need(value)?),
        "EXPORT" => o.exports.push(need(value)?),
        "INCLUDE" => o.include.push(need(value)?),
        // Measured: /LIBPATH with no value is accepted and does nothing.
        "LIBPATH" => o.libpath.extend(optional(value)),
        "DEF" => o.def = Some(optional(value)),
        "DEFARM64NATIVE" => o.def_arm64_native = Some(need(value)?),
        "LIST" => o.list = Some(optional(value)),
        "NODEFAULTLIB" => o.nodefaultlib.push(optional(value)),
        // Measured: /LTCG:anything is accepted.
        "LTCG" => o.ltcg = true,
        "NOLOGO" => {
            flag(diags, value);
            o.nologo = true;
        }
        "BREPRO" => {
            flag(diags, value);
            o.brepro = true;
        }
        "VERBOSE" => {
            if value.is_some() {
                return Err(syntax());
            }
            o.verbose = true;
        }
        "MACHINE" => {
            let v = need(value)?;
            match v.to_ascii_uppercase().as_str() {
                "X86" => o.machine = Some(Machine::X86),
                "X64" | "AMD64" => o.machine = Some(Machine::X64),
                "ARM64" => o.machine = Some(Machine::Arm64),
                "ARM64EC" => o.machine = Some(Machine::Arm64Ec),
                "ARM64X" => o.machine = Some(Machine::Arm64X),
                "EBC" => o.machine = Some(Machine::Ebc),
                "ARM" | "ARMNT" | "THUMB" => {
                    return Err(Diag::fatal(
                        1423,
                        "ARM32 is no longer a supported target platform. Use a VS 2022 \
                         toolchain to continue building for ARM32",
                    ));
                }
                _ => diags.push(Diag::warning(
                    4012,
                    format!(
                        "invalid value '{v}', must be 'ARM64, ARM64X, EBC, X64, or X86'; \
                         option ignored"
                    ),
                )),
            }
        }
        "SUBSYSTEM" => {
            let v = need(value)?;
            if !valid_subsystem(&v) {
                return Err(syntax());
            }
            o.subsystem = Some(v);
        }
        "ERRORREPORT" => {
            let v = need(value)?;
            if !matches!(
                v.to_ascii_uppercase().as_str(),
                "NONE" | "PROMPT" | "QUEUE" | "SEND"
            ) {
                return Err(syntax());
            }
            o.errorreport = Some(v);
        }
        "WX" => match value {
            None => o.warnings.all_errors = true,
            Some(v) if v.eq_ignore_ascii_case("NO") => {
                o.warnings.all_errors = false;
                o.warnings.errors.clear();
            }
            Some(v) => o.warnings.errors.extend(numbers(word, v)?),
        },
        "IGNORE" => {
            let v = need(value)?;
            o.warnings.ignored.extend(numbers(word, &v)?);
        }
        "LINKREPRO" => o.linkrepro = Some(need(value)?),
        "LINKREPROTARGET" => o.linkrepro_target = Some(need(value)?),
        "LINKREPROFULLPATHRSP" => o.linkrepro_fullpath_rsp = Some(need(value)?),
        "?" | "HELP" => o.help = true,
        "TURBOLIB" => o.turbolib.push(need(value)?),
        _ => return Ok(false),
    }
    Ok(true)
}

/// `/WX:` and `/IGNORE:` lists: decimal numbers separated by commas.
fn numbers(word: &str, v: &str) -> Result<Vec<u32>, Diag> {
    v.split(',')
        .map(|n| n.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| {
            Diag::fatal(
                1147,
                format!("invalid number specified with option '{word}'"),
            )
        })
}

/// `/SUBSYSTEM:name[,major[.minor]]` with a name `lib.exe` lists.
fn valid_subsystem(v: &str) -> bool {
    let (name, version) = match v.split_once(',') {
        Some((n, ver)) => (n, Some(ver)),
        None => (v, None),
    };
    let known = matches!(
        name.to_ascii_uppercase().as_str(),
        "BOOT_APPLICATION"
            | "CONSOLE"
            | "EFI_APPLICATION"
            | "EFI_BOOT_SERVICE_DRIVER"
            | "EFI_ROM"
            | "EFI_RUNTIME_DRIVER"
            | "NATIVE"
            | "POSIX"
            | "WINDOWS"
            | "WINDOWSCE"
    );
    let version_ok = version.is_none_or(|ver| {
        let (major, minor) = match ver.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (ver, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        digits(major) && minor.is_none_or(digits)
    });
    known && version_ok
}

/// `llvm-lib`: whether `name` is an option it knows (and if so, applies it).
/// Its options with values are spelled `name:value`; its flags must match
/// whole.
fn llvm_option(o: &mut Options, name: &str, value: Option<&str>) -> Result<bool, Diag> {
    let lower = name.to_ascii_lowercase();
    match (lower.as_str(), value) {
        ("out", Some(v)) => o.out = Some(v.to_string()),
        ("def", Some(v)) => o.def = Some(Some(v.to_string())),
        ("defarm64native", Some(v)) => o.def_arm64_native = Some(v.to_string()),
        ("libpath", Some(v)) => o.libpath.push(v.to_string()),
        ("ignore", Some(v)) => {
            // llvm-lib's codes are words (emptyoutput) or numbers.
            if let Ok(n) = v.parse::<u32>() {
                o.warnings.ignored.push(n);
            } else if v.eq_ignore_ascii_case("emptyoutput") {
                o.warnings.ignored.push(EMPTY_OUTPUT);
            }
        }
        ("machine", Some(v)) => {
            o.machine = Some(match v.to_ascii_lowercase().as_str() {
                "x64" | "amd64" => Machine::X64,
                "x86" | "i386" => Machine::X86,
                "arm" => Machine::Arm,
                "arm64" => Machine::Arm64,
                "arm64ec" => Machine::Arm64Ec,
                "arm64x" => Machine::Arm64X,
                "mips" => Machine::Mips,
                _ => {
                    return Err(Diag::llvm(
                        Kind::Fatal,
                        format!("unknown /machine: arg {v}"),
                    ));
                }
            });
        }
        ("nodefaultlib", v) => o.nodefaultlib.push(v.map(str::to_string)),
        ("subsystem", Some(v)) => o.subsystem = Some(v.to_string()),
        ("wx", None) => o.warnings.all_errors = true,
        ("wx", Some(v)) if v.eq_ignore_ascii_case("no") => o.warnings.all_errors = false,
        ("list", None) => o.list = Some(None),
        ("llvmlibthin", None) => o.thin = true,
        ("llvmlibempty", None) => o.empty = true,
        ("llvmlibindex", None) => o.index = true,
        ("llvmlibindex", Some(v)) if v.eq_ignore_ascii_case("no") => o.index = false,
        ("ltcg", None) => o.ltcg = true,
        ("nologo", None) => o.nologo = true,
        ("verbose", None) => o.verbose = true,
        ("help" | "?" | "??", None) => o.help = true,
        ("turbolib", Some(v)) => o.turbolib.push(v.to_string()),
        _ => return Ok(false),
    }
    Ok(true)
}

/// The code `/ignore:emptyoutput` silences in `llvm-lib` (it has no number).
pub const EMPTY_OUTPUT: u32 = u32::MAX;

#[cfg(test)]
mod tests;
