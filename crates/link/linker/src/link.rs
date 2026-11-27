//! A link, from its configuration to the image written.

use std::path::PathBuf;
use std::time::Instant;

use crate::config::Config;
use crate::layout;
use crate::resolve::{self, LinkError};
use crate::write::{Image, base_relocation_blocks};

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
pub fn link(config: Config) -> Outcome {
    let mut out = Outcome::default();
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
    let mut layout = layout::layout(&resolved, &roots);
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
    out.written = true;
    out
}
