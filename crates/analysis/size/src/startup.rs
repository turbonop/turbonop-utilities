//! What runs when an image is loaded, before its own code's `main` (or, of
//! a library, what runs when it is loaded): in the order it runs, phase by
//! phase, each function the loader or the runtime calls, and (from the
//! cross-reference graph, see [`crate::xref`]) what each calls.
//!
//! - PE: the TLS callbacks (the loader's, before the entry point), the
//!   entry point, then what the CRT calls from it: the runtime checks'
//!   initializers (`.rtc$I*`), the C initializers (`.CRT$XI*`, by
//!   `_initterm_e`), and the C++ dynamic initializers (`.CRT$XC*`, by
//!   `_initterm`), each group's entries in order; then `main`, `wmain`,
//!   `WinMain`, `wWinMain` (or a DLL's `DllMain`).
//! - ELF: `.preinit_array` (a program's), `DT_INIT`, `.ctors` (last
//!   first), `.init_array`, the entry point, then `main`.
//! - Mach-O: the initializers dyld calls (`__mod_init_func` pointers, or
//!   `__init_offsets` offsets), then the entry point (`LC_MAIN`'s is
//!   `main` itself).
//!
//! The groups' bounds come from the debug information (a PE image's COFF
//! groups) or the sections; their entries are read from the file, through
//! the loader's relocations where it writes them.

use hexcavator_image::Image;
use hexcavator_xref::Edge;

use crate::Model;
use crate::xref::Xref;

/// A phase of loading: what runs, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase {
    /// Its name: a group's or section's (`.CRT$XCU`, `.init_array`), or
    /// what it is (`TLS callbacks`, `the entry point`).
    pub name: String,
    /// Who calls it, and when.
    pub detail: &'static str,
    pub entries: Vec<Entry>,
}

/// A function a phase calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its address (the image crate's).
    pub addr: u64,
    /// Its node in the graph, if one is there.
    pub node: Option<u32>,
    /// Where its address is (an initializer list's slot).
    pub slot: Option<u64>,
}

/// What runs when an image is loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Startup {
    /// Whether the image is a library (what runs is when it is loaded,
    /// not before a `main`).
    pub library: bool,
    /// The phases, in the order they run.
    pub phases: Vec<Phase>,
    /// The program's own code's first function (`main`, `WinMain`,
    /// `DllMain`, ...), if one is found.
    pub main: Option<u32>,
}

/// The most entries read of one list.
const MAX_ENTRIES: u64 = 1 << 16;

/// What runs when `image` (whose graph is `x`) is loaded.
pub fn startup(image: &Image<'_>, x: &Xref, model: &Model) -> Startup {
    let mut s = if let Some(pe) = image.pe() {
        pe_startup(image, pe, x, model)
    } else if let Some(elf) = image.elf() {
        elf_startup(image, elf, x)
    } else if let Some(m) = image.macho() {
        macho_startup(image, m, x)
    } else if let Some(m) = image.wasm() {
        wasm_startup(m, x)
    } else {
        Startup::default()
    };
    s.phases.retain(|p| !p.entries.is_empty());
    // (A Mach-O image's entry point is main itself: one phase.)
    if let Some(m) = s.main
        && s.phases
            .last()
            .is_some_and(|p| p.entries.iter().any(|e| e.node == Some(m)))
    {
        s.main = None;
    }
    s
}

fn entry(x: &Xref, addr: u64, slot: Option<u64>) -> Entry {
    Entry {
        addr,
        node: x
            .graph
            .node_starting_at(addr)
            .or_else(|| x.graph.node_at(addr)),
        slot,
    }
}

use crate::loader_lists::words;

/// The code node named one of `names` (by its name without parameters),
/// a global one first.
fn named(x: &Xref, names: &[&str]) -> Option<u32> {
    let base = |n: &str| -> String {
        let n = n.split('(').next().unwrap_or(n).trim();
        // (Mach-O's and 32-bit Windows' leading underscore.)
        n.strip_prefix('_')
            .filter(|_| !n.starts_with("__"))
            .unwrap_or(n)
            .to_string()
    };
    names.iter().find_map(|want| {
        x.ids().find(|&i| {
            x.graph.nodes()[i as usize].code
                && (base(&x.names[i as usize]) == *want
                    || x.aliases[i as usize].iter().any(|a| base(a) == *want))
        })
    })
}

fn pe_startup(
    image: &Image<'_>,
    pe: &hexcavator_pe::PeFile<'_>,
    x: &Xref,
    model: &Model,
) -> Startup {
    let base = image.image_base();
    let wide = pe
        .optional_header()
        .is_some_and(hexcavator_pe::OptionalHeader::is_64);
    let library = pe.is_dll();
    let mut phases = Vec::new();
    let tls: Vec<Entry> = pe
        .tls_callbacks()
        .take(MAX_ENTRIES as usize)
        .filter_map(|va| va.get().checked_sub(base))
        .map(|rva| entry(x, rva, None))
        .collect();
    phases.push(Phase {
        name: "TLS callbacks".into(),
        detail: "the loader calls each before the entry point (and again as each thread starts and ends)",
        entries: tls,
    });
    let start = pe
        .optional_header()
        .map_or(0, |h| u64::from(h.address_of_entry_point()));
    if start != 0 {
        phases.push(Phase {
            name: "the entry point".into(),
            detail: if library {
                "the loader calls it when the DLL is loaded (DLL_PROCESS_ATTACH), and as threads start and end"
            } else {
                "the loader calls it once the program and its DLLs are loaded"
            },
            entries: vec![entry(x, start, None)],
        });
    }
    // The CRT's lists, as it calls them: runtime checks', C, then C++.
    let mut groups: Vec<&crate::Group> = model
        .groups
        .iter()
        .filter(|g| {
            g.name.starts_with(".rtc$I")
                || g.name.starts_with(".CRT$XI")
                || g.name.starts_with(".CRT$XC")
        })
        .collect();
    let rank = |n: &str| {
        if n.starts_with(".rtc$") {
            0
        } else if n.starts_with(".CRT$XI") {
            1
        } else {
            2
        }
    };
    groups.sort_by(|a, b| (rank(&a.name), &a.name).cmp(&(rank(&b.name), &b.name)));
    for g in groups {
        let entries: Vec<Entry> = words(
            image,
            g.addr,
            g.addr.saturating_add(g.size.vm.max(g.size.file)),
            wide,
            &[],
        )
        .into_iter()
        .filter(|&(_, v)| v != 0)
        .filter_map(|(slot, va)| Some(entry(x, va.checked_sub(base)?, Some(slot))))
        .collect();
        phases.push(Phase {
            name: g.name.clone(),
            detail: match rank(&g.name) {
                0 => "the runtime checks' initializers: the CRT calls them from the entry point, first",
                1 => "C initializers: the CRT calls them from the entry point (_initterm_e), before C++'s",
                _ => "C++ dynamic initializers (global objects' constructors): the CRT calls them from the entry point (_initterm)",
            },
            entries,
        });
    }
    let main = if library {
        named(x, &["DllMain"])
    } else {
        named(x, &["main", "wmain", "WinMain", "wWinMain"])
    };
    Startup {
        library,
        phases,
        main,
    }
}

fn elf_startup(image: &Image<'_>, elf: &hexcavator_elf::ElfFile<'_>, x: &Xref) -> Startup {
    use crate::loader_lists::{List, When};
    use hexcavator_elf::consts::*;
    let h = elf.header();
    let library = h.kind == ET_DYN && !elf.segments().any(|s| s.kind == PT_INTERP);
    // (Thumb functions' addresses have their low bit set.)
    let code = |v: u64| if h.machine == EM_ARM { v & !1 } else { v };
    let lists = crate::loader_lists::lists(image);
    let of = |pick: &dyn Fn(&List) -> bool| -> Vec<Entry> {
        lists
            .iter()
            .filter(|l| pick(l))
            .flat_map(|l| l.entries.iter().map(|&(slot, v)| entry(x, v, slot)))
            .collect()
    };
    let mut phases = Vec::new();
    if !library {
        phases.push(Phase {
            name: ".preinit_array".into(),
            detail: "the dynamic loader calls these first, before any library's initializers (a program's only)",
            entries: of(&|l| l.when == When::PreInit),
        });
    }
    phases.push(Phase {
        name: "DT_INIT".into(),
        detail: "the loader calls it (_init), before .init_array",
        entries: of(&|l| l.when == When::Init && l.section.is_none()),
    });
    phases.push(Phase {
        name: ".ctors".into(),
        detail: "older constructors' list: _init's __do_global_ctors_aux calls them, last first",
        entries: of(&|l| l.name == ".ctors"),
    });
    phases.push(Phase {
        name: ".init_array".into(),
        detail: if library {
            "the loader calls these when the library is loaded, in order"
        } else {
            "the loader (or, in a static program, __libc_start_main) calls these in order, before main"
        },
        entries: of(&|l| l.when == When::Init && l.section.is_some() && l.name != ".ctors"),
    });
    if !library && h.entry != 0 {
        phases.push(Phase {
            name: "the entry point".into(),
            detail: "the loader jumps to it (_start) once everything is loaded; it calls __libc_start_main, which calls main",
            entries: vec![entry(x, code(h.entry), None)],
        });
    }
    let main = if library { None } else { named(x, &["main"]) };
    Startup {
        library,
        phases,
        main,
    }
}

fn macho_startup(image: &Image<'_>, m: &hexcavator_macho::MachO<'_>, x: &Xref) -> Startup {
    use crate::loader_lists::When;
    const MH_EXECUTE: u32 = 2;
    let library = m.header().filetype != MH_EXECUTE;
    let mut entries = Vec::new();
    let mut names = Vec::new();
    for l in crate::loader_lists::lists(image)
        .into_iter()
        .filter(|l| l.when == When::Init)
    {
        entries.extend(l.entries.iter().map(|&(slot, v)| entry(x, v, slot)));
        names.push(l.name);
    }
    let mut phases = vec![Phase {
        name: if names.is_empty() {
            "__mod_init_func".into()
        } else {
            names.join(", ")
        },
        detail: if library {
            "dyld calls these when the library is loaded, in order"
        } else {
            "dyld calls these in order, once everything is loaded, before the entry point"
        },
        entries,
    }];
    if !library && let Some(e) = m.entry() {
        phases.push(Phase {
            name: "the entry point".into(),
            detail: "dyld calls it last (LC_MAIN's is main itself)",
            entries: vec![entry(x, e, None)],
        });
    }
    let main = if library { None } else { named(x, &["main"]) };
    Startup {
        library,
        phases,
        main,
    }
}

/// A WebAssembly module's: its start function (the runtime calls it when
/// it instantiates the module), then a WASI reactor's `_initialize` (the
/// host calls it before any other export) or wasm-ld's constructors
/// (`__wasm_call_ctors`, which `_start` and `_initialize` call), then a
/// WASI command's `_start`. A module without `_start` is a library.
fn wasm_startup(m: &hexcavator_wasm::Module<'_>, x: &Xref) -> Startup {
    let export = |name: &[u8]| {
        m.exports()
            .into_iter()
            .find(|e| e.kind == 0 && e.name == name)
            .map(|e| e.index)
    };
    let at = |index: u32| function_address(m, index);
    let start = export(b"_start");
    let mut phases = Vec::new();
    if let Some(a) = m.start_function().and_then(at) {
        phases.push(Phase {
            name: "the start function".into(),
            detail: "the runtime calls it when it instantiates the module, before the host can call an export",
            entries: vec![entry(x, a, None)],
        });
    }
    if let Some(a) = export(b"_initialize").and_then(at) {
        phases.push(Phase {
            name: "_initialize".into(),
            detail: "a WASI reactor's: the host calls it before any other export; it calls __wasm_call_ctors",
            entries: vec![entry(x, a, None)],
        });
    } else if let Some(n) = named(x, &["__wasm_call_ctors"]) {
        let node = &x.graph.nodes()[n as usize];
        phases.push(Phase {
            name: "__wasm_call_ctors".into(),
            detail: "wasm-ld's: global objects' constructors and C initializers, in order; _start calls it before main",
            entries: vec![Entry {
                addr: node.start,
                node: Some(n),
                slot: None,
            }],
        });
    }
    if let Some(a) = start.and_then(at) {
        phases.push(Phase {
            name: "the entry point".into(),
            detail: "a WASI command's _start: the host calls it; it calls __wasm_call_ctors, then main",
            entries: vec![entry(x, a, None)],
        });
    }
    let library = start.is_none();
    let main = if library {
        None
    } else {
        named(
            x,
            &["main", "__main_void", "__main_argc_argv", "__original_main"],
        )
    };
    Startup {
        library,
        phases,
        main,
    }
}

/// The address of a WebAssembly module's function `index`, as its graph
/// has it: its body's, in the code section.
fn function_address(m: &hexcavator_wasm::Module<'_>, index: u32) -> Option<u64> {
    let code = m
        .sections()
        .iter()
        .find(|s| s.id == hexcavator_wasm::SECTION_CODE)?;
    let f = m.functions().into_iter().find(|f| f.index == index)?;
    m.body(&f)?.0.checked_sub(code.contents_offset)
}

/// The code `node` calls or jumps to (a tail call, a jump table), each
/// once, in the order its code first does (its pieces', disassembled
/// again for their call sites; any the scan doesn't place, after).
pub fn callees(image: &Image<'_>, x: &Xref, node: u32) -> Vec<(u32, Edge)> {
    let nodes = x.graph.nodes();
    let mut set: Vec<(u32, Edge)> = Vec::new();
    // (A function split in pieces: all of its pieces' references.)
    let own = x.graph.pieces(x.graph.origin(node));
    for piece in own.clone() {
        for &(to, edge) in x.graph.references(piece) {
            if !matches!(edge, Edge::Call | Edge::Jump) || own.contains(&to) {
                continue;
            }
            if nodes.get(to as usize).is_some_and(|n| n.code) && !set.iter().any(|&(t, _)| t == to)
            {
                set.push((to, edge));
            }
        }
    }
    // In call-site order.
    let mut refs = Vec::new();
    for piece in own {
        let Some(n) = nodes.get(piece as usize) else {
            continue;
        };
        if let Some(code) = image.bytes_at(n.start, n.end.saturating_sub(n.start)) {
            hexcavator_xref::scan_image(x.arch, code, n.start, image.pe().map(|_| 0), &mut refs);
        }
    }
    let mut out = Vec::with_capacity(set.len());
    for r in &refs {
        let Some(t) = x
            .graph
            .node_starting_at(r.to)
            .or_else(|| x.graph.node_at(r.to))
        else {
            continue;
        };
        if let Some(i) = set.iter().position(|&(s, _)| s == t) {
            out.push(set.swap_remove(i));
        }
    }
    // (What the scan doesn't place, by the graph's order.)
    set.sort_unstable_by_key(|&(t, _)| t);
    out.extend(set);
    out
}
