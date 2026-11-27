//! `ar -M`: MRI librarian scripts, read from standard input, as GNU `ar`
//! (`arsup.c`) and `llvm-ar` run them.
//!
//! A line is a command and its arguments; `*` and `;` start comments.
//! Commands are case-insensitive:
//!
//! | Command | GNU `ar` | `llvm-ar` |
//! |---|---|---|
//! | `CREATE a` | a new archive | the same |
//! | `CREATETHIN a` | - | a new thin archive |
//! | `OPEN a` | an existing archive, its members kept | - |
//! | `ADDMOD f, ...` | the files, in order, before the members already there | the file (one), last |
//! | `ADDLIB l [(m, ...)]` | the library's members each put first (so backwards), or those named, in order, first | its members, last |
//! | `DELETE m, ...` | every member of each name | every member of the name |
//! | `REPLACE f, ...` | each member of the name, or the file put first | - |
//! | `CLEAR` | no members | - |
//! | `SAVE` | writes the archive | marks it to be written at the end |
//! | `END` | ends the script | the same |
//!
//! GNU `ar`'s `DIRECTORY`, `EXTRACT`, `LIST`, `VERBOSE`, and `FULLDIR` are
//! refused here, as are a GNU script's lines after a failed command (GNU
//! `ar` goes on unless run non-interactively, where it stops: as here).

use super::args::{Command, Flavor};
use super::run::{Fatal, Item, Plan, Run, new_item, read_old, write_plan};
use turbonop_archive::unix::Dialect;

/// The words of a GNU script's line: names separated by spaces, commas,
/// and parentheses (the parentheses kept as words).
fn gnu_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in line.chars() {
        match ch {
            ' ' | '\t' | ',' | '(' | ')' => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                if ch == '(' || ch == ')' {
                    out.push(ch.to_string());
                }
            }
            _ => word.push(ch),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Runs the script on standard input.
pub fn run(r: &mut Run<'_>, c: &Command) -> Result<(), Fatal> {
    let text = String::from_utf8_lossy(r.env.stdin.as_deref().unwrap_or(&[])).into_owned();
    let llvm = r.flavor == Flavor::LlvmAr;
    let deterministic = c.deterministic.unwrap_or(true);
    let mut archive: Option<String> = None;
    let mut thin = false;
    let mut members: Vec<Item> = Vec::new();
    let mut saved = false;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split([';', '*']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (command, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        let mut rest = rest.trim().to_string();
        if llvm && rest.len() >= 2 && rest.starts_with('"') && rest.ends_with('"') {
            rest = rest[1..rest.len() - 1].to_string();
        }
        let words = gnu_words(&rest);
        let open = |r: &Run<'_>, archive: &Option<String>| -> Result<String, Fatal> {
            archive.clone().ok_or_else(|| {
                r.fatal(if llvm {
                    "no output archive has been opened".to_string()
                } else {
                    "no open output archive".to_string()
                })
            })
        };
        let line_error = |r: &Run<'_>, e: String| r.fatal(format!("script line {}: {e}", n + 1));
        match command.to_ascii_lowercase().as_str() {
            "create" | "createthin" if command.eq_ignore_ascii_case("create") || llvm => {
                if llvm && archive.is_some() {
                    return Err(r.fatal("editing multiple archives not supported"));
                }
                if llvm && saved {
                    return Err(r.fatal("file already saved"));
                }
                let name = if llvm {
                    rest.clone()
                } else {
                    words.first().cloned().unwrap_or_default()
                };
                if name.is_empty() {
                    return Err(r.fatal("missing archive name"));
                }
                thin = command.eq_ignore_ascii_case("createthin");
                archive = Some(name);
                members.clear();
            }
            "open" if !llvm => {
                let name = words.first().cloned().unwrap_or_default();
                let old = read_old(r, &name)?
                    .ok_or_else(|| r.fatal(format!("Can't open input archive {name}")))?;
                members = old.members;
                thin = false;
                archive = Some(name);
            }
            "addmod" => {
                let a = open(r, &archive)?;
                let files = if llvm {
                    vec![rest.clone()]
                } else {
                    words.clone()
                };
                // GNU: the files, in order, before the members already there
                // (its parser lists them backwards, then puts each first).
                let mut added = Vec::with_capacity(files.len());
                for f in files {
                    added.push(new_item(r, &a, &f, thin, false, false, deterministic)?);
                }
                if llvm {
                    members.extend(added);
                } else {
                    members.splice(0..0, added);
                }
            }
            "addlib" => {
                let a = open(r, &archive)?;
                let lib = if llvm {
                    rest.clone()
                } else {
                    words.first().cloned().unwrap_or_default()
                };
                let old = read_old(r, &lib)?
                    .ok_or_else(|| r.fatal(format!("{lib}: No such file or directory")))?;
                if llvm && thin && !old.thin {
                    return Err(
                        r.fatal("cannot add a regular archive's contents to a thin archive")
                    );
                }
                // GNU: only the modules listed in parentheses, if any.
                let wanted: Vec<&String> = words
                    .iter()
                    .skip(1)
                    .filter(|w| *w != "(" && *w != ")")
                    .collect();
                // GNU: each member put first (so the library's backwards), or
                // the modules listed, in the list's order, first.
                let mut picked: Vec<Item> = Vec::new();
                if wanted.is_empty() {
                    picked = old.members;
                    if !llvm {
                        picked.reverse();
                    }
                } else {
                    for w in &wanted {
                        if let Some(m) = old.members.iter().find(|m| m.name == **w) {
                            picked.push(m.clone());
                        }
                    }
                }
                let mut block = Vec::with_capacity(picked.len());
                for mut m in picked {
                    if llvm && deterministic {
                        m.fields = turbonop_archive::write::Fields::new(b"0", b"0", b"0", b"644")
                            .unwrap_or(m.fields);
                    }
                    if thin && llvm && !std::path::Path::new(&m.name).is_absolute() {
                        // Its path from the new archive's directory.
                        let dir = std::path::Path::new(&lib)
                            .parent()
                            .map_or_else(|| m.name.clone().into(), |d| d.join(&m.name));
                        m.name =
                            super::run::relative_to_archive(&r.env.cwd, &a, &dir.to_string_lossy());
                    }
                    block.push(m);
                }
                if llvm {
                    members.extend(block);
                } else {
                    members.splice(0..0, block);
                }
            }
            "delete" => {
                open(r, &archive)?;
                let names = if llvm {
                    vec![rest.clone()]
                } else {
                    words.clone()
                };
                for name in names {
                    let before = members.len();
                    if llvm {
                        let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
                        members.retain(|m| base(&m.name) != base(&name));
                    } else {
                        members.retain(|m| m.name != name);
                        if members.len() == before {
                            return Err(line_error(r, format!("can't find module file {name}")));
                        }
                    }
                }
            }
            "replace" if !llvm => {
                let a = open(r, &archive)?;
                for f in &words {
                    let mut found = false;
                    for m in &mut members {
                        if m.name == *f {
                            *m = new_item(r, &a, f, thin, false, false, deterministic)?;
                            found = true;
                        }
                    }
                    if !found {
                        r.warn(&format!("can't find module file {f}"));
                        let item = new_item(r, &a, f, thin, false, false, deterministic)?;
                        members.insert(0, item);
                    }
                }
            }
            "clear" if !llvm => members.clear(),
            "save" => {
                let a = open(r, &archive)?;
                saved = true;
                if !llvm {
                    let plan = Plan {
                        members: std::mem::take(&mut members),
                        thin,
                        dialect: Dialect::Gnu,
                        symtab: true,
                        deterministic,
                    };
                    write_plan(r, &a, plan)?;
                    archive = None;
                }
            }
            "end" => break,
            other => {
                return Err(line_error(
                    r,
                    if llvm {
                        format!("unknown command: {other}")
                    } else {
                        format!("{other}: not supported by turbolib's ar -M")
                    },
                ));
            }
        }
    }
    if llvm
        && saved
        && let Some(a) = archive
    {
        let plan = Plan {
            members,
            thin,
            dialect: Dialect::Gnu,
            symtab: true,
            deterministic,
        };
        write_plan(r, &a, plan)?;
    }
    Ok(())
}
