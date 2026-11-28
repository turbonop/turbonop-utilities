//! A link, from its configuration to the image written.

use std::path::PathBuf;
use std::time::Instant;

use crate::config::Config;
use crate::resolve::{self, LinkError};
use crate::write::{Image, base_relocation_blocks};
use crate::{exports, implib, layout};

/// What a link did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Errors and warnings, in order.
    pub diagnostics: Vec<LinkError>,
    /// Each phase's time (`/TIME`).
    pub times: Vec<(&'static str, std::time::Duration)>,
    /// Whether the image was written.
    pub written: bool,
}

/// The output's path: `/OUT`, or the first object's name with `.exe`
/// (`.dll` for `/DLL`).
pub fn output_path(c: &Config) -> Option<PathBuf> {
    if let Some(o) = &c.out {
        return Some(o.clone());
    }
    let first = c.inputs.first()?;
    let mut p = PathBuf::from(first);
    p.set_extension(if c.dll { "dll" } else { "exe" });
    p.file_name().map(PathBuf::from)
}

/// Links.
pub fn link(mut config: Config) -> Outcome {
    let mut out = Outcome::default();
    if let Some(def) = config.def.clone() {
        let parsed = std::fs::read(&def)
            .map_err(|e| format!("cannot open file '{}': {e}", def.display()))
            .and_then(|b| {
                exports::parse_def(&crate::args::decode_text(&b))
                    .map_err(|e| format!("{}: {e}", def.display()))
            });
        match parsed {
            Ok(d) => exports::apply_def(&mut config, &d),
            Err(m) => {
                out.diagnostics.push(LinkError {
                    code: 1118,
                    message: m,
                    warning: false,
                });
                return out;
            }
        }
    }
    let mut phase = Instant::now();
    let mut lap = |name: &'static str, out: &mut Outcome| {
        let now = Instant::now();
        out.times.push((name, now - phase));
        phase = now;
    };
    let Some(path) = output_path(&config) else {
        out.diagnostics.push(LinkError {
            code: 1561,
            message: "entry point must be defined".into(),
            warning: false,
        });
        return out;
    };
    let Some(resolved) = resolve::resolve(config, &mut out.diagnostics) else {
        return out;
    };
    lap("resolve", &mut out);
    out.times.extend(resolved.times.iter().copied());
    for line in &resolved.log {
        eprintln!("{line}");
    }

    let mut roots: Vec<u32> = resolved.entry.into_iter().collect();
    for name in resolved
        .config
        .includes
        .iter()
        .map(String::as_bytes)
        // (The delay-load helper: only the tail merges turbolink writes
        // call it.)
        .chain([
            &b"_load_config_used"[..],
            b"_tls_used",
            b"__delayLoadHelper2",
        ])
    {
        if let Some(&g) = resolved.names.get(name) {
            roots.push(g);
        }
    }
    for e in &resolved.config.exports {
        let name = e.internal.as_deref().unwrap_or(&e.name);
        if let Some(&g) = resolved.names.get(name.as_bytes()) {
            roots.push(g);
        }
    }
    let export_table = match export_table(&resolved, &path) {
        Ok(t) => t,
        Err(m) => {
            out.diagnostics.push(LinkError {
                code: 1120,
                message: m,
                warning: false,
            });
            return out;
        }
    };
    let mut all = Vec::new();
    for (source, (name, data)) in resolved.resources.iter().enumerate() {
        match crate::resources::parse_res(data) {
            Ok(r) => all.extend(r.into_iter().map(|mut r| {
                r.source = crate::n32(source);
                r
            })),
            Err(m) => {
                out.diagnostics.push(LinkError {
                    code: 1136,
                    message: format!("{name}: invalid or corrupt file: {m}"),
                    warning: false,
                });
                return out;
            }
        }
    }
    // The manifest: written beside the image, or one of its resources.
    let mut res_beside_manifest = 0;
    match crate::manifest::make(&resolved.config, &path) {
        Ok(Some(embedded)) => {
            res_beside_manifest = crate::n32(resolved.resources.len());
            all.push(embedded);
        }
        Ok(None) => {}
        Err(e) => {
            out.diagnostics.push(e);
            return out;
        }
    }
    let resources = if all.is_empty() {
        None
    } else {
        match crate::resources::plan(all) {
            Ok(mut r) => {
                r.res_beside_manifest = res_beside_manifest;
                Some(r)
            }
            Err(m) => {
                out.diagnostics.push(LinkError {
                    code: 1123,
                    message: m,
                    warning: false,
                });
                return out;
            }
        }
    };
    let mut layout = layout::layout(&resolved, &roots, export_table, resources);
    let debug = resolved.config.debug != crate::config::Debug::None;
    let pdb_path = crate::pdb::pdb_path(&resolved, &path);
    if debug {
        layout.set_codeview_name(crate::pdb::image_pdb_name(&resolved, &pdb_path));
    }
    lap("layout", &mut out);
    let sites = Image::new(&resolved, &layout).base_relocation_sites();
    layout.add_base_relocations(base_relocation_blocks(&sites));
    lap("base relocations", &mut out);

    let image = Image::new(&resolved, &layout);
    // The image and the PDB, at once.
    let ((mut bytes, problems), pdb) = turbonop_par::join(
        || image.write(resolved.entry),
        || debug.then(|| crate::pdb::build(&resolved, &layout, &image, &pdb_path)),
    );
    lap("write (and PDB)", &mut out);
    if let Some(pdb) = pdb {
        match pdb {
            Ok((mut written, warnings)) => {
                for w in warnings {
                    out.diagnostics.push(LinkError {
                        code: 4099,
                        message: w,
                        warning: true,
                    });
                }
                // One GUID for both: with /Brepro the image's hash's (as
                // link.exe's), else the PDB's contents'.
                if let Some(c) = layout
                    .chunks
                    .iter()
                    .find(|c| c.kind == crate::layout::ChunkKind::CodeView)
                    && let Some(at) = layout.file_offset_of(c.rva + 4)
                {
                    let at = at as usize;
                    if resolved.config.repro {
                        let mut guid = [0u8; 16];
                        guid.copy_from_slice(&bytes[at..at + 16]);
                        written.set_guid(guid);
                    } else {
                        bytes[at..at + 16].copy_from_slice(&written.guid);
                    }
                }
                if let Err(e) = std::fs::write(&pdb_path, &written.file) {
                    out.diagnostics.push(LinkError {
                        code: 1201,
                        message: format!(
                            "error writing to program database '{}': {e}",
                            pdb_path.display()
                        ),
                        warning: false,
                    });
                }
                lap("PDB written", &mut out);
            }
            Err(m) => out.diagnostics.push(LinkError {
                code: 1201,
                message: format!("program database '{}': {m}", pdb_path.display()),
                warning: false,
            }),
        }
    }
    if let Some(map) = &resolved.config.map {
        let map_path = map.clone().unwrap_or_else(|| path.with_extension("map"));
        let text = image.map(&bytes, &path);
        if let Err(e) = std::fs::write(&map_path, text) {
            out.diagnostics.push(LinkError {
                code: 1104,
                message: format!("cannot open file '{}': {e}", map_path.display()),
                warning: false,
            });
        }
        lap("map", &mut out);
    }
    for p in problems {
        out.diagnostics.push(LinkError {
            code: 0,
            message: p,
            warning: false,
        });
    }
    if out.diagnostics.iter().any(|d| !d.warning) {
        return out;
    }
    if let Err(e) = std::fs::write(&path, &bytes) {
        out.diagnostics.push(LinkError {
            code: 1104,
            message: format!("cannot open file '{}': {e}", path.display()),
            warning: false,
        });
        return out;
    }
    lap("output", &mut out);
    // The import library, for a DLL (or anything exporting).
    let c = &resolved.config;
    if let Some(t) = &layout.exports {
        let lib_path = c
            .implib
            .clone()
            .unwrap_or_else(|| path.with_extension("lib"));
        let machine = c.machine.unwrap_or(crate::config::Machine::X64);
        let mut hints = vec![0u16; t.directory.exports.len()];
        for (k, (_, i)) in t.directory.names.iter().enumerate() {
            hints[*i] = u16::try_from(k).unwrap_or(0);
        }
        // (Not with /NOIMPLIB; the export object still, as link.exe.)
        let lib = (!c.no_implib).then(|| {
            implib::write(
                &t.dll_name,
                machine.number(),
                machine.is_64(),
                &t.directory.exports,
                &hints,
                crate::write::timestamp(c),
            )
        });
        if let Some(lib) = lib
            && let Err(e) = std::fs::write(&lib_path, lib)
        {
            out.diagnostics.push(LinkError {
                code: 1104,
                message: format!("cannot open file '{}': {e}", lib_path.display()),
                warning: false,
            });
            return out;
        }
        lap("import library", &mut out);
        // The export object beside it (link.exe's .exp), unless /NOEXP.
        if !c.no_exp {
            let exp_path = lib_path.with_extension("exp");
            let cwd = std::env::current_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let exe = std::env::current_exe()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let def_text = c.def.as_ref().and_then(|d| std::fs::read(d).ok());
            let def_path = c
                .def
                .as_ref()
                .map(|d| std::path::absolute(d).unwrap_or_else(|_| d.clone()));
            let ordinals_given: Vec<String> = c
                .exports
                .iter()
                .filter(|e| e.ordinal.is_some())
                .map(|e| crate::exports::exported_name(e).to_owned())
                .collect();
            let exp = crate::exp::write(
                &t.directory,
                &crate::exp::Inputs {
                    path: &exp_path,
                    dll_name: &t.dll_name,
                    directives: c.def_directives.as_deref(),
                    machine: machine.number(),
                    def: def_path.as_deref().zip(def_text.as_deref()),
                    cwd: &cwd,
                    exe: &exe,
                    ordinals_given: &ordinals_given,
                },
            );
            if let Err(e) = std::fs::write(&exp_path, exp) {
                out.diagnostics.push(LinkError {
                    code: 1104,
                    message: format!("cannot open file '{}': {e}", exp_path.display()),
                    warning: false,
                });
                return out;
            }
            lap("export object", &mut out);
        }
    }
    out.written = true;
    out
}

/// The export directory's plan, if the image exports anything.
fn export_table(
    r: &resolve::Resolved,
    path: &std::path::Path,
) -> Result<Option<exports::ExportTable>, String> {
    if r.config.exports.is_empty() {
        return Ok(None);
    }
    let list = exports::finish(&r.config.exports)?;
    let defined = |n: &str| {
        r.names
            .get(n.as_bytes())
            .is_some_and(|&g| r.globals[g as usize].def != resolve::Def::Undefined)
    };
    let mut forwarders = Vec::with_capacity(list.len());
    let mut targets = Vec::with_capacity(list.len());
    for e in &list {
        let f = exports::forwarder(e, defined).map(str::to_owned);
        let target = e.internal.as_deref().unwrap_or(&e.name);
        targets.push(if f.is_some() {
            u32::MAX
        } else {
            match r.names.get(target.as_bytes()) {
                Some(&g) if r.globals[g as usize].def != resolve::Def::Undefined => g,
                _ => return Err(format!("unresolved external symbol {target} (an export)")),
            }
        });
        forwarders.push(f);
    }
    let dll_name = r.config.library_name.clone().unwrap_or_else(|| {
        path.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    });
    let directory = exports::plan(list, &dll_name, &forwarders);
    Ok(Some(exports::ExportTable {
        directory,
        forwarders,
        targets,
        dll_name,
    }))
}
