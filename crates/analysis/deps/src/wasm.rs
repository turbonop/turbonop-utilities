//! A WebAssembly module's dependencies: the modules its imports are from
//! (each with what it imports of it), and the shared libraries its
//! `dylink.0` section says it needs (Emscripten's dynamic linking).
//!
//! An import's module is the host's (its runtime's, or JavaScript's: `env`,
//! `wasi_snapshot_preview1`) unless a module of its name is beside the one
//! importing it (`name` or `name.wasm`), as a component's or a bundler's
//! are; a needed library is looked for there and in the directories given.

use std::path::PathBuf;

use crate::search::Reject;
use crate::{Ctx, DepKind, Found, Import, ImportStatus, Resolved, Status, Wanted};

fn parsed<'a>(ctx: &'a Ctx<'_>, m: usize) -> Option<turbonop_wasm::Module<'a>> {
    let loaded = ctx.loaded.get(m)?.as_ref()?;
    turbonop_wasm::Module::parse(loaded.data()).ok()
}

/// Module `m`'s imports' modules (each once, in the order first imported
/// from, with what it imports), then its needed libraries.
pub(crate) fn wanted(ctx: &mut Ctx<'_>, m: usize) -> Vec<Wanted> {
    let Some(module) = parsed(ctx, m) else {
        return Vec::new();
    };
    let mut out: Vec<Wanted> = Vec::new();
    for i in module.imports() {
        let name = String::from_utf8_lossy(i.module).into_owned();
        let import = Import {
            name: Some(String::from_utf8_lossy(i.field).into_owned()),
            ordinal: None,
            version: None,
            weak: false,
            status: ImportStatus::Unchecked,
        };
        match out
            .iter_mut()
            .find(|w| w.kind == DepKind::Import && w.name == name)
        {
            Some(w) => w.imports.push(import),
            None => out.push(Wanted {
                name,
                kind: DepKind::Import,
                imports: vec![import],
            }),
        }
    }
    out.extend(module.needed().into_iter().map(|n| Wanted {
        name: String::from_utf8_lossy(n).into_owned(),
        kind: DepKind::Needed,
        imports: Vec::new(),
    }));
    out
}

/// Where `name` is, as `loader` names it: beside it (`name`, and for an
/// import's module `name.wasm`), then in the directories given; else the
/// host's (an import's module), or missing (a needed library).
pub(crate) fn resolve(ctx: &mut Ctx<'_>, loader: usize, name: &str, kind: DepKind) -> Resolved {
    // (A module's name is any string: a path's only if it's a file's.)
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if file.is_empty() || name.contains('\0') || file == "." || file == ".." {
        return Resolved::Other(Status::Missing { tried: Vec::new() });
    }
    let dir = ctx.graph.modules[loader]
        .status
        .path()
        .and_then(std::path::Path::parent)
        .map_or_else(|| ctx.root_dir.clone(), std::path::Path::to_path_buf);
    let mut names = vec![file.to_string()];
    if kind == DepKind::Import && !file.ends_with(".wasm") {
        names.push(format!("{file}.wasm"));
    }
    let mut candidates: Vec<(PathBuf, Found)> =
        names.iter().map(|n| (dir.join(n), Found::AppDir)).collect();
    for d in &ctx.options.dirs {
        candidates.extend(names.iter().map(|n| (d.join(n), Found::SearchDir)));
    }
    let mut tried = Vec::new();
    for (path, how) in candidates {
        if tried.contains(&path) {
            continue;
        }
        match ctx.search.open(&path) {
            Ok(loaded) => return Resolved::File { path, how, loaded },
            Err(Reject::Absent) => {}
            // (A file of the name that isn't a module isn't the import's.)
            Err(Reject::WrongArch(_) | Reject::Unreadable(_)) if kind == DepKind::Import => {}
            Err(Reject::WrongArch(arch)) => {
                return Resolved::Other(Status::WrongArch { path, arch });
            }
            Err(Reject::Unreadable(why)) => {
                return Resolved::Other(Status::Unreadable { path, why });
            }
        }
        tried.push(path);
    }
    Resolved::Other(if kind == DepKind::Import {
        Status::Host
    } else {
        Status::Missing { tried }
    })
}
