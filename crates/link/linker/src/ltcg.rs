//! Link-time code generation (`/LTCG`, for `/GL` objects), as link.exe does
//! it (docs/turbolink-c2.md), in three steps:
//!
//! 1. The back end (`c2.dll`) opens each `/GL` object and reports its
//!    publics and directives. turbolink resolves the link once with a
//!    stand-in object for each (its definitions and references, its
//!    directives), so the libraries load as they will.
//! 2. The roots: the IL definitions something outside the IL refers to
//!    (native objects and library members, the entry point, exports,
//!    `/INCLUDE`s). The back end generates code for the program from them.
//! 3. The generated object takes the `/GL` objects' place on the command
//!    line, and the link goes on as an ordinary one.
//!
//! A first version: `/GL` objects on the command line (not yet in
//! libraries), the back end's default switches for an EXE or a DLL, no
//! incremental LTCG.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::resolve::{FileKind, InputKind, LinkError};

/// Where the back end is: `TURBOLINK_C2`, else `c2.dll` beside the
/// `link.exe` (or `cl.exe`) found on `PATH`, else on `PATH` itself.
pub fn find_backend() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TURBOLINK_C2") {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let c2 = dir.join("c2.dll");
        (c2.is_file() && (dir.join("link.exe").is_file() || dir.join("cl.exe").is_file()))
            .then_some(c2)
    })
}

/// The command line with its `/GL` objects replaced by the code the back
/// end generates for them (`None`: there are none). The generated object
/// is written to `temp`.
pub fn prepare(config: &Config, temp: &Path) -> Result<Option<Config>, Vec<LinkError>> {
    let err = |m: String| vec![LinkError::error(1257, m)];
    let mut il: Vec<(usize, PathBuf, u32)> = Vec::new();
    for (i, input) in config.inputs.iter().enumerate() {
        let path = PathBuf::from(input);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if crate::resolve::identify(&bytes) == InputKind::Ltcg {
            let size = u32::try_from(bytes.len()).map_err(|_| err(format!("{input}: too big")))?;
            il.push((i, path, size));
        }
    }
    if il.is_empty() {
        return Ok(None);
    }
    let c2 = find_backend().ok_or_else(|| {
        err("link-time code generation needs the compiler's back end, c2.dll: not found (set TURBOLINK_C2, or run from a Visual C++ developer prompt)".into())
    })?;
    let mut backend = crate::c2::Backend::load(&c2).map_err(err)?;

    // 1. Each object, opened, and a stand-in for it.
    std::fs::create_dir_all(temp).map_err(|e| err(format!("{}: {e}", temp.display())))?;
    let mut objects = Vec::new();
    for (_, path, size) in &il {
        objects.push(backend.open(path, *size).map_err(err)?);
    }
    let mut first = config.clone();
    let mut stand_ins = Vec::new();
    for (k, ((i, _, _), object)) in il.iter().zip(&objects).enumerate() {
        let p = temp.join(format!("ltcg-stand-in-{k}.obj"));
        std::fs::write(&p, stand_in(object)).map_err(|e| err(format!("{}: {e}", p.display())))?;
        first.inputs[*i] = p.display().to_string();
        stand_ins.push(p.display().to_string());
    }
    let mut diagnostics = Vec::new();
    let Some(resolved) = crate::resolve::resolve(first, &mut diagnostics) else {
        return Err(diagnostics);
    };

    // 2. The roots: what refers to the IL from outside it.
    let mut wanted: turbonop_hash::Set<&[u8]> = turbonop_hash::Set::default();
    for file in &resolved.files {
        if stand_ins.contains(&file.name) {
            continue;
        }
        if let FileKind::Object { object, .. } = &file.kind {
            for s in &object.symbols {
                if matches!(
                    s.kind,
                    crate::object::SymbolKind::Undefined | crate::object::SymbolKind::Weak { .. }
                ) {
                    wanted.insert(s.name);
                }
            }
        }
    }
    if let Some(g) = resolved.entry {
        wanted.insert(resolved.globals[g as usize].name);
    }
    let named: Vec<String> = config
        .exports
        .iter()
        .map(|e| e.internal.clone().unwrap_or_else(|| e.name.clone()))
        .chain(config.includes.iter().cloned())
        .collect();
    for n in &named {
        wanted.insert(n.as_bytes());
    }
    let mut roots = Vec::new();
    let mut seen = turbonop_hash::Set::default();
    for p in objects.iter().flat_map(|o| &o.publics) {
        if matches!(p.kind, 2 | 4) && wanted.contains(p.name.as_slice()) && seen.insert(p.handle) {
            roots.push(p.handle);
        }
    }
    if roots.is_empty() {
        return Err(err(
            "link-time code generation: nothing refers to the /GL objects' code".into(),
        ));
    }

    // 3. The code, in the first /GL object's place.
    let switches: &[&str] = if config.dll { &["-GD"] } else { &["-GA"] };
    let mut directives = Vec::new();
    for o in &objects {
        directives.push(b' ');
        directives.extend_from_slice(&o.directives);
    }
    let generated = backend
        .generate(switches, &roots, &directives)
        .map_err(err)?;
    let out = temp.join("ltcg.obj");
    std::fs::write(&out, &generated).map_err(|e| err(format!("{}: {e}", out.display())))?;
    let mut config = config.clone();
    let first_il = il[0].0;
    let replaced: turbonop_hash::Set<usize> = il.iter().map(|(i, _, _)| *i).collect();
    config.inputs = config
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(i, input)| {
            if i == first_il {
                Some(out.display().to_string())
            } else if replaced.contains(&i) {
                None
            } else {
                Some(input.clone())
            }
        })
        .collect();
    Ok(Some(config))
}

/// A COFF object standing in for a `/GL` object during the first
/// resolution: its directives, each definition in a section of its own (a
/// COMDAT as the IL's is), each reference undefined.
pub fn stand_in(object: &crate::c2::IlObject) -> Vec<u8> {
    let defs: Vec<&crate::c2::Public> = object
        .publics
        .iter()
        .filter(|p| matches!(p.kind, 2 | 4))
        .collect();
    let refs: Vec<&crate::c2::Public> = object.publics.iter().filter(|p| p.kind == 1).collect();
    let nsec = 1 + defs.len();
    let mut strings: Vec<u8> = vec![0; 4];
    let name8 = |n: &[u8], strings: &mut Vec<u8>| -> [u8; 8] {
        let mut r = [0u8; 8];
        if n.len() <= 8 {
            r[..n.len()].copy_from_slice(n);
        } else {
            let off = u32::try_from(strings.len()).unwrap_or(0);
            r[4..].copy_from_slice(&off.to_le_bytes());
            strings.extend_from_slice(n);
            strings.push(0);
        }
        r
    };
    let mut symbols = Vec::new();
    let put = |name: [u8; 8], section: i16, class: u8, aux: u8, symbols: &mut Vec<u8>| {
        symbols.extend_from_slice(&name);
        symbols.extend_from_slice(&0u32.to_le_bytes());
        symbols.extend_from_slice(&section.to_le_bytes());
        symbols.extend_from_slice(&0u16.to_le_bytes());
        symbols.push(class);
        symbols.push(aux);
    };
    // The directives' section, then one per definition (its section symbol
    // and its definition).
    let mut headers = Vec::new();
    let headers_size = 20 + 40 * nsec;
    let header = |name: &[u8; 8], size: u32, at: u32, chars: u32, headers: &mut Vec<u8>| {
        headers.extend_from_slice(name);
        for v in [0, 0, size, at, 0, 0] {
            headers.extend_from_slice(&u32::to_le_bytes(v));
        }
        headers.extend_from_slice(&[0; 4]);
        headers.extend_from_slice(&chars.to_le_bytes());
    };
    header(
        b".drectve",
        u32::try_from(object.directives.len()).unwrap_or(0),
        u32::try_from(headers_size).unwrap_or(0),
        0x0010_0A00,
        &mut headers,
    );
    for (k, d) in defs.iter().enumerate() {
        let section = i16::try_from(k + 2).unwrap_or(i16::MAX);
        let chars = if d.comdat.is_some() {
            0x6050_1020
        } else {
            0x6050_0020
        };
        header(b".text$mn", 0, 0, chars, &mut headers);
        if let Some(selection) = d.comdat {
            put(
                name8(b".text$mn", &mut strings),
                section,
                3,
                1,
                &mut symbols,
            );
            let mut aux = [0u8; 18];
            aux[14] = if selection == 0 { 2 } else { selection };
            symbols.extend_from_slice(&aux);
        }
        put(name8(&d.name, &mut strings), section, 2, 0, &mut symbols);
    }
    for r in &refs {
        put(name8(&r.name, &mut strings), 0, 2, 0, &mut symbols);
    }
    let total = u32::try_from(strings.len()).unwrap_or(0);
    strings[..4].copy_from_slice(&total.to_le_bytes());
    let mut o = Vec::new();
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&u16::try_from(nsec).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    let symbols_at = headers_size + object.directives.len();
    o.extend_from_slice(&u32::try_from(symbols_at).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&u32::try_from(symbols.len() / 18).unwrap_or(0).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&headers);
    o.extend_from_slice(&object.directives);
    o.extend_from_slice(&symbols);
    o.extend_from_slice(&strings);
    o
}
