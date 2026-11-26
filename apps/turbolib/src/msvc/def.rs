//! `/DEF`: an import library and an export file (`lib.exe`), or an import
//! library (`llvm-lib`), from a module-definition file, `/EXPORT` options,
//! and the objects' `/EXPORT` directives.
//!
//! Measured with `lib.exe` 14.52:
//!
//! - **Inputs.** `/DEF:x.def` (the last one counts), or `/DEF` alone for
//!   none. A `.def` file that can't be read is LNK1104. Its exports come
//!   first, then the `/EXPORT` options', then the objects' directives.
//!   Objects are read for their definitions and directives and are not
//!   members; libraries are ignored (with LNK4001 if there are no
//!   objects). The machine is `/MACHINE`'s, or the objects', or x64 (with
//!   LNK4068).
//! - **Names.** The output is `/OUT`, or the `.def` file's name with
//!   `.lib` (with neither, LNK1159). The export file is the output with
//!   `.exp` for its last extension (`o` gives `o.exp`, `o.x.y` `o.x.exp`).
//!   The DLL is the one `NAME` or `LIBRARY` names, else `/NAME`'s (with
//!   `.dll` if it has no `.`), else the `.def` file's name, else the
//!   output's, with `.dll`.
//! - **Messages.** A `.def` file's are `x.def(line) : ...`, as
//!   [`turbonop_moduledef`] describes them; LNK4017 and LNK4275 can't be
//!   silenced by `/IGNORE`. `   Creating library x.lib and object x.exp`
//!   is printed before the library is written, and before LNK1121 (two
//!   exports with one ordinal), which then stops it.

use super::args::Flavor;
use super::diag::{Diag, Kind, Reporter};
use super::inputs::{self, InputKind, MachineCheck};
use super::options::Options;
use super::run::{Env, now, read_input, resolve, write_file};
use std::borrow::Cow;
use turbonop_archive::write::{Style, Tool};
use turbonop_moduledef::descriptors::Stamp;
use turbonop_moduledef::exp::{DefFile, ExportFile, directives, greatest_given};
use turbonop_moduledef::exports::{
    Definitions, Origin, Resolved, Unresolved, resolve as resolve_exports,
};
use turbonop_moduledef::imports::ImportLibrary;
use turbonop_moduledef::option::{OptionError, drectve_exports, parse_export_option};
use turbonop_moduledef::{Export, ModuleDefinition, ParseError, Warning};

/// Runs `/DEF`.
pub fn run(flavor: Flavor, opts: &Options, env: &Env, r: &mut Reporter) -> Result<(), ()> {
    match flavor {
        Flavor::Lib => lib(opts, env, r),
        Flavor::LlvmLib => llvm(opts, env, r),
    }
}

/// `llvm-lib /def:` (read in LLVM's `LibDriver.cpp`): `/out` and
/// `/machine` are required, the `.def` file is read with LLVM's grammar
/// ([`turbonop_moduledef::llvm`]), inputs are ignored, and no export file
/// is written.
fn llvm(opts: &Options, env: &Env, r: &mut Reporter) -> Result<(), ()> {
    use turbonop_moduledef::llvm::{ec_insertion, members, parse};
    let fail = |r: &mut Reporter, text: String| r.report(Diag::llvm(Kind::Fatal, text));
    let Some(out) = &opts.out else {
        return fail(r, "no output path given".into());
    };
    let Some(m) = opts.machine.map(super::options::Machine::number) else {
        return fail(r, "/def option requires /machine to be specified".into());
    };
    let read = |name: &str| {
        std::fs::read(resolve(&env.cwd, name)).map_err(|e| {
            let why = match e.kind() {
                std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
                std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
                _ => e.to_string(),
            };
            format!("cannot open file {name}: {why}")
        })
    };
    let name = opts.def.clone().flatten().unwrap_or_default();
    let text = match read(&name) {
        Ok(t) => t,
        Err(e) => return fail(r, e),
    };
    if text.is_empty() {
        return fail(r, "definition file empty".into());
    }
    let parse_error = "Invalid data was encountered while parsing the file";
    let Ok(def) = parse(&text, m) else {
        return fail(
            r,
            format!(
                "error parsing definition
{parse_error}"
            ),
        );
    };
    let mut output_file = def.output_file.clone();
    let mut native = Vec::new();
    if matches!(m, inputs::ARM64EC | inputs::ARM64X)
        && let Some(n) = &opts.def_arm64_native
    {
        let text = match read(n) {
            Ok(t) => t,
            Err(e) => return fail(r, e),
        };
        if text.is_empty() {
            return fail(r, "native definition file empty".into());
        }
        let Ok(nd) = parse(&text, inputs::ARM64) else {
            return fail(
                r,
                format!(
                    "error parsing native definition
{parse_error}"
                ),
            );
        };
        native = nd.exports;
        output_file = nd.output_file;
    }
    let written = members(&output_file, &def.exports, m, &native, &ec_insertion)
        .map_err(|e| e.0)
        .and_then(|ms| {
            ms.write(Style::LlvmLib, Tool::LlvmLib)
                .map_err(|e| e.to_string())
        });
    let bytes = match written {
        Ok(b) => b,
        Err(e) => return fail(r, format!("{out}: {e}")),
    };
    if write_file(&resolve(&env.cwd, out), &bytes).is_err() {
        return r.report(Diag::llvm(Kind::Fatal, "cannot write the output").on(out.clone()));
    }
    Ok(())
}

/// The file name of `path` (after its last `\` or `/`).
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `path` with its last extension (in its file name) replaced by `ext`,
/// or `ext` added.
fn with_extension(path: &str, ext: &str) -> String {
    let base = path.len() - file_name(path).len();
    match path[base..].rfind('.') {
        Some(dot) => format!("{}.{ext}", &path[..base + dot]),
        None => format!("{path}.{ext}"),
    }
}

/// The file name of `path` without its last extension.
fn stem(path: &str) -> &str {
    let f = file_name(path);
    f.rfind('.').map_or(f, |d| &f[..d])
}

/// A `.def` file read, as messages name it.
struct DefText {
    /// As given on the command line.
    name: String,
    /// Its full path, as the export file records it.
    full: String,
    text: Vec<u8>,
}

/// The first word of line `line` (from 1) of `text`.
fn first_word(text: &[u8], line: usize) -> String {
    let l = text
        .split(|&b| b == b'\n')
        .nth(line.saturating_sub(1))
        .unwrap_or_default();
    let w = l
        .split(|&b| b == b' ' || b == b'\t' || b == b'\r')
        .find(|w| !w.is_empty())
        .unwrap_or_default();
    String::from_utf8_lossy(w).into_owned()
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A warning `lib.exe` gives even under `/IGNORE`.
fn report_unignorable(r: &mut Reporter, d: Diag) -> Result<(), ()> {
    let saved = r.policy.ignored.clone();
    r.policy.ignored.retain(|&c| c != d.code);
    let result = r.report(d);
    r.policy.ignored = saved;
    result
}

/// A `.def` file's warnings, as `lib.exe` words them.
fn def_warnings(r: &mut Reporter, def: &DefText, w: &[Warning<'_>]) -> Result<(), ()> {
    let at = |line: usize| format!("{}({line})", def.name);
    for w in w {
        match *w {
            Warning::Ignored { line, statement } => report_unignorable(
                r,
                Diag::warning(
                    4017,
                    format!(
                        "{} statement not supported for the target platform; ignored",
                        show(statement)
                    ),
                )
                .on(at(line)),
            )?,
            Warning::SecondModule { line } => report_unignorable(
                r,
                Diag::warning(
                    4275,
                    format!(
                        "'{}' statement cannot appear after 'LIBRARY' or 'NAME'; ignored",
                        first_word(&def.text, line)
                    ),
                )
                .on(at(line)),
            )?,
            Warning::DirectoryIgnored { line } => r.report(
                Diag::warning(
                    4093,
                    format!(
                        "Drive/Directory component ignored in '{}' statement",
                        first_word(&def.text, line)
                    ),
                )
                .on(at(line)),
            )?,
            Warning::Constant { line } => r.report(
                Diag::warning(4087, "CONSTANT keyword is obsolete; use DATA").on(at(line)),
            )?,
            Warning::DuplicateExport { name, .. } => r.report(
                Diag::warning(
                    4197,
                    format!(
                        "export '{}' specified multiple times; using first specification",
                        show(name)
                    ),
                )
                .on(def.name.clone()),
            )?,
        }
    }
    Ok(())
}

/// A `.def` file's error, as `lib.exe` words it.
fn def_error(def: &DefText, e: &ParseError<'_>) -> Diag {
    let at = |line: usize| format!("{}({line})", def.name);
    match *e {
        ParseError::Syntax { line, word } => {
            Diag::fatal(1118, format!("syntax error in '{}' statement", show(word))).on(at(line))
        }
        ParseError::InvalidOrdinal { line, text } => {
            Diag::fatal(1119, format!("invalid ordinal number '{}'", show(text))).on(at(line))
        }
        ParseError::DuplicateOrdinal { ordinal } => {
            Diag::fatal(1121, format!("duplicate ordinal number '{ordinal}'")).on(def.name.clone())
        }
        ParseError::CommitAboveReserve { line } => Diag::fatal(
            1229,
            format!(
                "Commit size greater than reserve size in '{}'",
                first_word(&def.text, line)
            ),
        )
        .on(at(line)),
    }
}

/// Reads the `.def` file named `name`.
fn read_def(env: &Env, name: &str) -> Result<DefText, Diag> {
    let path = resolve(&env.cwd, name);
    let text = std::fs::read(&path)
        .map_err(|_| Diag::fatal(1104, format!("cannot open file '{name}'")))?;
    Ok(DefText {
        name: name.to_string(),
        full: full_path(&path),
        text,
    })
}

/// `path` made whole as Windows' `GetFullPathName` makes it: `\` for `/`,
/// and `.` and `..` resolved.
fn full_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy().replace('/', "\\");
    let (prefix, rest) = match s.find(":\\") {
        Some(i) => (&s[..i + 2], &s[i + 2..]),
        None => ("", &s[..]),
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('\\') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("{prefix}{}", parts.join("\\"))
}

/// An object's definitions and `/EXPORT` directives.
struct ObjectExports {
    name: String,
    defined: Vec<Vec<u8>>,
    directives: Vec<Vec<u8>>,
}

fn object_exports(name: &str, data: &[u8]) -> Option<ObjectExports> {
    let o = turbonop_coff::Object::parse(data).ok()?;
    let defined = o
        .symbols
        .iter()
        .filter(|s| s.is_external() && s.section.id().is_some())
        .map(|s| s.name.as_bytes().to_vec())
        .collect();
    let mut directives = Vec::new();
    for s in &o.sections {
        if s.name.as_bytes() == b".drectve" {
            directives.extend(drectve_exports(s.bytes()));
        }
    }
    Some(ObjectExports {
        name: name.to_string(),
        defined,
        directives,
    })
}

fn option_error(e: OptionError<'_>) -> Diag {
    match e {
        OptionError::Invalid => Diag::fatal(1161, "invalid export specification"),
        OptionError::InvalidOrdinal(t) => {
            Diag::fatal(1119, format!("invalid ordinal number '{}'", show(t)))
        }
    }
}

fn lib(opts: &Options, env: &Env, r: &mut Reporter) -> Result<(), ()> {
    // The .def file.
    let def_name = opts.def.clone().flatten().filter(|n| !n.is_empty());
    let def_text = match &def_name {
        Some(n) => match read_def(env, n) {
            Ok(d) => Some(d),
            Err(d) => return r.report(d),
        },
        None => None,
    };
    let empty = ModuleDefinition::default();
    let mut deferred = None;
    let parsed = match &def_text {
        Some(d) => match turbonop_moduledef::parse(&d.text) {
            Ok(p) => Some(p),
            Err(e @ ParseError::DuplicateOrdinal { .. }) => {
                deferred = Some(def_error(d, &e));
                None
            }
            Err(e) => return r.report(def_error(d, &e)),
        },
        None => None,
    };
    let def = parsed.as_ref().unwrap_or(&empty);
    if let Some(d) = &def_text {
        def_warnings(r, d, &def.warnings)?;
    }
    let native_text = match &opts.def_arm64_native {
        Some(n) => match read_def(env, n) {
            Ok(d) => Some(d),
            Err(d) => return r.report(d),
        },
        None => None,
    };
    let native = match &native_text {
        Some(d) => match turbonop_moduledef::parse(&d.text) {
            Ok(p) => {
                def_warnings(r, d, &p.warnings)?;
                Some(p)
            }
            Err(e) => return r.report(def_error(d, &e)),
        },
        None => None,
    };

    // The command line's exports.
    let mut given: Vec<(Export<'_>, Origin)> = def
        .exports()
        .into_iter()
        .map(|e| (e, Origin::Definition))
        .collect();
    for spec in &opts.exports {
        match parse_export_option(spec.as_bytes()) {
            Ok(o) => {
                if o.extra_delimiter {
                    r.report(Diag::warning(
                        4051,
                        "extra delimiter in /EXPORT specification",
                    ))?;
                }
                given.push((o.export, Origin::Option));
            }
            Err(e) => return r.report(option_error(e)),
        }
    }
    // A name given again: the first, with LNK4197 if it differs.
    let mut kept: Vec<(Export<'_>, Origin)> = Vec::with_capacity(given.len());
    for (e, o) in given {
        match kept.iter().find(|(k, _)| k.name == e.name) {
            Some((k, _)) => {
                let same = Export { line: 0, ..*k } == Export { line: 0, ..e };
                if !same {
                    r.report(Diag::warning(
                        4197,
                        format!(
                            "export '{}' specified multiple times; using first specification",
                            show(e.name)
                        ),
                    ))?;
                }
            }
            None => kept.push((e, o)),
        }
    }

    // The objects.
    let mut read = Vec::new();
    for name in &opts.inputs {
        match read_input(opts, env, name, Flavor::Lib) {
            Ok(i) => read.push(i),
            Err(d) => return r.report(d),
        }
    }
    let mut check = MachineCheck::new(
        opts.machine.map(super::options::Machine::number),
        Flavor::Lib,
    );
    let mut objects = Vec::new();
    let mut libraries = 0;
    for i in &read {
        let kind = match inputs::classify(Flavor::Lib, &i.name, &i.data) {
            Ok(k) => k,
            Err(d) => return r.report(d),
        };
        match kind {
            InputKind::Library => libraries += 1,
            InputKind::Object(_) => {
                if let Err(d) = check.lib_object(&i.name, kind) {
                    return r.report(d);
                }
                let Some(x) = object_exports(&i.name, &i.data) else {
                    return r
                        .report(Diag::fatal(1107, "invalid or corrupt file").on(i.name.clone()));
                };
                objects.push(x);
            }
            _ => {
                if let Err(d) = check.lib_object(&i.name, kind) {
                    return r.report(d);
                }
            }
        }
    }
    if objects.is_empty() && libraries > 0 {
        r.report(Diag::warning(
            4001,
            "no object files specified; libraries used",
        ))?;
    }
    let machine = match check.machine {
        Some(m) => m,
        None => {
            r.report(Diag::warning(
                4068,
                "/MACHINE not specified; defaulting to X64",
            ))?;
            inputs::X64
        }
    };
    let directive_specs: Vec<&Vec<u8>> = objects.iter().flat_map(|o| &o.directives).collect();
    for spec in &directive_specs {
        match parse_export_option(spec) {
            Ok(o) => kept.push((o.export, Origin::Directive)),
            Err(e) => return r.report(option_error(e)),
        }
    }

    // The names.
    let Some(out) = opts
        .out
        .clone()
        .or_else(|| def_name.as_ref().map(|d| format!("{}.lib", stem(d))))
    else {
        return r.report(Diag::fatal(1159, "no output file specified"));
    };
    let exp = with_extension(&out, "exp");
    let def_stem = def_name.as_deref().map_or_else(|| stem(&out), stem);
    let named = def.module.as_ref().is_some_and(|m| m.name.is_some());
    let dll: Vec<u8> = match (&opts.name, named) {
        (Some(n), false) => {
            let mut v = n.as_bytes().to_vec();
            if !n.contains('.') {
                v.extend_from_slice(b".dll");
            }
            v
        }
        _ => def.file_name(def_stem.as_bytes()),
    };
    // ARM64X: the DLL the native .def file names (measured).
    let dll = match (&native, &native_text) {
        (Some(n), Some(t))
            if machine == inputs::ARM64X && n.module.as_ref().is_some_and(|m| m.name.is_some()) =>
        {
            n.file_name(stem(file_name(&t.name)).as_bytes())
        }
        _ => dll,
    };

    // Each export resolved.
    let ec_machine = if machine == inputs::ARM64X {
        inputs::ARM64EC
    } else {
        machine
    };
    let defined: Vec<&[u8]> = objects
        .iter()
        .flat_map(|o| o.defined.iter().map(Vec::as_slice))
        .collect();
    let definitions = Definitions::new(defined);
    let resolved = match resolve_exports(
        &kept,
        ec_machine,
        (!objects.is_empty()).then_some(&definitions),
    ) {
        Ok(v) => v,
        Err(errors) => return unresolved(r, def_text.as_ref(), &out, &errors, &objects),
    };
    let native_resolved: Vec<Resolved<'_>> = match &native {
        Some(n) => {
            let given: Vec<_> = n
                .exports()
                .into_iter()
                .map(|e| (e, Origin::Definition))
                .collect();
            resolve_exports(&given, inputs::ARM64, None).unwrap_or_default()
        }
        None => Vec::new(),
    };

    r.lines
        .push(format!("   Creating library {out} and object {exp}"));
    if let Some(d) = deferred {
        return r.report(d);
    }

    // The library.
    let mut plan = ImportLibrary {
        machine,
        dll: dll.clone(),
        imports: ImportLibrary::imports_of(&resolved, ec_machine),
        native: ImportLibrary::imports_of(&native_resolved, inputs::ARM64),
    };
    if machine != inputs::ARM64X {
        plan.native.clear();
    }
    let time = u32::try_from(now()).unwrap_or(u32::MAX);
    let (stamp, style) = if opts.brepro {
        (Stamp::Brepro, Style::LibExeBrepro)
    } else {
        (
            Stamp::Time(time),
            Style::LibExe {
                time: u64::from(time),
            },
        )
    };
    let library = match plan.members(stamp).write(style, Tool::LibExe) {
        Ok(b) => b,
        Err(e) => return r.report(Diag::fatal(1104, format!("cannot write the library: {e}"))),
    };

    // The export file.
    let cwd = env.cwd.to_string_lossy().into_owned();
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut drectve = def_text
        .as_ref()
        .map_or_else(Vec::new, |_| directives(def, def_stem.as_bytes()));
    if let (Some(n), Some(_)) = (&native, &native_text) {
        drectve.extend(directives(n, def_stem.as_bytes()));
    }
    // ARM64EC and ARM64X: a second export file for the native exports
    // (with none for ARM64EC), the tables sized for both.
    let ec = ec_machine == inputs::ARM64EC;
    let both = count(resolved.len() + native_resolved.len());
    let file = ExportFile {
        machine: ec_machine,
        dll: &dll,
        exports: &resolved,
        directives: &drectve,
        name: exp.as_bytes(),
        cwd: cwd.as_bytes(),
        exe: exe.as_bytes(),
        def: def_text.as_ref().map(|d| DefFile {
            path: d.full.as_bytes(),
            contents: &d.text,
        }),
        time: (!opts.brepro).then_some(time),
        reserved: native_resolved
            .iter()
            .filter_map(|e| e.ordinal)
            .min()
            .map(|m| (m, None)),
        slots: if ec { both } else { 0 },
    };
    let mut files = vec![(out.clone(), library), (exp.clone(), file.write())];
    if ec {
        let name = with_extension(&out, "a64n.exp");
        let native_file = ExportFile {
            machine: inputs::ARM64,
            exports: &native_resolved,
            directives: &[],
            name: name.as_bytes(),
            reserved: Some((file.base(), greatest_given(&resolved))),
            slots: file.functions().max(both),
            ..file
        };
        let bytes = native_file.write();
        files.push((name, bytes));
    }
    if !r.write_output {
        return Ok(());
    }
    drop(read);
    for (path, bytes) in &files {
        if write_file(&resolve(&env.cwd, path), bytes).is_err() {
            return r.report(Diag::fatal(1104, format!("cannot open file '{path}'")));
        }
    }
    Ok(())
}

/// LNK2001 for each export not found (LNK4022 and LNK4002 for one with
/// several matches), then LNK1120.
fn unresolved(
    r: &mut Reporter,
    def: Option<&DefText>,
    out: &str,
    errors: &[Unresolved<'_>],
    objects: &[ObjectExports],
) -> Result<(), ()> {
    let at = def.map_or_else(|| "LINK".to_string(), |d| d.name.clone());
    let mut count = 0;
    for e in errors {
        let name: Cow<'_, [u8]> = match e {
            Unresolved::Missing(n) => n.clone(),
            Unresolved::Ambiguous(n, matches) => {
                r.report(
                    Diag::warning(
                        4022,
                        format!("cannot find unique match for symbol '{}'", show(n)),
                    )
                    .on(at.clone()),
                )?;
                for m in matches {
                    let text = show(m);
                    let plain =
                        turbonop_demangle::msvc::demangle(&text, 0).unwrap_or_else(|| text.clone());
                    let object = objects
                        .iter()
                        .find(|o| o.defined.iter().any(|d| d == m))
                        .map_or("", |o| o.name.as_str());
                    r.report(
                        Diag::warning(4002, format!("\"{plain}\" ({text}) defined in {object}"))
                            .on(at.clone()),
                    )?;
                }
                Cow::Borrowed(*n)
            }
        };
        count += 1;
        let d = Diag {
            kind: Kind::Error,
            code: 2001,
            file: Some(at.clone()),
            text: format!("unresolved external symbol {}", show(&name)),
        };
        r.report(d)?;
    }
    r.report(Diag::fatal(1120, format!("{count} unresolved externals")).on(out.to_string()))
}

/// A count as a 32-bit number.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
