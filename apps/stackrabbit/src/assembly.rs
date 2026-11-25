//! A function's instructions, annotated with the time spent in each: its
//! code disassembled (from the image file the trace names, if it's still
//! there and is the image the trace saw) with Hexcavator's decoders, and
//! the trace's CPU samples counted by instruction: the samples whose
//! instruction pointer is at it (its own time), and, at a call, the samples
//! in what it called (by the return addresses on their stacks), and which
//! functions those were (an indirect call's targets, as sampled). Then its
//! basic blocks, each with its time, and the loops among them (a branch to
//! an earlier block), each with its time and source line; and each
//! instruction's source line. And a source file's loops: those of its
//! sampled functions.

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};
use turbonop_image::Image;
use turbonop_mmap::Mmap;
use turbonop_trace::{ImageIx, Trace};
use turbonop_trace_analysis::{PdbSymbolizer, Resolver, Symbolizer};

/// The function `id` names (`module|<offset>`, as the Functions tab gives
/// it) annotated with the samples in `time` (clock ticks; all, if none).
pub fn assembly(
    trace: &Trace,
    trace_path: &Path,
    resolver: &Resolver,
    symbols: &PdbSymbolizer,
    id: &str,
    time: Option<(i64, i64)>,
) -> Result<Value, String> {
    let bad = || format!("fn: not a function's ID: {id}");
    let (module, what) = id.rsplit_once('|').ok_or_else(bad)?;
    let start = u64::from_str_radix(what, 16).map_err(|_| bad())?;
    let images: Vec<ImageIx> = (0..trace.images.len())
        .filter(|&i| trace.images[i].file_name() == module)
        .map(|i| ImageIx(u32::try_from(i).expect("fewer than 4G images")))
        .collect();
    if images.is_empty() {
        return Err(format!("fn: no module {module} in the trace"));
    }
    // Its extent and name, from the first of its images whose symbols
    // have a function there (a trace can have several builds of a module,
    // in different processes).
    let mut any_symbols = false;
    let (table_image, table, end, name) = images
        .iter()
        .find_map(|&i| {
            let t = symbols.table(trace, i)?;
            any_symbols = true;
            let k = t.iter().position(|f| f.0 == start)?;
            let (_, end, name) = t.iter().nth(k)?;
            let name = name.to_string();
            Some((i, t, end, name))
        })
        .ok_or_else(|| {
            if any_symbols {
                format!("{module}: no function at {start:#x}")
            } else {
                format!("{module}: no symbols, so no function's extent")
            }
        })?;
    // (It's an image of that build whose file is needed.)
    let images: Vec<ImageIx> = images
        .iter()
        .copied()
        .filter(|&i| {
            let (a, b) = (&trace.images[i.index()], &trace.images[table_image.index()]);
            a.size == b.size && a.time_date_stamp == b.time_date_stamp
        })
        .collect();
    // Its bytes, from an image file that is the image the trace saw (at
    // the path the trace names, as a DOS path).
    let volumes = crate::reading::Volumes::of(trace, trace_path);
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let mut tried = Vec::new();
    let mut found = None;
    for &i in &images {
        let img = &trace.images[i.index()];
        let file = system_root(&volumes.dos(&img.path), &windows);
        if tried.contains(&file) {
            continue;
        }
        tried.push(file.clone());
        let Ok(map) = Mmap::open_named(Path::new(&file)) else {
            continue;
        };
        let same = Image::parse(map.as_slice()).is_ok_and(|image| {
            image
                .pe()
                .and_then(|pe| pe.optional_header())
                .is_none_or(|h| {
                    (img.size == 0 || u64::from(h.size_of_image()) == img.size)
                        && (img.time_date_stamp == 0
                            || image.pe().is_some_and(|pe| {
                                pe.file_header().time_date_stamp.get() == img.time_date_stamp
                            }))
                })
        });
        if same {
            found = Some(map);
            break;
        }
    }
    let map = found.ok_or_else(|| {
        format!(
            "{module}: its image file isn't here, or isn't the one the trace saw ({})",
            tried.join(", ")
        )
    })?;
    let image = Image::parse(map.as_slice()).map_err(|e| format!("{module}: {e}"))?;
    let code = image
        .bytes_at(start, end - start)
        .ok_or_else(|| format!("{module}: the function's bytes aren't in the file"))?;
    let arch = image.arch();
    // The names of what it calls and jumps to, for the listing.
    let (insns, _) = turbonop_code::sweep(arch, code, start);
    let mut names: Vec<(u64, String)> = insns
        .iter()
        .filter_map(|i| i.target)
        .filter_map(|t| {
            table
                .function(t)
                .filter(|f| f.0 == t)
                .map(|f| (t, f.1.to_string()))
        })
        .collect();
    names.sort();
    names.dedup();
    let listing = turbonop_size::disasm::decode(
        &turbonop_size::disasm::Function {
            name: name.clone(),
            isa: arch,
            thumb: false,
            data: None,
            address: start,
            code,
            symbols: names,
            riscv_arch: None,
            big_endian: false,
            mips_flags: 0,
        },
        turbonop_size::disasm::Labels::Blocks,
    );
    let Counted {
        own,
        under: callees,
        targets,
        total,
    } = count(trace, resolver, symbols, &images, start, end, time);
    let at = |offset: u64| {
        listing
            .lines
            .partition_point(|l| l.address <= offset)
            .checked_sub(1)
    };
    let mut line_own = vec![0u64; listing.lines.len()];
    let mut line_callees = vec![0u64; listing.lines.len()];
    for (&offset, &n) in &own {
        if let Some(i) = at(offset) {
            line_own[i] += n;
        }
    }
    for (&offset, &n) in &callees {
        // (A return address is just past its call.)
        if let Some(i) = at(offset.saturating_sub(1)) {
            line_callees[i] += n;
        }
    }
    // Each call's targets, as sampled: the functions under it, heaviest
    // first.
    let mut line_targets: Vec<Vec<(u64, String)>> = vec![Vec::new(); listing.lines.len()];
    for (&(offset, callee, callee_start), &n) in &targets {
        if let Some(i) = at(offset.saturating_sub(1)) {
            let label = match callee {
                None => "? (code in no module: JIT-compiled, say)".to_string(),
                Some(callee) => {
                    let file = trace.images[callee.index()].file_name();
                    match symbols.function(trace, callee, callee_start) {
                        _ if callee_start == NO_FUNCTION => format!("{file} (no symbols)"),
                        Some((s, f)) if s == callee_start => format!("{file}!{f}"),
                        _ => format!("{file}+{callee_start:#x}"),
                    }
                }
            };
            match line_targets[i].iter_mut().find(|t| t.1 == label) {
                Some(t) => t.0 += n,
                None => line_targets[i].push((n, label)),
            }
        }
    }
    for t in &mut line_targets {
        t.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    let block_of: Vec<usize> = {
        let mut b = vec![0; listing.lines.len()];
        for (k, blk) in listing.blocks.iter().enumerate() {
            for x in b.iter_mut().skip(blk.first).take(blk.count) {
                *x = k;
            }
        }
        b
    };
    // The hottest instructions, by their own samples.
    let mut hot: Vec<usize> = (0..listing.lines.len())
        .filter(|&i| line_own[i] > 0)
        .collect();
    hot.sort_by(|&a, &b| line_own[b].cmp(&line_own[a]).then(a.cmp(&b)));
    hot.truncate(3);
    let shown = |offset: u64| image.shown_address(offset);
    let own_total: u64 = line_own.iter().sum();
    // Each instruction's source line (by file, in `files`).
    let mut files: Vec<String> = Vec::new();
    let sources: Vec<Option<(usize, u32)>> = listing
        .lines
        .iter()
        .map(|l| {
            let (file, line) = symbols.line(trace, table_image, l.address)?;
            let f = match files.iter().position(|f| *f == file) {
                Some(f) => f,
                None => {
                    files.push(file);
                    files.len() - 1
                }
            };
            Some((f, line))
        })
        .collect();
    let loops = loops(&listing, &line_own, &line_callees, &sources);
    Ok(json!({
        "name": name,
        "module": module,
        "offset": start,
        "size": end - start,
        "samples": total,
        "own": own_total,
        "hottest": hot,
        "lines": listing.lines.iter().enumerate().map(|(i, l)| json!({
            "address": format!("{:x}", shown(l.address)),
            "offset": l.address,
            "bytes": l.bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            "text": l.text,
            "label": l.label.map(|n| listing.label_name(n)),
            "block": block_of[i],
            "own": line_own[i],
            "callees": line_callees[i],
            "targets": line_targets[i].iter().map(|(n, f)| json!({"function": f, "samples": n})).collect::<Vec<_>>(),
            "file": sources[i].map(|s| s.0),
            "line": sources[i].map(|s| s.1),
        })).collect::<Vec<_>>(),
        "files": files,
        "loops": loops,
        "blocks": listing.blocks.iter().enumerate().map(|(k, b)| {
            let own: u64 = line_own[b.first..b.first + b.count].iter().sum();
            json!({
                "index": k,
                "first": b.first,
                "count": b.count,
                "own": own,
                "successors": b.successors.iter().map(|(s, _)| s).collect::<Vec<_>>(),
                // (A branch to it from it or a later block: a loop's head.)
                "loop": listing.blocks.iter().enumerate()
                    .any(|(j, o)| j >= k && o.successors.iter().any(|(s, _)| *s == k)),
            })
        }).collect::<Vec<_>>(),
        "undecoded": listing.rest.as_ref().map(|r| r.0),
    }))
}

/// A listing's loops: a block branched to from it or a later block is a
/// loop's head, and the loop runs to the last block branching back to it
/// (as laid out: a loop the compiler laid out elsewhere is longer here).
/// Each loop's time: its instructions' own samples (`own`, by line), and
/// with what they call (`under`); and the source line (`sources`, by
/// line) of its first instruction that has one.
pub fn loops(
    listing: &turbonop_size::disasm::Listing,
    own: &[u64],
    under: &[u64],
    sources: &[Option<(usize, u32)>],
) -> Vec<Value> {
    let blocks = &listing.blocks;
    (0..blocks.len())
        .filter_map(|k| {
            let last = (k..blocks.len())
                .filter(|&j| blocks[j].successors.iter().any(|(s, _)| *s == k))
                .max()?;
            let first = blocks[k].first;
            let end = blocks[last].first + blocks[last].count;
            let mine: u64 = own[first..end].iter().sum();
            let called: u64 = under[first..end].iter().sum();
            let source = sources[first..end].iter().flatten().next();
            Some(json!({
                "head": k,
                "last": last,
                "first": first,
                "end": end,
                "own": mine,
                "samples": mine + called,
                "file": source.map(|s| s.0),
                "line": source.map(|s| s.1),
            }))
        })
        .collect()
}

/// The loops in the source file `file` (as the trace's symbols name it):
/// those of the `limit` functions with the most samples whose first
/// instruction is in it (whose image files are here), each with its
/// function, time and line, in `spec`'s time range. And the functions
/// that couldn't be read, with why.
pub fn file_loops(
    trace: &Trace,
    trace_path: &Path,
    resolver: &Resolver,
    symbols: &PdbSymbolizer,
    file: &str,
    limit: usize,
    spec: &crate::views::TreeSpec,
) -> Value {
    let time = spec
        .time
        .map(|(a, b)| (crate::views::ticks(trace, a), crate::views::ticks(trace, b)));
    let spec = crate::views::TreeSpec {
        time: spec.time,
        ..crate::views::TreeSpec::default()
    };
    let f = crate::views::functions(trace, resolver, symbols, &spec, usize::MAX);
    let total = f["total"].as_u64().unwrap_or(0);
    // Each module's first image with symbols (a DLL is an image in each
    // process: their symbols are the same).
    let mut with_symbols: FxHashMap<&str, Option<ImageIx>> = FxHashMap::default();
    let mut candidates: Vec<(String, String, u64)> = Vec::new();
    for row in f["functions"].as_array().into_iter().flatten() {
        let Some(id) = row["fn"].as_str() else {
            continue;
        };
        let Some((module, what)) = id.rsplit_once('|') else {
            continue;
        };
        let Ok(start) = u64::from_str_radix(what, 16) else {
            continue;
        };
        let image = *with_symbols.entry(module).or_insert_with(|| {
            (0..trace.images.len())
                .filter(|&i| trace.images[i].file_name() == module)
                .map(|i| ImageIx(u32::try_from(i).expect("fewer than 4G images")))
                .find(|&i| symbols.table(trace, i).is_some())
        });
        let in_file = image
            .and_then(|i| symbols.line(trace, i, start))
            .is_some_and(|(f, _)| f == file);
        if in_file {
            candidates.push((
                id.to_string(),
                row["label"].as_str().unwrap_or("").to_string(),
                row["inclusive"].as_u64().unwrap_or(0),
            ));
        }
    }
    candidates.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    candidates.truncate(limit);
    let annotated = turbonop_par::map_range(candidates.len(), 1, |i| {
        assembly(trace, trace_path, resolver, symbols, &candidates[i].0, time)
    });
    let mut loops = Vec::new();
    let mut unread = Vec::new();
    for ((id, label, _), a) in candidates.iter().zip(annotated) {
        let a = match a {
            Ok(a) => a,
            Err(e) => {
                unread.push(json!({"fn": id, "function": label, "error": e}));
                continue;
            }
        };
        let files = a["files"].as_array().cloned().unwrap_or_default();
        for l in a["loops"].as_array().into_iter().flatten() {
            let Some(f) = l["file"].as_u64() else {
                continue;
            };
            if usize::try_from(f)
                .ok()
                .and_then(|f| files.get(f))
                .and_then(Value::as_str)
                != Some(file)
            {
                continue;
            }
            let instructions = l["end"].as_u64().unwrap_or(0) - l["first"].as_u64().unwrap_or(0);
            let samples = l["samples"].as_u64().unwrap_or(0);
            // Loops starting at one line (nested, or one the compiler
            // split): one, the one with the most time, and how many.
            if let Some(same) = loops
                .iter_mut()
                .find(|o: &&mut Value| o["fn"] == id.as_str() && o["line"] == l["line"])
            {
                same["loops"] = json!(same["loops"].as_u64().unwrap_or(1) + 1);
                if samples > same["samples"].as_u64().unwrap_or(0) {
                    same["own"] = l["own"].clone();
                    same["samples"] = json!(samples);
                    same["instructions"] = json!(instructions);
                }
                continue;
            }
            loops.push(json!({
                "fn": id,
                "function": label,
                "line": l["line"],
                "own": l["own"],
                "samples": samples,
                "instructions": instructions,
                "loops": 1,
            }));
        }
    }
    json!({
        "file": file,
        "total": total,
        "ticksPerSecond": trace.clock.ticks_per_second,
        "loops": loops,
        "unread": unread,
    })
}

/// `path` with `\SystemRoot\` (as the kernel's images are named) as the
/// Windows directory `windows`, where the trace's volumes don't say.
pub fn system_root(path: &str, windows: &str) -> String {
    const ROOT: &str = r"\SystemRoot\";
    if path.len() >= ROOT.len()
        && path.is_char_boundary(ROOT.len())
        && path[..ROOT.len()].eq_ignore_ascii_case(ROOT)
    {
        return format!(r"{windows}\{}", &path[ROOT.len()..]);
    }
    path.to_string()
}

/// Events a worker counts at a time.
const CHUNK: usize = 1 << 16;

/// A call target in an image without symbols.
const NO_FUNCTION: u64 = u64::MAX;

/// A function's samples: see [`count`].
#[derive(Default)]
struct Counted {
    /// By offset, the samples at it.
    own: FxHashMap<u64, u64>,
    /// By return address (an offset), the samples under its call.
    under: FxHashMap<u64, u64>,
    /// By return address and the function called (its image, if any, and
    /// start, or [`NO_FUNCTION`]), the samples under the call that were in
    /// it.
    targets: FxHashMap<(u64, Option<ImageIx>, u64), u64>,
    /// The samples with it on their stacks at all.
    total: u64,
}

impl Counted {
    fn add(&mut self, other: Self) {
        for (k, v) in other.own {
            *self.own.entry(k).or_default() += v;
        }
        for (k, v) in other.under {
            *self.under.entry(k).or_default() += v;
        }
        for (k, v) in other.targets {
            *self.targets.entry(k).or_default() += v;
        }
        self.total += other.total;
    }
}

/// The samples in `time` in the function `[start, end)` of `images`: by
/// offset, those at it (instruction pointers) and those under it (return
/// addresses on their stacks, once a sample each), and what each call
/// called; and how many samples had it on their stacks at all. On every
/// core.
fn count(
    trace: &Trace,
    resolver: &Resolver,
    symbols: &PdbSymbolizer,
    images: &[ImageIx],
    start: u64,
    end: u64,
    time: Option<(i64, i64)>,
) -> Counted {
    let s = &trace.samples;
    let (from, to) = match time {
        Some((a, b)) => (
            s.time.partition_point(|&t| t < a),
            s.time.partition_point(|&t| t < b),
        ),
        None => (0, s.len()),
    };
    // Where it is, in each of its images (a DLL is often at one address in
    // every process): most frames are nowhere near it, and need no lookup.
    let mut ranges: Vec<(u64, u64)> = images
        .iter()
        .map(|i| {
            let base = trace.images[i.index()].base;
            (base.saturating_add(start), base.saturating_add(end))
        })
        .collect();
    ranges.sort_unstable();
    ranges.dedup();
    let near = |address: u64| {
        let i = ranges.partition_point(|r| r.0 <= address);
        // (They're one function's extent at each base. Only those starting
        // at or before it can hold it, and of those only the last few:
        // extents at different bases overlap only where two processes'
        // images of the module, at nearby bases, are different builds.)
        ranges[..i].iter().rev().take(4).any(|r| address < r.1)
    };
    let images: FxHashSet<ImageIx> = images.iter().copied().collect();
    // Kernel-mode addresses: the top half of the address space (a 32-bit
    // trace's images all lie below 4 GB).
    let narrow = trace.images.iter().all(|i| i.base < 1 << 32);
    let kernel = |a: u64| {
        if narrow {
            a >= 0x8000_0000
        } else {
            a >= 0xffff_8000_0000_0000
        }
    };
    let chunks = (to - from).div_ceil(CHUNK);
    let parts = turbonop_par::map_range(chunks, 1, |chunk| {
        let mut c = Counted::default();
        let mut seen: Vec<u64> = Vec::new();
        for i in from + chunk * CHUNK..(from + (chunk + 1) * CHUNK).min(to) {
            let process = trace.thread_process(s.thread[i]);
            let weight = u64::from(s.weight[i]);
            let inside = |address: u64| {
                if !near(address) {
                    return None;
                }
                resolver.image(process, address).and_then(|img| {
                    let offset = address - trace.images[img.index()].base;
                    (images.contains(&img) && offset >= start && offset < end).then_some(offset)
                })
            };
            let mut on_stack = false;
            if let Some(o) = inside(s.ip[i]) {
                *c.own.entry(o).or_default() += weight;
                on_stack = true;
            }
            // Its callers' return addresses: the stack's frames but the
            // innermost (each call site once, however deep the recursion).
            seen.clear();
            let mut frames = trace.stacks.frames(s.stack[i]);
            let mut inner = frames.next().unwrap_or(0);
            let mut innermost = true;
            for f in frames {
                // What the frame's call called: the next frame in.
                let callee = if innermost {
                    inner
                } else {
                    inner.wrapping_sub(1)
                };
                innermost = false;
                // A return address is past its call: inside if the call is.
                // But the first user-mode frame outside kernel-mode ones is
                // where the code was interrupted (a system call's or an
                // interrupt's): the instruction itself.
                let interrupted = kernel(inner) && !kernel(f);
                inner = f;
                let at = if interrupted { f } else { f.wrapping_sub(1) };
                if let Some(o) = inside(at) {
                    let o = o + 1;
                    if !seen.contains(&o) {
                        seen.push(o);
                        *c.under.entry(o).or_default() += weight;
                        let target = match resolver.image(process, callee) {
                            Some(img) => {
                                let off = callee - trace.images[img.index()].base;
                                let f = symbols.function_start(trace, img, off);
                                (Some(img), f.unwrap_or(NO_FUNCTION))
                            }
                            None => (None, 0),
                        };
                        *c.targets.entry((o, target.0, target.1)).or_default() += weight;
                    }
                    on_stack = true;
                }
            }
            c.total += u64::from(on_stack) * weight;
        }
        c
    });
    let mut all = Counted::default();
    for c in parts {
        all.add(c);
    }
    all
}
