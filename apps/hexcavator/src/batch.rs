//! `hexcavator batch`: a summary of each of many images (PE, ELF, Mach-O,
//! WebAssembly), one JSON line each, for tracking sizes across a product's
//! binaries and builds.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

pub const USAGE: &str = "\
usage: hexcavator batch [options] <file or directory>...

A summary of each image (an executable or shared library: PE, ELF, Mach-O,
or WebAssembly) given, or found in the directories given (by what the files
are, not their names), with its debug information: one JSON object per
line, with its path, its debug information's, its sections' sizes, its
largest libraries and compilands, and its findings' totals (wasted bytes of
duplicated data, and template families' bytes). An image that can't be read
gets a line with its error, and the rest go on. Images are read in
parallel; the lines are in the images' order.

options:
  -r          look in directories' subdirectories too
  -top <n>    libraries and compilands per image (default 20; 0: all)
  -findings   include the findings' totals (slower: reads every symbol)";

pub fn run(args: &[String]) -> ExitCode {
    let mut recurse = false;
    let mut findings = false;
    let mut top = 20usize;
    let mut inputs = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-r" | "/r" => recurse = true,
            "-findings" | "/findings" => findings = true,
            "-top" | "/top" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => top = n,
                None => return usage("-top needs a number"),
            },
            s if s.starts_with('-') && !Path::new(s).exists() => {
                return usage(&format!("unknown option '{s}'"));
            }
            s => inputs.push(PathBuf::from(s)),
        }
    }
    if inputs.is_empty() {
        return usage("no files or directories given");
    }
    let mut images = Vec::new();
    for input in &inputs {
        if input.is_dir() {
            collect(input, recurse, &mut images);
        } else {
            images.push(input.clone());
        }
    }
    images.sort();
    let lines = summaries(&images, top, findings);
    let mut out = io::BufWriter::new(crate::out::stdout());
    let mut failed = false;
    for (path, line) in images.iter().zip(lines) {
        let line = match line {
            Ok(v) => v,
            Err(e) => {
                failed = true;
                json!({"path": path.display().to_string(), "error": e})
            }
        };
        if let Err(e) = writeln!(out, "{line}") {
            return lost(&e);
        }
    }
    if let Err(e) = out.flush() {
        return lost(&e);
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn usage(message: &str) -> ExitCode {
    ereport!("hexcavator batch: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

/// Each image's summary, in order, read in parallel (each thread taking
/// the next image as it finishes one).
fn summaries(images: &[PathBuf], top: usize, findings: bool) -> Vec<Result<Value, String>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(images.len().max(1));
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, Result<Value, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = images.get(i) else { break };
                        mine.push((i, summary(path, top, findings)));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// The images in `dir` (and its subdirectories, if `recurse`): the files
/// that are executables or shared libraries, of any format.
fn collect(dir: &Path, recurse: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if recurse {
                collect(&path, recurse, out);
            }
        } else if is_image(&path) {
            out.push(path);
        }
    }
}

/// Whether the file at `path` is a linked image (an executable or shared
/// library, not an object, a dump, or a debug file).
fn is_image(path: &Path) -> bool {
    let Ok(map) = hexcavator_mmap::Mmap::open(path) else {
        return false;
    };
    hexcavator_image::Image::parse(&map).is_ok_and(|i| i.kind() == hexcavator_image::Kind::Linked)
}

/// The summary of the image at `path`.
fn summary(path: &Path, top: usize, findings: bool) -> Result<Value, String> {
    let opened = hexcavator_program::Opened::open(path, None)?;
    let program = opened.program()?;
    let (image, debug, data, debug_path) = (
        &program.image,
        &program.debug,
        opened.data(),
        opened.debug_path(),
    );
    let model = program.model()?;
    let take = |n: usize| if top == 0 { n } else { top.min(n) };
    let sections: Vec<Value> = model
        .sections
        .iter()
        .map(|s| json!({"name": s.name, "file": s.size.file, "vm": s.size.vm}))
        .collect();
    // Libraries and compilands, largest first.
    let ranked = |totals: std::collections::BTreeMap<u32, hexcavator_size::Totals>,
                  name: &dyn Fn(u32) -> String| {
        let mut v: Vec<(u32, hexcavator_size::Size)> =
            totals.into_iter().map(|(k, t)| (k, t.size)).collect();
        v.sort_by(|a, b| {
            b.1.file
                .max(b.1.vm)
                .cmp(&a.1.file.max(a.1.vm))
                .then_with(|| a.0.cmp(&b.0))
        });
        let n = take(v.len());
        v[..n]
            .iter()
            .map(|(k, s)| json!({"name": name(*k), "file": s.file, "vm": s.vm}))
            .collect::<Vec<Value>>()
    };
    let libraries = ranked(
        model.totals(|p| Some(model.library_of(p.compiland))),
        &|l| {
            model
                .libraries
                .get(l as usize)
                .map_or_else(String::new, |x| x.name.clone())
        },
    );
    let compilands = ranked(model.totals(|p| Some(p.compiland)), &|c| {
        model
            .compilands
            .get(c as usize)
            .map_or_else(String::new, |x| x.name.clone())
    });
    let mut v = json!({
        "path": path.display().to_string(),
        "debug": debug_path.display().to_string(),
        "fileSize": data.len(),
        "sections": sections,
        "libraries": libraries,
        "compilands": compilands,
    });
    if findings {
        let syms = program.symbols()?;
        let f = hexcavator_size::findings::find(image, debug, syms);
        v["findings"] = json!({
            "duplicateData": f.duplicates.iter().map(|d| d.wasted().file).sum::<u64>(),
            "templateFamilies": f.families.iter().map(|t| t.size.file).sum::<u64>(),
            "wastefulVirtuals": f.virtuals.iter().map(hexcavator_size::findings::WastefulVirtual::wasted).sum::<u64>(),
        });
    }
    Ok(v)
}

/// A write that failed: a closed pipe (`| head`) ends the listing as
/// asked; any other error loses output, and fails.
fn lost(e: &io::Error) -> ExitCode {
    if e.kind() == io::ErrorKind::BrokenPipe {
        ExitCode::SUCCESS
    } else {
        ereport!("hexcavator batch: writing the output: {e}");
        ExitCode::FAILURE
    }
}
