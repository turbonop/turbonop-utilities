//! What the command line asks for ([`Config`]), from its words
//! ([`crate::args`]), with `link.exe`'s defaults, and the diagnostics
//! `link.exe` would give for them.
//!
//! Every option `link.exe` knows is known here (its option table, from
//! the 14.52 tools, is [`KNOWN`]): an option turbolink doesn't implement
//! yet is reported as such ([`Diagnostic::NotYet`]), never mistaken for a
//! misspelling, which `link.exe` ignores with LNK4044.

use std::path::PathBuf;

use crate::args::Arg;

/// The target machine (`/MACHINE`, or the first object's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    /// `X64` (`AMD64`).
    X64,
    /// `X86`.
    X86,
    /// `ARM64`.
    Arm64,
    /// `ARM64EC`.
    Arm64Ec,
    /// `ARM64X`.
    Arm64X,
    /// `ARM` (Thumb-2).
    Arm,
}

impl Machine {
    fn parse(v: &str) -> Option<Machine> {
        Some(match v.to_ascii_lowercase().as_str() {
            "x64" | "amd64" => Machine::X64,
            "x86" | "i386" => Machine::X86,
            "arm64" => Machine::Arm64,
            "arm64ec" => Machine::Arm64Ec,
            "arm64x" => Machine::Arm64X,
            "arm" | "armnt" => Machine::Arm,
            _ => return None,
        })
    }

    /// The COFF machine number.
    pub fn number(self) -> u16 {
        match self {
            Machine::X64 => 0x8664,
            Machine::X86 => 0x14C,
            Machine::Arm64 | Machine::Arm64X => 0xAA64,
            Machine::Arm64Ec => 0xA641,
            Machine::Arm => 0x1C4,
        }
    }

    /// Whether images for it are PE32+ (64-bit).
    pub fn is_64(self) -> bool {
        !matches!(self, Machine::X86 | Machine::Arm)
    }
}

/// `/SUBSYSTEM`'s subsystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    /// `CONSOLE`.
    Console,
    /// `WINDOWS`.
    Windows,
    /// `NATIVE`.
    Native,
    /// `POSIX`.
    Posix,
    /// `BOOT_APPLICATION`.
    BootApplication,
    /// `EFI_APPLICATION`.
    EfiApplication,
    /// `EFI_BOOT_SERVICE_DRIVER`.
    EfiBootServiceDriver,
    /// `EFI_ROM`.
    EfiRom,
    /// `EFI_RUNTIME_DRIVER`.
    EfiRuntimeDriver,
    /// `WINDOWSCE`.
    WindowsCe,
}

impl Subsystem {
    fn parse(v: &str) -> Option<Subsystem> {
        Some(match v.to_ascii_lowercase().as_str() {
            "console" => Subsystem::Console,
            "windows" => Subsystem::Windows,
            "native" => Subsystem::Native,
            "posix" => Subsystem::Posix,
            "boot_application" => Subsystem::BootApplication,
            "efi_application" => Subsystem::EfiApplication,
            "efi_boot_service_driver" => Subsystem::EfiBootServiceDriver,
            "efi_rom" => Subsystem::EfiRom,
            "efi_runtime_driver" => Subsystem::EfiRuntimeDriver,
            "windowsce" => Subsystem::WindowsCe,
            _ => return None,
        })
    }

    /// The optional header's `Subsystem` number.
    pub fn number(self) -> u16 {
        match self {
            Subsystem::Native => 1,
            Subsystem::Windows => 2,
            Subsystem::Console => 3,
            Subsystem::Posix => 7,
            Subsystem::WindowsCe => 9,
            Subsystem::EfiApplication => 10,
            Subsystem::EfiBootServiceDriver => 11,
            Subsystem::EfiRuntimeDriver => 12,
            Subsystem::EfiRom => 13,
            Subsystem::BootApplication => 16,
        }
    }
}

/// `/DEBUG`'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Debug {
    /// No PDB (no `/DEBUG`, or `/DEBUG:NONE`).
    None,
    /// `/DEBUG` or `/DEBUG:FULL`: a PDB with everything.
    Full,
    /// `/DEBUG:FASTLINK`: a PDB that refers to the objects' debug
    /// information (turbolink writes a full one instead, which is as
    /// fast for it: see `docs/turbolink.md`).
    FastLink,
}

/// `/DEFAULTLIB`'s and `/NODEFAULTLIB`'s effect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NoDefaultLib {
    /// Default libraries are searched.
    #[default]
    None,
    /// `/NODEFAULTLIB` alone: none are.
    All,
    /// `/NODEFAULTLIB:name`: these aren't (lower case, as named).
    These(Vec<String>),
}

/// One `/EXPORT` (or a `.def` file's export).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// The name the DLL exports it by.
    pub name: String,
    /// The symbol it is (`/EXPORT:name=internal`), if not `name`.
    pub internal: Option<String>,
    /// `@ordinal`.
    pub ordinal: Option<u16>,
    /// `NONAME`: by ordinal only.
    pub noname: bool,
    /// `DATA`.
    pub data: bool,
    /// `PRIVATE`: not in the import library.
    pub private: bool,
    /// `EXPORTAS`, the name it is exported as (ARM64EC, and since 17.x
    /// any machine).
    pub export_as: Option<String>,
}

impl Export {
    /// Parses `/EXPORT`'s value:
    /// `name[=internal][,@ordinal[,NONAME]][,DATA][,PRIVATE][,EXPORTAS,name]`.
    pub fn parse(v: &str) -> Result<Export, String> {
        let mut parts = v.split(',');
        let head = parts.next().unwrap_or_default();
        let (name, internal) = match head.split_once('=') {
            // `=` inside a quoted C++ name never occurs; `name==x` names
            // `name` exported as `=x`, which link.exe also reads so.
            Some((n, i)) => (n, Some(i.to_owned())),
            None => (head, None),
        };
        if name.is_empty() {
            return Err(format!("/EXPORT:{v}: no name"));
        }
        let mut e = Export {
            name: name.to_owned(),
            internal,
            ordinal: None,
            noname: false,
            data: false,
            private: false,
            export_as: None,
        };
        while let Some(p) = parts.next() {
            if let Some(o) = p.strip_prefix('@') {
                e.ordinal = Some(
                    parse_number(o)
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|&n| n != 0)
                        .ok_or_else(|| format!("/EXPORT:{v}: bad ordinal '{o}'"))?,
                );
                continue;
            }
            match p.to_ascii_lowercase().as_str() {
                "noname" => e.noname = true,
                "data" => e.data = true,
                "private" => e.private = true,
                "constant" => e.data = true,
                "exportas" => {
                    e.export_as = Some(
                        parts
                            .next()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| format!("/EXPORT:{v}: EXPORTAS needs a name"))?
                            .to_owned(),
                    );
                }
                _ => return Err(format!("/EXPORT:{v}: unknown '{p}'")),
            }
        }
        Ok(e)
    }
}

/// `/SECTION:name,attributes[,ALIGN=n]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionAttributes {
    /// The section's name.
    pub name: String,
    /// Characteristics set (`IMAGE_SCN_*`).
    pub set: u32,
    /// Characteristics cleared (`!`).
    pub clear: u32,
    /// `ALIGN=n`.
    pub align: Option<u32>,
}

impl SectionAttributes {
    /// Parses `/SECTION`'s value.
    pub fn parse(v: &str) -> Result<SectionAttributes, String> {
        let mut parts = v.split(',');
        let name = parts.next().unwrap_or_default();
        if name.is_empty() {
            return Err(format!("/SECTION:{v}: no name"));
        }
        let mut s = SectionAttributes {
            name: name.to_owned(),
            set: 0,
            clear: 0,
            align: None,
        };
        for p in parts {
            if let Some(a) = p
                .strip_prefix("ALIGN=")
                .or_else(|| p.strip_prefix("align="))
            {
                s.align = Some(
                    parse_number(a)
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| n.is_power_of_two() && *n <= 8192)
                        .ok_or_else(|| format!("/SECTION:{v}: bad alignment '{a}'"))?,
                );
                continue;
            }
            let mut not = false;
            for c in p.chars() {
                let bit = match c.to_ascii_uppercase() {
                    '!' => {
                        not = true;
                        continue;
                    }
                    'D' => 0x0200_0000, // discardable
                    'E' => 0x2000_0000, // execute
                    'K' => 0x0400_0000, // not cached
                    'P' => 0x0800_0000, // not paged
                    'R' => 0x4000_0000, // read
                    'S' => 0x1000_0000, // shared
                    'W' => 0x8000_0000, // write
                    _ => return Err(format!("/SECTION:{v}: unknown attribute '{c}'")),
                };
                if not {
                    s.clear |= bit;
                } else {
                    s.set |= bit;
                }
                not = false;
            }
        }
        Ok(s)
    }
}

/// `/MANIFEST`'s choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manifest {
    /// No manifest (`/MANIFEST:NO`, the default).
    No,
    /// `/MANIFEST`: written beside the image.
    SideBySide,
    /// `/MANIFEST:EMBED[,ID=n]`: a resource of the image.
    Embed {
        /// The resource's ID (1 for an EXE, 2 for a DLL, by default).
        id: Option<u16>,
    },
}

/// What a command line asks for.
#[derive(Clone, Debug)]
pub struct Config {
    /// The inputs, in order (objects, libraries, `.res`, ...).
    pub inputs: Vec<String>,
    /// `/OUT`.
    pub out: Option<PathBuf>,
    /// `/MACHINE`.
    pub machine: Option<Machine>,
    /// `/DLL`.
    pub dll: bool,
    /// `/ENTRY`.
    pub entry: Option<String>,
    /// `/NOENTRY`.
    pub no_entry: bool,
    /// `/SUBSYSTEM` and its version.
    pub subsystem: Option<Subsystem>,
    /// `/SUBSYSTEM:x,major.minor`.
    pub subsystem_version: Option<(u16, u16)>,
    /// `/OSVERSION`.
    pub os_version: Option<(u16, u16)>,
    /// `/VERSION`.
    pub version: (u16, u16),
    /// `/BASE`.
    pub base: Option<u64>,
    /// `/FIXED` (no relocations).
    pub fixed: bool,
    /// `/DYNAMICBASE` (ASLR).
    pub dynamic_base: bool,
    /// `/NXCOMPAT`.
    pub nx_compat: bool,
    /// `/HIGHENTROPYVA`; `None`: the default (on for 64-bit images with
    /// `/DYNAMICBASE`).
    pub high_entropy_va: Option<bool>,
    /// `/LARGEADDRESSAWARE`; `None`: the default (on for 64-bit images).
    pub large_address_aware: Option<bool>,
    /// `/TSAWARE`; `None`: the default (on for EXEs).
    pub ts_aware: Option<bool>,
    /// `/APPCONTAINER`.
    pub app_container: bool,
    /// `/CETCOMPAT`.
    pub cet_compat: bool,
    /// `/GUARD:CF`.
    pub guard_cf: bool,
    /// `/GUARD:EHCONT`.
    pub guard_ehcont: bool,
    /// The long-jump table with `/GUARD:CF` (not with `/GUARD:NOLONGJMP`).
    pub guard_longjmp: bool,
    /// `/HIGHENTROPYVA` aside, `/ALLOWBIND`; `false`: `NO_BIND`.
    pub allow_bind: bool,
    /// `/ALLOWISOLATION`; `false`: `NO_ISOLATION`.
    pub allow_isolation: bool,
    /// `/INTEGRITYCHECK`.
    pub integrity_check: bool,
    /// `/STACK`: reserve, commit.
    pub stack: (u64, Option<u64>),
    /// `/HEAP`: reserve, commit.
    pub heap: (u64, Option<u64>),
    /// `/ALIGN`: section alignment.
    pub align: Option<u32>,
    /// `/FILEALIGN`.
    pub file_align: Option<u32>,
    /// `/LIBPATH`s, in order (before the `LIB` variable's).
    pub lib_paths: Vec<PathBuf>,
    /// `/DEFAULTLIB`s, in order.
    pub default_libs: Vec<String>,
    /// `/NODEFAULTLIB`.
    pub no_default_lib: NoDefaultLib,
    /// `/DISALLOWLIB`s, as written: libraries the link shouldn't use (a
    /// CRT's objects name the CRTs they can't be mixed with).
    pub disallow_libs: Vec<String>,
    /// `/INCLUDE`s: symbols to keep (and to pull from libraries).
    pub includes: Vec<String>,
    /// `/ALTERNATENAME:from=to`.
    pub alternate_names: Vec<(String, String)>,
    /// `/MERGE:from=to`.
    pub merges: Vec<(String, String)>,
    /// `/SECTION`s.
    pub sections: Vec<SectionAttributes>,
    /// `/EXPORT`s.
    pub exports: Vec<Export>,
    /// `/DEF`.
    pub def: Option<PathBuf>,
    /// The `.def` file's `LIBRARY` name: the DLL's name in its export
    /// directory (else the output's file name).
    pub library_name: Option<String>,
    /// What the `.def` file's `LIBRARY` or `NAME` says to link with
    /// (` /DLL /OUT:x.dll`): the export object's `.drectve`.
    pub def_directives: Option<String>,
    /// `/IMPLIB`.
    pub implib: Option<PathBuf>,
    /// `/NOIMPLIB`.
    pub no_implib: bool,
    /// `/NOEXP`.
    pub no_exp: bool,
    /// `/OPT:REF`; `None`: the default (on without `/DEBUG`).
    pub opt_ref: Option<bool>,
    /// `/OPT:ICF`; `None`: the default (as `/OPT:REF`).
    pub opt_icf: Option<bool>,
    /// `/OPT:ICF=n`'s iterations.
    pub icf_iterations: Option<u32>,
    /// `/OPT:LBR` (ARM).
    pub opt_lbr: Option<bool>,
    /// `/DEBUG`.
    pub debug: Debug,
    /// `/PDB`.
    pub pdb: Option<PathBuf>,
    /// `/PDBALTPATH`.
    pub pdb_alt_path: Option<String>,
    /// `/PDBSTRIPPED`.
    pub pdb_stripped: Option<PathBuf>,
    /// `/NATVIS` files.
    pub natvis: Vec<PathBuf>,
    /// `/SOURCELINK`.
    pub source_link: Option<PathBuf>,
    /// `/MAP` (and its file, if named).
    pub map: Option<Option<PathBuf>>,
    /// `/MAPINFO:EXPORTS`.
    pub map_exports: bool,
    /// `/ORDER:@file`.
    pub order: Option<PathBuf>,
    /// `/DELAYLOAD` DLLs.
    pub delay_load: Vec<String>,
    /// `/DELAY:UNLOAD`.
    pub delay_unload: bool,
    /// `/DELAY:NOBIND`.
    pub delay_nobind: bool,
    /// `/DEPENDENTLOADFLAG`.
    pub dependent_load_flag: Option<u16>,
    /// `/FUNCTIONPADMIN`: bytes before each function, if given.
    pub function_pad_min: Option<u32>,
    /// `/SAFESEH`; `None`: the default.
    pub safe_seh: Option<bool>,
    /// `/STUB`.
    pub stub: Option<PathBuf>,
    /// `/SWAPRUN:CD`, `/SWAPRUN:NET`.
    pub swap_run_cd: bool,
    /// `/SWAPRUN:NET`.
    pub swap_run_net: bool,
    /// `/RELEASE` (a checksum in the header).
    pub release: bool,
    /// `/Brepro`: no time stamps; reproducible output.
    pub repro: bool,
    /// `/WHOLEARCHIVE` (all) or `/WHOLEARCHIVE:lib`.
    pub whole_archive: Vec<Option<String>>,
    /// `/FORCE:MULTIPLE`.
    pub force_multiple: bool,
    /// `/FORCE:UNRESOLVED`.
    pub force_unresolved: bool,
    /// `/MANIFEST`.
    pub manifest: Manifest,
    /// Whether `/MANIFEST` was given (without it, a `/MANIFESTDEPENDENCY`
    /// asks for a manifest beside the image, as link.exe writes one).
    pub manifest_given: bool,
    /// `/MANIFESTUAC`; `None`: the default fragment; `Some(None)`:
    /// `/MANIFESTUAC:NO`.
    pub manifest_uac: Option<Option<String>>,
    /// `/MANIFESTDEPENDENCY`s.
    pub manifest_dependencies: Vec<String>,
    /// `/MANIFESTFILE`.
    pub manifest_file: Option<PathBuf>,
    /// `/MANIFESTINPUT`s.
    pub manifest_inputs: Vec<PathBuf>,
    /// `/FAILIFMISMATCH:key=value`s.
    pub fail_if_mismatch: Vec<(String, String)>,
    /// `/WX` (warnings are errors; `/WX:nnnn,...`: these).
    pub warnings_as_errors: Option<Vec<u32>>,
    /// `/IGNORE:nnnn`s.
    pub ignore_warnings: Vec<u32>,
    /// `/NOLOGO`.
    pub no_logo: bool,
    /// `/VERBOSE`.
    pub verbose: bool,
    /// `/TIME`.
    pub time: bool,
    /// `/LTCG` (any form): link-time code generation is asked for.
    pub ltcg: bool,
    /// `/INCREMENTAL`; turbolink links fully, always.
    pub incremental: Option<bool>,
    /// `/DRIVER` or `/KERNEL`.
    pub kernel: bool,
    /// `/THROWINGNEW`.
    pub throwing_new: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            inputs: Vec::new(),
            out: None,
            machine: None,
            dll: false,
            entry: None,
            no_entry: false,
            subsystem: None,
            subsystem_version: None,
            os_version: None,
            version: (0, 0),
            base: None,
            fixed: false,
            dynamic_base: true,
            nx_compat: true,
            high_entropy_va: None,
            large_address_aware: None,
            ts_aware: None,
            app_container: false,
            cet_compat: false,
            guard_cf: false,
            guard_ehcont: false,
            guard_longjmp: true,
            allow_bind: true,
            allow_isolation: true,
            integrity_check: false,
            stack: (0x10_0000, None),
            heap: (0x10_0000, None),
            align: None,
            file_align: None,
            lib_paths: Vec::new(),
            default_libs: Vec::new(),
            no_default_lib: NoDefaultLib::None,
            disallow_libs: Vec::new(),
            includes: Vec::new(),
            alternate_names: Vec::new(),
            merges: Vec::new(),
            sections: Vec::new(),
            exports: Vec::new(),
            def: None,
            library_name: None,
            def_directives: None,
            implib: None,
            no_implib: false,
            no_exp: false,
            opt_ref: None,
            opt_icf: None,
            icf_iterations: None,
            opt_lbr: None,
            debug: Debug::None,
            pdb: None,
            pdb_alt_path: None,
            pdb_stripped: None,
            natvis: Vec::new(),
            source_link: None,
            map: None,
            map_exports: false,
            order: None,
            delay_load: Vec::new(),
            delay_unload: false,
            delay_nobind: false,
            dependent_load_flag: None,
            function_pad_min: None,
            safe_seh: None,
            stub: None,
            swap_run_cd: false,
            swap_run_net: false,
            release: false,
            repro: false,
            whole_archive: Vec::new(),
            force_multiple: false,
            force_unresolved: false,
            manifest: Manifest::No,
            manifest_given: false,
            manifest_uac: None,
            manifest_dependencies: Vec::new(),
            manifest_file: None,
            manifest_inputs: Vec::new(),
            fail_if_mismatch: Vec::new(),
            warnings_as_errors: None,
            ignore_warnings: Vec::new(),
            no_logo: false,
            verbose: false,
            time: false,
            ltcg: false,
            incremental: None,
            kernel: false,
            throwing_new: false,
        }
    }
}

impl Config {
    /// Whether `/DELAYLOAD` names DLL `dll` (as an import object names it;
    /// case aside).
    pub fn is_delay_loaded(&self, dll: &[u8]) -> bool {
        self.delay_load
            .iter()
            .any(|d| d.as_bytes().eq_ignore_ascii_case(dll))
    }

    /// Whether unreferenced functions and data are removed: `/OPT:REF`,
    /// by default unless `/DEBUG`.
    pub fn gc_sections(&self) -> bool {
        self.opt_ref.unwrap_or(self.debug == Debug::None)
    }

    /// Whether identical COMDATs are folded: `/OPT:ICF`, by default when
    /// `/OPT:REF` is (whether link.exe's default follows an explicit
    /// `/OPT:REF` under `/DEBUG` is to be checked against it).
    pub fn icf(&self) -> bool {
        self.opt_icf.unwrap_or_else(|| self.gc_sections())
    }
}

/// Something the command line's reading says, to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// An option `link.exe` doesn't know (LNK4044: ignored).
    Unknown(String),
    /// An option `link.exe` knows that turbolink doesn't do yet.
    NotYet(String),
    /// An option turbolink accepts and has no use for (it means nothing
    /// for a native user-mode image, or only to `link.exe`'s internals).
    Ignored(String),
    /// A bad value (`link.exe` stops: LNK1117 and others).
    Bad(String),
}

/// `strtoul(s, 0)`: decimal, `0x` hexadecimal, or `0` octal.
pub fn parse_number(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else if s.len() > 1 && s.starts_with('0') {
        u64::from_str_radix(&s[1..], 8).ok()
    } else {
        s.parse().ok()
    }
}

/// `major[.minor]`.
fn parse_version(s: &str) -> Option<(u16, u16)> {
    let (a, b) = s.split_once('.').unwrap_or((s, "0"));
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// `reserve[,commit]`.
fn parse_sizes(s: &str) -> Option<(u64, Option<u64>)> {
    match s.split_once(',') {
        Some((r, c)) => Some((
            parse_number(r)?,
            if c.is_empty() {
                None
            } else {
                Some(parse_number(c)?)
            },
        )),
        None => Some((parse_number(s)?, None)),
    }
}

/// `a=b`.
fn pair(v: &str) -> Option<(String, String)> {
    let (a, b) = v.split_once('=')?;
    (!a.is_empty() && !b.is_empty()).then(|| (a.to_owned(), b.to_owned()))
}

/// Whether `value` is absent or `:NO` (for options such as
/// `/DYNAMICBASE[:NO]`): `Some(true)` for the option, `Some(false)` for
/// `:NO`, `None` for anything else.
fn yes_no(value: Option<&str>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(v) if v.eq_ignore_ascii_case("no") => Some(false),
        _ => None,
    }
}

/// The options of `link.exe` 14.52's option table (its own list of the
/// words it takes, the documented and the undocumented), lower case.
/// What isn't here and isn't one of the common options handled by name
/// is unknown to `link.exe` too.
pub const KNOWN: &[&str] = &[
    "align",
    "allowbind",
    "allowimagesizeover2gb",
    "allowisolation",
    "allpdata",
    "alternatename",
    "altsourcefiles",
    "appcontainer",
    "arm64hazardexist",
    "arm64hazardfree",
    "arm64xaggressiverwdatafold",
    "arm64xcrossdebug",
    "arm64xcrossresolve",
    "arm64xemulatorbuild",
    "arm64xfastforward",
    "arm64xfoldrwdata",
    "arm64xfunctionpadminx64",
    "arm64xhack",
    "arm64xnewchameleonlibfmt",
    "arm64xnomergeimports",
    "arm64xnoreorderobjs",
    "arm64xsameaddress",
    "arm64xuselegacyffs",
    "assemblydebug",
    "assemblylinkresource",
    "assemblymodule",
    "assemblymodulemap",
    "assemblyresource",
    "base",
    "baserelocclustering",
    "brepro",
    "cetcompat",
    "cetcompatstrict",
    "cetdynamicapisinproc",
    "cetipvalidationrelaxed",
    "cgthreads",
    "clrheader",
    "clrimagetype",
    "clrloaderoptimization",
    "clrnetcore",
    "clrrvaentry",
    "clrsignhash",
    "clrsupportlasterror",
    "clrsupportlasterrordll",
    "clrthreadattribute",
    "clrunmanagedcodecheck",
    "coffsecttopdbstrm",
    "crashondiag",
    "cvtres",
    "cxxmodulestrongownership",
    "d2",
    "db",
    "debug",
    "debugtype",
    "decisiondir",
    "def",
    "defarm64native",
    "defaultlib",
    "delay",
    "delayload",
    "delaysign",
    "dependentloadflag",
    "didatownsection",
    "disallowlib",
    "discardtrack",
    "dll",
    "dllrename",
    "driver",
    "dynamicbase",
    "dynamicdeopt",
    "dynamicdeopt-allowicf",
    "dynamicdeopt-deoptlink",
    "dynamicvaluefixupsym",
    "editandcontinue",
    "emitasx64image",
    "emitpogophaseinfo",
    "emittoolversioninfo",
    "emitvolatilemetadata",
    "enclave",
    "encpadsize",
    "entry",
    "errorreport",
    "etm",
    "expectedoutputsize",
    "experimental",
    "export",
    "exportarm64native",
    "exportpadmin",
    "failifmismatch",
    "fastfail",
    "fastgenprofile",
    "fe",
    "filealign",
    "fixed",
    "force",
    "forcewinmdversion12",
    "fullbuild",
    "funcoverride",
    "funcoverridedisablefunc",
    "funcoverrideemitcand",
    "funcoverrideoncallsites",
    "functionpadmin",
    "genpdbfromscratch",
    "genprofile",
    "guard",
    "guardsym",
    "heap",
    "highentropyva",
    "hotpatchcompatible",
    "hybriddefupgrade",
    "hybridexportthunks",
    "hybriduseguestmachine",
    "icfsection",
    "idlout",
    "ignore",
    "ignoreidl",
    "ilk",
    "iltcgwarningoff",
    "implib",
    "impthunkalign",
    "include",
    "includecoffsection",
    "incremental",
    "inferasanlibs",
    "integritycheck",
    "kernel",
    "keycontainer",
    "keyfile",
    "largeaddressaware",
    "last",
    "layoutpagesize",
    "libpath",
    "linkrepro",
    "linkreprofullpathrsp",
    "linkreprotarget",
    "lowmemory",
    "ltcg",
    "ltcgasmlist",
    "ltcgout",
    "machine",
    "manifest",
    "manifestdependency",
    "manifestfile",
    "manifestinput",
    "manifestuac",
    "map",
    "mapinfo",
    "maxilksize",
    "merge",
    "midl",
    "midlexe",
    "minpdbpathlen",
    "mt",
    "natvis",
    "noassembly",
    "nocoffgrpinfo",
    "nocomentry",
    "nod",
    "nodefaultlib",
    "noentry",
    "noexp",
    "nofunctionpadsection",
    "noilinkcoffgrppad",
    "noimplib",
    "nolinkrepro",
    "nologo",
    "nooptgids",
    "nooptidata",
    "nooptrefbeforeltcg",
    "noopttls",
    "novcfeature",
    "noxdatamerge",
    "nxcompat",
    "objmap",
    "odr",
    "odrignoresamesize",
    "onfailrepro",
    "opt",
    "opticfbytecomp",
    "order",
    "osversion",
    "out",
    "pchmap",
    "pdb",
    "pdbaltpath",
    "pdbinject",
    "pdbmap",
    "pdbpagesize",
    "pdbrpc",
    "pdbstripped",
    "pdbthreads",
    "preprocessallmodules",
    "profile",
    "release",
    "reportnoncomdatguardfunc",
    "retryonfileopenfailure",
    "safeseh",
    "section",
    "sourcelink",
    "stack",
    "stub",
    "subsystem",
    "swaprun",
    "throwingnew",
    "time",
    "tlbid",
    "tlbout",
    "tsaware",
    "useprofile",
    "verbose",
    "version",
    "wholearchive",
    "winmd",
    "winmddelaysign",
    "winmdfile",
    "winmdkeycontainer",
    "winmdkeyfile",
    "wowa64",
    "wx",
];

/// Options that mean nothing for a native user-mode image turbolink
/// writes (managed code, `link.exe`'s own machinery, its tools'
/// switches), accepted and ignored.
const IGNORED: &[&str] = &[
    "assemblydebug",
    "assemblylinkresource",
    "assemblymodule",
    "assemblyresource",
    "cgthreads",
    "clrimagetype",
    "clrloaderoptimization",
    "clrsupportlasterror",
    "clrthreadattribute",
    "clrunmanagedcodecheck",
    "d2",
    "delaysign",
    "editandcontinue",
    "emitpogophaseinfo",
    "emittoolversioninfo",
    "errorreport",
    "experimental",
    "idlout",
    "ignoreidl",
    "ilk",
    "iltcgwarningoff",
    "keycontainer",
    "keyfile",
    "linkrepro",
    "linkreprotarget",
    "midl",
    "noassembly",
    "nolinkrepro",
    "novcfeature",
    "nocoffgrpinfo",
    "pdbpagesize",
    "pdbthreads",
    "tlbid",
    "tlbout",
];

/// Reads `args` (and appends to `diags` what to report).
pub fn parse(args: &[Arg], diags: &mut Vec<Diagnostic>) -> Config {
    let mut c = Config::default();
    for a in args {
        match a {
            Arg::Input(path) => c.inputs.push(path.clone()),
            // (Only a directive may give /FAILIFMISMATCH: on the command
            // line link.exe doesn't know it, LNK4044.)
            Arg::Option { name, .. } if name.eq_ignore_ascii_case("failifmismatch") => {
                diags.push(Diagnostic::Unknown(a.to_string()));
            }
            Arg::Option { name, value } => apply(&mut c, name, value.as_deref(), a, diags),
        }
    }
    c
}

/// Applies `.drectve` options (an object's) to `c`: only those a
/// directive may give (others are reported as `link.exe` does, LNK4229).
pub fn apply_directives(c: &mut Config, args: &[Arg], diags: &mut Vec<Diagnostic>) {
    const ALLOWED: &[&str] = &[
        "alternatename",
        "defaultlib",
        "disallowlib",
        "editandcontinue",
        "entry",
        "export",
        "failifmismatch",
        "guardsym",
        "heap",
        "include",
        "inferasanlibs",
        "manifestdependency",
        "merge",
        "nodefaultlib",
        "section",
        "stack",
        "subsystem",
        "throwingnew",
        "release",
        "ltcgout",
        "dependentloadflag",
        "incremental",
    ];
    for a in args {
        match a {
            Arg::Option { name, value } if ALLOWED.contains(&name.as_str()) => {
                apply(c, name, value.as_deref(), a, diags);
            }
            other => diags.push(Diagnostic::Bad(format!(
                "directive '{other}' in an object is not allowed (LNK4229); ignored"
            ))),
        }
    }
}

fn apply(c: &mut Config, name: &str, value: Option<&str>, arg: &Arg, d: &mut Vec<Diagnostic>) {
    let bad = |d: &mut Vec<Diagnostic>| d.push(Diagnostic::Bad(format!("invalid option '{arg}'")));
    let need = |d: &mut Vec<Diagnostic>| -> Option<String> {
        match value {
            Some(v) if !v.is_empty() => Some(v.to_owned()),
            _ => {
                d.push(Diagnostic::Bad(format!("option '{arg}' needs a value")));
                None
            }
        }
    };
    macro_rules! flag {
        ($field:expr) => {
            match yes_no(value) {
                Some(b) => $field = b.into(),
                None => bad(d),
            }
        };
    }
    if name == "manifest" {
        c.manifest_given = true;
    }
    match name {
        "out" => c.out = need(d).map(PathBuf::from),
        "machine" => match value.and_then(Machine::parse) {
            Some(m) => c.machine = Some(m),
            None => bad(d),
        },
        "dll" => c.dll = true,
        "entry" => c.entry = need(d),
        "noentry" => c.no_entry = true,
        "subsystem" => {
            let Some(v) = need(d) else { return };
            let (s, ver) = match v.split_once(',') {
                Some((s, ver)) => (s, Some(ver)),
                None => (v.as_str(), None),
            };
            match Subsystem::parse(s) {
                Some(s) => c.subsystem = Some(s),
                None => return bad(d),
            }
            if let Some(ver) = ver {
                match parse_version(ver) {
                    Some(v) => c.subsystem_version = Some(v),
                    None => bad(d),
                }
            }
        }
        "osversion" => match value.and_then(parse_version) {
            Some(v) => c.os_version = Some(v),
            None => bad(d),
        },
        "version" => match value.and_then(parse_version) {
            Some(v) => c.version = v,
            None => bad(d),
        },
        "base" => {
            // `/BASE:@file,key` (a table of bases) is rare; not yet.
            let Some(v) = need(d) else { return };
            if v.starts_with('@') {
                d.push(Diagnostic::NotYet(format!("{arg} (a base file)")));
                return;
            }
            match parse_number(v.split(',').next().unwrap_or_default()) {
                Some(b) => c.base = Some(b),
                None => bad(d),
            }
        }
        "fixed" => flag!(c.fixed),
        "dynamicbase" => flag!(c.dynamic_base),
        "nxcompat" => flag!(c.nx_compat),
        "highentropyva" => match yes_no(value) {
            Some(b) => c.high_entropy_va = Some(b),
            None => bad(d),
        },
        "largeaddressaware" => match yes_no(value) {
            Some(b) => c.large_address_aware = Some(b),
            None => bad(d),
        },
        "tsaware" => match yes_no(value) {
            Some(b) => c.ts_aware = Some(b),
            None => bad(d),
        },
        "appcontainer" => flag!(c.app_container),
        "cetcompat" => flag!(c.cet_compat),
        "allowbind" => flag!(c.allow_bind),
        "allowisolation" => flag!(c.allow_isolation),
        "integritycheck" => flag!(c.integrity_check),
        "guard" => {
            for p in need(d).unwrap_or_default().split(',') {
                match p.to_ascii_lowercase().as_str() {
                    "cf" => c.guard_cf = true,
                    "no" | "nocf" => c.guard_cf = false,
                    "ehcont" => c.guard_ehcont = true,
                    "ehcont-" | "noehcont" => c.guard_ehcont = false,
                    "longjmp" => c.guard_longjmp = true,
                    "nolongjmp" => c.guard_longjmp = false,
                    // Kernel-mode and XFG variants.
                    "retpoline" | "xfg" | "delayloadsignret" | "memcpy" => {
                        d.push(Diagnostic::NotYet(format!("/GUARD:{p}")));
                    }
                    _ => bad(d),
                }
            }
        }
        "stack" => match value.and_then(parse_sizes) {
            Some(s) => c.stack = s,
            None => bad(d),
        },
        "heap" => match value.and_then(parse_sizes) {
            Some(s) => c.heap = s,
            None => bad(d),
        },
        "align" | "filealign" => {
            match value
                .and_then(parse_number)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| n.is_power_of_two())
            {
                Some(n) if name == "align" => c.align = Some(n),
                Some(n) => c.file_align = Some(n),
                None => bad(d),
            }
        }
        "libpath" => {
            if let Some(v) = need(d) {
                c.lib_paths.push(v.into());
            }
        }
        "defaultlib" => {
            if let Some(v) = need(d) {
                c.default_libs.push(v);
            }
        }
        "nodefaultlib" | "nod" => match value {
            None | Some("") => c.no_default_lib = NoDefaultLib::All,
            Some(v) => match &mut c.no_default_lib {
                NoDefaultLib::All => {}
                NoDefaultLib::These(list) => list.push(v.to_ascii_lowercase()),
                n @ NoDefaultLib::None => {
                    *n = NoDefaultLib::These(vec![v.to_ascii_lowercase()]);
                }
            },
        },
        "include" => {
            if let Some(v) = need(d) {
                c.includes.push(v);
            }
        }
        "alternatename" => match value.and_then(pair) {
            Some(p) => c.alternate_names.push(p),
            None => bad(d),
        },
        "merge" => match value.and_then(pair) {
            Some(p) => c.merges.push(p),
            None => bad(d),
        },
        "section" => match value.map(SectionAttributes::parse) {
            Some(Ok(s)) => c.sections.push(s),
            Some(Err(e)) => d.push(Diagnostic::Bad(e)),
            None => bad(d),
        },
        "export" => match value.map(Export::parse) {
            Some(Ok(e)) => c.exports.push(e),
            Some(Err(e)) => d.push(Diagnostic::Bad(e)),
            None => bad(d),
        },
        "def" => c.def = need(d).map(PathBuf::from),
        "implib" => c.implib = need(d).map(PathBuf::from),
        "noimplib" => c.no_implib = true,
        "noexp" => c.no_exp = true,
        "opt" => {
            for p in need(d).unwrap_or_default().split(',') {
                let p = p.to_ascii_lowercase();
                match p.as_str() {
                    "ref" => c.opt_ref = Some(true),
                    "noref" => c.opt_ref = Some(false),
                    "icf" => c.opt_icf = Some(true),
                    "noicf" => c.opt_icf = Some(false),
                    "lbr" => c.opt_lbr = Some(true),
                    "nolbr" => c.opt_lbr = Some(false),
                    // Windows 98's alignment; nothing now.
                    "win98" | "nowin98" => {}
                    _ => match p.strip_prefix("icf=").and_then(|n| n.parse().ok()) {
                        Some(n) => {
                            c.opt_icf = Some(true);
                            c.icf_iterations = Some(n);
                        }
                        None => bad(d),
                    },
                }
            }
        }
        "debug" => match value.map(str::to_ascii_lowercase).as_deref() {
            None | Some("full") => c.debug = Debug::Full,
            Some("fastlink") => c.debug = Debug::FastLink,
            Some("none") => c.debug = Debug::None,
            Some("ctypes" | "pdbmap") => d.push(Diagnostic::Ignored(arg.to_string())),
            Some(_) => bad(d),
        },
        "debugtype" => {
            for p in need(d).unwrap_or_default().split(',') {
                match p.to_ascii_lowercase().as_str() {
                    "cv" => {}
                    "pdata" | "fixup" => d.push(Diagnostic::NotYet(format!("/DEBUGTYPE:{p}"))),
                    _ => bad(d),
                }
            }
        }
        "pdb" => c.pdb = need(d).map(PathBuf::from),
        "pdbaltpath" => c.pdb_alt_path = need(d),
        "pdbstripped" => c.pdb_stripped = need(d).map(PathBuf::from),
        "natvis" => {
            if let Some(v) = need(d) {
                c.natvis.push(v.into());
            }
        }
        "sourcelink" => c.source_link = need(d).map(PathBuf::from),
        "map" => c.map = Some(value.filter(|v| !v.is_empty()).map(PathBuf::from)),
        "mapinfo" => match value.map(str::to_ascii_lowercase).as_deref() {
            Some("exports") => c.map_exports = true,
            // Older link.exe's `LINES` and `FIXUPS`.
            Some("lines" | "fixups") => d.push(Diagnostic::Ignored(arg.to_string())),
            _ => bad(d),
        },
        "order" => match value.and_then(|v| v.strip_prefix('@')) {
            Some(f) if !f.is_empty() => c.order = Some(f.into()),
            _ => bad(d),
        },
        "delayload" => {
            if let Some(v) = need(d) {
                c.delay_load.push(v);
            }
        }
        "delay" => match value.map(str::to_ascii_lowercase).as_deref() {
            Some("unload") => c.delay_unload = true,
            Some("nobind") => c.delay_nobind = true,
            _ => bad(d),
        },
        "dependentloadflag" => match value
            .and_then(parse_number)
            .and_then(|n| u16::try_from(n).ok())
        {
            Some(n) => c.dependent_load_flag = Some(n),
            None => bad(d),
        },
        "functionpadmin" => match value {
            None => c.function_pad_min = Some(0),
            Some(v) => match parse_number(v).and_then(|n| u32::try_from(n).ok()) {
                Some(n) => c.function_pad_min = Some(n),
                None => bad(d),
            },
        },
        "safeseh" => match yes_no(value) {
            Some(b) => c.safe_seh = Some(b),
            None => bad(d),
        },
        "stub" => c.stub = need(d).map(PathBuf::from),
        "swaprun" => match value.map(str::to_ascii_lowercase).as_deref() {
            Some("cd") => c.swap_run_cd = true,
            Some("net") => c.swap_run_net = true,
            _ => bad(d),
        },
        "release" => c.release = true,
        "brepro" => c.repro = true,
        "wholearchive" => c.whole_archive.push(value.map(str::to_owned)),
        "force" => match value.map(str::to_ascii_lowercase).as_deref() {
            None => {
                c.force_multiple = true;
                c.force_unresolved = true;
            }
            Some("multiple") => c.force_multiple = true,
            Some("unresolved") => c.force_unresolved = true,
            Some("pgorepro") => d.push(Diagnostic::Ignored(arg.to_string())),
            Some(_) => bad(d),
        },
        "manifest" => match value.map(str::to_ascii_lowercase).as_deref() {
            None => c.manifest = Manifest::SideBySide,
            Some("no") => c.manifest = Manifest::No,
            Some(v) => {
                let mut parts = v.split(',');
                if parts.next() != Some("embed") {
                    return bad(d);
                }
                let mut id = None;
                for p in parts {
                    match p
                        .strip_prefix("id=")
                        .and_then(parse_number)
                        .and_then(|n| u16::try_from(n).ok())
                    {
                        Some(n) => id = Some(n),
                        None => return bad(d),
                    }
                }
                c.manifest = Manifest::Embed { id };
            }
        },
        "manifestuac" => match value {
            None => c.manifest_uac = Some(Some(String::new())),
            Some(v) if v.eq_ignore_ascii_case("no") => c.manifest_uac = Some(None),
            Some(v) => c.manifest_uac = Some(Some(v.to_owned())),
        },
        "manifestdependency" => {
            if let Some(v) = need(d) {
                c.manifest_dependencies.push(v);
            }
        }
        "manifestfile" => c.manifest_file = need(d).map(PathBuf::from),
        "manifestinput" => {
            if let Some(v) = need(d) {
                c.manifest_inputs.push(v.into());
            }
        }
        "failifmismatch" => match value.and_then(pair) {
            Some(p) => c.fail_if_mismatch.push(p),
            None => bad(d),
        },
        "wx" => match value {
            None => c.warnings_as_errors = Some(Vec::new()),
            Some(v) if v.eq_ignore_ascii_case("no") => c.warnings_as_errors = None,
            Some(v) => match v
                .split(',')
                .map(|n| n.trim().parse::<u32>())
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(list) => c.warnings_as_errors = Some(list),
                Err(_) => bad(d),
            },
        },
        "ignore" => {
            for n in need(d).unwrap_or_default().split(',') {
                match n.trim().parse() {
                    Ok(n) => c.ignore_warnings.push(n),
                    Err(_) => bad(d),
                }
            }
        }
        "nologo" => c.no_logo = true,
        "verbose" => c.verbose = true,
        "time" | "time+" => c.time = true,
        "ltcg" | "ltcgout" | "useprofile" | "genprofile" | "fastgenprofile" => {
            let v = value.map(str::to_ascii_lowercase);
            if name == "ltcg" && v.as_deref() == Some("off") {
                c.ltcg = false;
            } else {
                c.ltcg = true;
                // (Plain LTCG is done; incremental LTCG and PGO aren't yet.)
                let plain =
                    name == "ltcg" && matches!(v.as_deref(), None | Some("status" | "nostatus"));
                if !plain {
                    d.push(Diagnostic::NotYet(arg.to_string()));
                }
            }
        }
        "incremental" => match yes_no(value) {
            Some(b) => c.incremental = Some(b),
            None => bad(d),
        },
        "driver" | "kernel" => {
            c.kernel = true;
            d.push(Diagnostic::NotYet(arg.to_string()));
        }
        "throwingnew" => c.throwing_new = true,
        "profile" => {
            // Keeps relocations and a full PDB for profilers: /FIXED:NO,
            // /DEBUG, /INCREMENTAL:NO (and /OPT:REF stays).
            c.fixed = false;
            if c.debug == Debug::None {
                c.debug = Debug::Full;
            }
            c.incremental = Some(false);
        }
        // Directives that only check or note (handled by the link: kept
        // for it to see).
        "disallowlib" => {
            if let Some(v) = need(d) {
                c.disallow_libs.push(v);
            }
        }
        "guardsym" | "inferasanlibs" => {}
        _ if IGNORED.contains(&name) => d.push(Diagnostic::Ignored(arg.to_string())),
        _ if KNOWN.contains(&name) => d.push(Diagnostic::NotYet(arg.to_string())),
        _ => d.push(Diagnostic::Unknown(arg.to_string())),
    }
}

#[cfg(test)]
mod tests;
