//! Which members a library gets, and in what order, from the inputs.
//!
//! Order matters: when two members define a symbol, the archive's index
//! names the first in archive order, so a different order can link a
//! different definition. Each tool's rule was measured (`lib.exe` 14.52) or
//! read (`llvm-lib`, `LibDriver.cpp`), and is followed exactly.
//!
//! **`lib.exe`.** The output is, first, each input library's members, the
//! libraries in command-line order and each one's members in reverse of
//! its archive order (the order `/LIST` prints); then the objects, in
//! reverse command-line order. But a library that is the only input keeps
//! its order, when nothing is removed from it. In detail:
//!
//! - An object named twice (names compare without case, `/` read as `\`)
//!   counts once, with LNK4042 for each repeat. A library named twice
//!   counts once, silently.
//! - An object replaces every library member with its name ("Replacing
//!   x"), in every input library.
//! - `/REMOVE:x` removes every library member named x, from every input
//!   library, after replacement; never an object named on the command
//!   line. If none is found, LNK4014. Members of two libraries with the same
//!   name are otherwise both kept.
//! - An output with no members is fatal (LNK1160).
//!
//! **`llvm-lib`.** Each input in command-line order, a library replaced by
//! its members in archive order; inputs named twice by exactly the same
//! path count once; then the whole list reversed. No member replaces
//! another.

use super::diag::Diag;

/// One input, as the ordering sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// An object (or import object, or anything stored whole), by the path
    /// given on the command line.
    Object(String),
    /// A library, by its path, and its members' names in archive order.
    Library(String, Vec<String>),
}

/// Where an output member comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The object `inputs[i]`.
    Object(usize),
    /// Member `m` (in archive order) of the library `inputs[i]`.
    Member(usize, usize),
}

/// The output's members, in archive order, and what the tool says about
/// them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ordered {
    pub members: Vec<Source>,
    /// Warnings, in order.
    pub warnings: Vec<Diag>,
    /// What `lib.exe` prints on standard output while doing this:
    /// "Replacing x" always, and with `/VERBOSE`, "Appending x" for each
    /// member in archive order.
    pub messages: Vec<String>,
}

/// A name as `lib.exe` compares names: `/` as `\`, without case.
pub fn key(name: &str) -> String {
    name.replace('/', "\\").to_lowercase()
}

/// The member name `lib.exe` stores for an input path: the path as given,
/// with `/` written as `\` (measured: `./c.obj` is stored as `.\c.obj`).
/// `llvm-lib` stores the path as given.
pub fn lib_member_name(path: &str) -> String {
    path.replace('/', "\\")
}

/// `lib.exe`'s order, with `/REMOVE`'s names and whether `/VERBOSE` is on.
pub fn lib_order(inputs: &[Input], remove: &[String], verbose: bool) -> Result<Ordered, Diag> {
    let mut out = Ordered::default();

    // Objects, first occurrence of each name.
    let mut objects: Vec<usize> = Vec::new();
    let mut object_keys: Vec<String> = Vec::new();
    let mut libraries: Vec<usize> = Vec::new();
    let mut library_keys: Vec<String> = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        match input {
            Input::Object(path) => {
                let k = key(path);
                if object_keys.contains(&k) {
                    out.warnings.push(
                        Diag::warning(4042, "object specified more than once; extras ignored")
                            .on(path.clone()),
                    );
                } else {
                    object_keys.push(k);
                    objects.push(i);
                }
            }
            Input::Library(path, _) => {
                let k = key(path);
                if !library_keys.contains(&k) {
                    library_keys.push(k);
                    libraries.push(i);
                }
            }
        }
    }

    // Library members, minus those an object replaces. Measured: a library
    // that is the command line's only input keeps its order (`lib
    // /OUT:x.lib y.lib` copies y.lib); otherwise each library's members
    // are reversed (even for `y.lib y.lib`).
    let only = inputs.len() == 1;
    let mut kept: Vec<(usize, usize, String)> = Vec::new();
    let mut replaced = vec![false; objects.len()];
    for &i in &libraries {
        let Input::Library(_, members) = &inputs[i] else {
            unreachable!()
        };
        let in_order: Vec<usize> = if only {
            (0..members.len()).collect()
        } else {
            (0..members.len()).rev().collect()
        };
        for m in in_order {
            let k = key(&members[m]);
            if let Some(o) = object_keys.iter().position(|ok| *ok == k) {
                if !replaced[o] {
                    replaced[o] = true;
                    out.messages.push(format!("Replacing {}", members[m]));
                }
                continue;
            }
            kept.push((i, m, k));
        }
    }

    // /REMOVE, from library members only.
    for r in remove {
        let k = key(r);
        let before = kept.len();
        kept.retain(|(_, _, mk)| *mk != k);
        if kept.len() == before {
            out.warnings.push(Diag::warning(
                4014,
                format!("cannot find member object {r}"),
            ));
        }
    }

    out.members
        .extend(kept.iter().map(|&(i, m, _)| Source::Member(i, m)));
    out.members
        .extend(objects.iter().rev().map(|&i| Source::Object(i)));
    if out.members.is_empty() {
        return Err(Diag::fatal(1160, "library with zero objects not allowed"));
    }
    if verbose {
        for s in &out.members {
            out.messages
                .push(format!("Appending {}", stored_name(inputs, *s)));
        }
    }
    Ok(out)
}

/// The name a member is stored under: a library member's own name, or an
/// object's path as `lib.exe` stores it.
pub fn stored_name(inputs: &[Input], s: Source) -> String {
    match s {
        Source::Object(i) => match &inputs[i] {
            Input::Object(p) => lib_member_name(p),
            Input::Library(..) => unreachable!("an object source names a library"),
        },
        Source::Member(i, m) => match &inputs[i] {
            Input::Library(_, members) => members[m].clone(),
            Input::Object(_) => unreachable!("a member source names an object"),
        },
    }
}

/// `llvm-lib`'s order.
pub fn llvm_order(inputs: &[Input]) -> Ordered {
    let mut seen: Vec<&str> = Vec::new();
    let mut members = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let path = match input {
            Input::Object(p) | Input::Library(p, _) => p.as_str(),
        };
        if seen.contains(&path) {
            continue;
        }
        seen.push(path);
        match input {
            Input::Object(_) => members.push(Source::Object(i)),
            Input::Library(_, m) => members.extend((0..m.len()).map(|m| Source::Member(i, m))),
        }
    }
    members.reverse();
    Ordered {
        members,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
