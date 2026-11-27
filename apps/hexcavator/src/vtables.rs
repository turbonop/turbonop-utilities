//! `hexcavator vtables`: an image's virtual function tables, each slot named
//! by the function it points to (see `turbonop_program::vtables`).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use turbonop_program::vtables::Vtable;

pub const USAGE: &str = "\
usage: hexcavator vtables [options] <image> [<text>]

An image's virtual function tables (MSVC's `vftable's), with each slot's
offset, the address it holds, and the function there, by name: the
interface a class implements, in order. Given text, only the tables
whose names hold it.

Slots are read from the image's base relocations, so a table needs only
its public symbol (a stripped PDB's): its length is then inferred, as
the slots that follow it holding addresses in code, up to the next
table. With RTTI (/GR), the slot before each table points to its
class's locator, which names it: a table with no symbol (a lambda's
std::function's, which a stripped PDB leaves out) is found and named by
its RTTI (shown as such), and the table before it ends there. Without
RTTI, an inferred length is at most the table's: a table with no symbol
right after it reads as more of it. A slot whose function was folded
with others (/OPT:ICF) names the table's class's names first; when more
than one is left, it is one of them.

options:
  -debug <path> its debug information: a PDB, or a file with its DWARF
                (or -pdb; default: its own, or the one it names)
  -all          every name at each slot's address, the other classes' too
  -json         as JSON";

struct Options {
    image: PathBuf,
    filter: Option<String>,
    pdb: Option<PathBuf>,
    all: bool,
    json: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut pdb = None;
    let mut all = false;
    let mut json = false;
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.strip_prefix('-') {
            Some("pdb" | "debug") => {
                pdb = Some(PathBuf::from(it.next().ok_or("-debug needs a path")?));
            }
            Some("all") => all = true,
            Some("json") => json = true,
            Some("help" | "h" | "?") => return Err(String::new()),
            Some(o) if !o.is_empty() => return Err(format!("unknown option -{o}")),
            _ => positional.push(a.clone()),
        }
    }
    let (image, filter) = match &positional[..] {
        [image] => (image, None),
        [image, text] => (image, Some(text.clone())),
        _ => return Err("an image, and text to match, please".into()),
    };
    Ok(Options {
        image: PathBuf::from(image),
        filter,
        pdb,
        all,
        json,
    })
}

pub fn run(args: &[String]) -> ExitCode {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                ereport!("hexcavator vtables: {e}\n");
            }
            ereport!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match report(&o) {
        Ok(text) => crate::out::print(text.as_bytes()),
        Err(e) => {
            ereport!("hexcavator vtables: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report(o: &Options) -> Result<String, String> {
    let opened = turbonop_program::Opened::open(&o.image, o.pdb.as_deref())?;
    let program = opened.program()?;
    let mut tables = program.vtables()?;
    if let Some(f) = &o.filter {
        tables.retain(|t| {
            t.name.contains(f.as_str()) || t.aliases.iter().any(|a| a.contains(f.as_str()))
        });
    }
    let shown = |rva: u64| program.image.shown_address(rva);
    if o.json {
        let j: Vec<_> = tables
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "aliases": t.aliases,
                    "class": t.class,
                    "for": t.for_base,
                    "rva": format!("{:#x}", t.rva),
                    "address": format!("{:#x}", shown(t.rva)),
                    "section": t.section,
                    "sized": t.sized,
                    "by_rtti": t.by_rtti,
                    "slots": t.slots.iter().enumerate().map(|(i, s)| json!({
                        "index": i,
                        "offset": s.offset,
                        "target": format!("{:#x}", shown(s.target)),
                        "name": s.name(),
                        "names": s.names,
                        "own": s.own,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        return Ok(format!("{:#}\n", json!(j)));
    }
    let mut out = String::new();
    for t in &tables {
        text(&mut out, t, o.all, &shown);
    }
    let _ = writeln!(out, "({} tables)", tables.len());
    Ok(out)
}

fn text(out: &mut String, t: &Vtable, all: bool, shown: &dyn Fn(u64) -> u64) {
    let length = if t.sized {
        ""
    } else {
        ", length inferred: at most"
    };
    let rtti = if t.by_rtti { ", named by its RTTI" } else { "" };
    let _ = writeln!(
        out,
        "{}  ({:#x}, {}, {} slots{length}{rtti})",
        t.name,
        shown(t.rva),
        t.section,
        t.slots.len()
    );
    for a in &t.aliases {
        let _ = writeln!(out, "  also {a}");
    }
    for (i, s) in t.slots.iter().enumerate() {
        let head = format!("  {i:>3}  +{:<5x} {:#x}", s.offset, shown(s.target));
        let shown_names: &[String] = if all || s.own == 0 {
            &s.names
        } else {
            &s.names[..s.own]
        };
        match shown_names {
            [] => {
                let _ = writeln!(out, "{head}  (no name)");
            }
            [one] => {
                let _ = writeln!(out, "{head}  {one}");
            }
            many if all => {
                let _ = writeln!(
                    out,
                    "{head}  {} names ({} of its class):",
                    many.len(),
                    s.own
                );
                for n in many {
                    let _ = writeln!(out, "{:>w$}{n}", "", w = head.len() + 4);
                }
            }
            many => {
                let _ = writeln!(out, "{head}  one of {}:", many.len());
                for n in many {
                    let _ = writeln!(out, "{:>w$}{n}", "", w = head.len() + 4);
                }
            }
        }
    }
    out.push('\n');
}
