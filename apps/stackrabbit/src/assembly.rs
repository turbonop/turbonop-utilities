//! A function's instructions, annotated with the time spent in each: its
//! code disassembled (from the image file the trace names, if it's still
//! there and is the image the trace saw) with Hexcavator's decoders, and
//! the trace's CPU samples counted by instruction: the samples whose
//! instruction pointer is at it (its own time), and, at a call, the samples
//! in what it called (by the return addresses on their stacks). Then its
//! basic blocks, each with its time, and the loops among them (a branch to
//! an earlier block).

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};
use turbonop_image::Image;
use turbonop_mmap::Mmap;
use turbonop_trace::{ImageIx, Trace};
use turbonop_trace_analysis::{PdbSymbolizer, Resolver};

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
    // Its extent and name, from the first of its images with symbols.
    let table = images
        .iter()
        .find_map(|&i| symbols.table(trace, i))
        .ok_or_else(|| format!("{module}: no symbols, so no function's extent"))?;
    let (end, name) = table
        .iter()
        .find(|f| f.0 == start)
        .map(|(_, e, n)| (e, n.to_string()))
        .ok_or_else(|| format!("{module}: no function at {start:#x}"))?;
    // Its bytes, from an image file that is the image the trace saw (at
    // the path the trace names, as a DOS path).
    let volumes = crate::reading::Volumes::of(trace, trace_path);
    let mut tried = Vec::new();
    let mut found = None;
    for &i in &images {
        let img = &trace.images[i.index()];
        let file = system_root(&volumes.dos(&img.path));
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
    let (own, callees, total) = count(trace, resolver, &images, start, end, time);
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
        })).collect::<Vec<_>>(),
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

/// `path` with `\SystemRoot\` (as the kernel's images are named) as the
/// Windows directory, where the trace's volumes don't say.
fn system_root(path: &str) -> String {
    const ROOT: &str = r"\SystemRoot\";
    if path.len() >= ROOT.len()
        && path.is_char_boundary(ROOT.len())
        && path[..ROOT.len()].eq_ignore_ascii_case(ROOT)
    {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        return format!(r"{windows}\{}", &path[ROOT.len()..]);
    }
    path.to_string()
}

/// Events a worker counts at a time.
const CHUNK: usize = 1 << 16;

/// The samples in `time` in the function `[start, end)` of `images`: by
/// offset, those at it (instruction pointers) and those under it (return
/// addresses on their stacks, once a sample each); and how many samples
/// had it on their stacks at all. On every core.
fn count(
    trace: &Trace,
    resolver: &Resolver,
    images: &[ImageIx],
    start: u64,
    end: u64,
    time: Option<(i64, i64)>,
) -> (FxHashMap<u64, u64>, FxHashMap<u64, u64>, u64) {
    let s = &trace.samples;
    let (from, to) = match time {
        Some((a, b)) => (
            s.time.partition_point(|&t| t < a),
            s.time.partition_point(|&t| t < b),
        ),
        None => (0, s.len()),
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
    let parts = turbonop_par::map_range(chunks, 1, |c| {
        let mut own: FxHashMap<u64, u64> = FxHashMap::default();
        let mut under: FxHashMap<u64, u64> = FxHashMap::default();
        let mut total = 0u64;
        let mut seen: Vec<u64> = Vec::new();
        for i in from + c * CHUNK..(from + (c + 1) * CHUNK).min(to) {
            let process = trace.thread_process(s.thread[i]);
            let weight = u64::from(s.weight[i]);
            let inside = |address: u64| {
                resolver.image(process, address).and_then(|img| {
                    let offset = address - trace.images[img.index()].base;
                    (images.contains(&img) && offset >= start && offset < end).then_some(offset)
                })
            };
            let mut on_stack = false;
            if let Some(o) = inside(s.ip[i]) {
                *own.entry(o).or_default() += weight;
                on_stack = true;
            }
            // Its callers' return addresses: the stack's frames but the
            // innermost (each call site once, however deep the recursion).
            seen.clear();
            let mut frames = trace.stacks.frames(s.stack[i]);
            let mut inner = frames.next().unwrap_or(0);
            for f in frames {
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
                        *under.entry(o).or_default() += weight;
                    }
                    on_stack = true;
                }
            }
            total += u64::from(on_stack) * weight;
        }
        (own, under, total)
    });
    let (mut own, mut under, mut total) = (FxHashMap::default(), FxHashMap::default(), 0);
    for (o, u, t) in parts {
        for (k, v) in o {
            *own.entry(k).or_default() += v;
        }
        for (k, v) in u {
            *under.entry(k).or_default() += v;
        }
        total += t;
    }
    (own, under, total)
}

#[cfg(test)]
mod tests {
    use super::system_root;

    #[test]
    fn the_kernels_images_are_in_the_windows_directory() {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        assert_eq!(
            system_root(r"\SystemRoot\system32\ntoskrnl.exe"),
            format!(r"{windows}\system32\ntoskrnl.exe")
        );
        assert_eq!(
            system_root(r"\systemroot\System32\drivers\tcpip.sys"),
            format!(r"{windows}\System32\drivers\tcpip.sys")
        );
        assert_eq!(system_root(r"C:\app\a.exe"), r"C:\app\a.exe");
        assert_eq!(system_root("é"), "é");
    }
}
