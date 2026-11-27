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
        .chain([&b"_load_config_used"[..], b"_tls_used"])
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
    for (name, data) in &resolved.resources {
        match crate::resources::parse_res(data) {
            Ok(r) => all.extend(r),
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
    let resources = if all.is_empty() {
        None
    } else {
        match crate::resources::plan(all) {
            Ok(r) => Some(r),
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
    lap("layout", &mut out);
    let sites = Image::new(&resolved, &layout).base_relocation_sites();
    layout.add_base_relocations(base_relocation_blocks(&sites));
    lap("base relocations", &mut out);

    let image = Image::new(&resolved, &layout);
    let (bytes, problems) = image.write(resolved.entry);
    lap("write", &mut out);
    if let Some(map) = &resolved.config.map {
        let map_path = map.clone().unwrap_or_else(|| path.with_extension("map"));
        let mut lines: Vec<(u32, String)> = (0..resolved.globals.len() as u32)
            .filter_map(|g| {
                let rva = image.rva_of(g)?;
                let global = &resolved.globals[g as usize];
                let file = match global.def {
                    resolve::Def::Section { file, .. } | resolve::Def::Common { file, .. } => {
                        resolved.files[file as usize].name.as_str()
                    }
                    _ => "",
                };
                Some((
                    rva,
                    format!(
                        "{rva:08x}  {}  {file}",
                        String::from_utf8_lossy(global.name)
                    ),
                ))
            })
            .collect();
        lines.sort();
        let text: String = lines.into_iter().map(|(_, l)| l + "\n").collect();
        if let Err(e) = std::fs::write(&map_path, text) {
            out.diagnostics.push(LinkError {
                code: 1104,
                message: format!("cannot open file '{}': {e}", map_path.display()),
                warning: false,
            });
        }
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
    if let Some(t) = &layout.exports
        && !c.no_implib
    {
        let lib_path = c
            .implib
            .clone()
            .unwrap_or_else(|| path.with_extension("lib"));
        let machine = c.machine.unwrap_or(crate::config::Machine::X64);
        let mut hints = vec![0u16; t.directory.exports.len()];
        for (k, (_, i)) in t.directory.names.iter().enumerate() {
            hints[*i] = u16::try_from(k).unwrap_or(0);
        }
        let lib = implib::write(
            &t.dll_name,
            machine.number(),
            machine.is_64(),
            &t.directory.exports,
            &hints,
            crate::write::timestamp(c),
        );
        if let Err(e) = std::fs::write(&lib_path, lib) {
            out.diagnostics.push(LinkError {
                code: 1104,
                message: format!("cannot open file '{}': {e}", lib_path.display()),
                warning: false,
            });
            return out;
        }
        lap("import library", &mut out);
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
    let dll_name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let directory = exports::plan(list, &dll_name, &forwarders);
    Ok(Some(exports::ExportTable {
        directory,
        forwarders,
        targets,
        dll_name,
    }))
}
