//! Names for addresses in an image, for `diff`: from its PDB (found as
//! every command finds it, or given), or from a linker map (link.exe's
//! `/MAP` format, or turbolink's: `rva  name  file`).

use std::path::Path;

/// One named address range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Named {
    /// Relative to the image base.
    pub rva: u32,
    /// Its size (to the next name's address in its section if not known).
    pub size: u32,
    pub name: String,
    /// Whether the size is the debug information's (else it runs to the
    /// next name, or to its section's end).
    pub sized: bool,
}

/// An image's names, by address.
#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    /// Sorted by address (and then name).
    pub list: Vec<Named>,
    /// Where they came from ("the PDB", "the map", "none").
    pub source: String,
}

impl Names {
    /// The name holding `rva`, and how far into it.
    pub fn at(&self, rva: u32) -> Option<(&Named, u32)> {
        let i = self.list.partition_point(|n| n.rva <= rva);
        let n = self.list.get(i.checked_sub(1)?)?;
        // (A name of unknown or zero size holds what follows it, up to the
        // next.)
        (rva - n.rva < n.size.max(1) || n.size == 0).then(|| (n, rva - n.rva))
    }

    /// Every name at the start of what holds `rva` (folded names share an
    /// address), and how far into it `rva` is.
    pub fn all_at(&self, rva: u32) -> Option<(Vec<&str>, u32)> {
        let (first, off) = self.at(rva)?;
        let start = first.rva;
        let from = self.list.partition_point(|n| n.rva < start);
        let names = self.list[from..]
            .iter()
            .take_while(|n| n.rva == start)
            .map(|n| n.name.as_str())
            .collect();
        Some((names, off))
    }

    /// `rva` as `name+offset`; past a name's end, as `name~offset`
    /// from the nearest name before it (the same in both images if what is
    /// between is); before every name, as itself.
    pub fn describe(&self, rva: u32) -> String {
        match self.at(rva) {
            Some((n, 0)) => n.name.clone(),
            Some((n, off)) => format!("{}+{off:#x}", n.name),
            None => {
                let i = self.list.partition_point(|n| n.rva <= rva);
                match i.checked_sub(1).and_then(|i| self.list.get(i)) {
                    Some(n) => format!("{}~{:#x}", n.name, rva - n.rva),
                    None => format!("{rva:#x}"),
                }
            }
        }
    }

    /// Only the names keep holds (those both images have, so that each
    /// describes the other's addresses alike), sizes not known running to
    /// the next of them (or to the end of the section, sections).
    pub fn restricted(&self, keep: &turbonop_hash::Set<&str>, sections: &[(u32, u32)]) -> Names {
        let kept: Vec<&Named> = self
            .list
            .iter()
            .filter(|n| keep.contains(n.name.as_str()))
            .collect();
        let mut list = Vec::with_capacity(kept.len());
        for (i, n) in kept.iter().enumerate() {
            let size = if n.sized {
                n.size
            } else {
                let end = sections
                    .iter()
                    .find(|&&(s, len)| n.rva >= s && n.rva < s.saturating_add(len))
                    .map_or(n.rva, |&(s, len)| s.saturating_add(len));
                let next = kept[i + 1..]
                    .iter()
                    .find(|m| m.rva > n.rva)
                    .map_or(end, |m| m.rva.min(end));
                next.saturating_sub(n.rva)
            };
            list.push(Named {
                size,
                ..(*n).clone()
            });
        }
        Names {
            list,
            source: self.source.clone(),
        }
    }

    /// Each name's address (the first, for a name given twice).
    pub fn by_name(&self) -> turbonop_hash::Map<&str, &Named> {
        let mut m = turbonop_hash::map_with_capacity(self.list.len());
        for n in &self.list {
            m.entry(n.name.as_str()).or_insert(n);
        }
        m
    }
}

/// Reads a linker map: link.exe's (its `Publics by Value` and static
/// symbols lines, `0001:00000000  name  0000000140001000 f  obj`, made
/// relative to its preferred load address) or turbolink's (`rva  name
/// file`, hexadecimal).
pub(crate) fn read_map(text: &str) -> Vec<(u32, String)> {
    let mut base: u64 = 0;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("Preferred load address is ") {
            base = u64::from_str_radix(rest.trim(), 16).unwrap_or(0);
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        // link.exe: "0001:00000000  name  0000000140001000 f  obj".
        if words.len() >= 3
            && words[0].len() == 13
            && words[0].as_bytes().get(4) == Some(&b':')
            && words[2].len() == 16
            && let Ok(va) = u64::from_str_radix(words[2], 16)
        {
            if va >= base
                && let Ok(rva) = u32::try_from(va - base)
                && rva != 0
            {
                out.push((rva, words[1].to_owned()));
            }
            continue;
        }
        // turbolink: "00001000  name  file".
        let mut parts = line.splitn(3, "  ");
        if let (Some(rva), Some(name)) = (parts.next(), parts.next())
            && rva.len() == 8
            && let Ok(rva) = u32::from_str_radix(rva, 16)
            && !name.is_empty()
        {
            out.push((rva, name.to_owned()));
        }
    }
    out
}

/// An image's names: from `map` if given, else from its debug information
/// (`debug`, or found), else none. `sections` is the image's (RVA, size)
/// of each section, for sizes not known otherwise.
pub(crate) fn load(
    image: &Path,
    debug: Option<&Path>,
    map: Option<&Path>,
    image_base: u64,
    sections: &[(u32, u32)],
) -> Result<Names, String> {
    let (mut raw, source): (Vec<(u32, u32, String)>, &str) = if let Some(m) = map {
        let text = std::fs::read(m).map_err(|e| format!("{}: {e}", m.display()))?;
        let list = read_map(&String::from_utf8_lossy(&text));
        (
            list.into_iter().map(|(r, n)| (r, 0, n)).collect(),
            "the map",
        )
    } else {
        match turbonop_program::Opened::open(image, debug) {
            Ok(opened) => {
                let program = opened.program().map_err(|e| e.to_string())?;
                let syms = program.symbols().map_err(|e| e.to_string())?;
                let list = syms
                    .iter()
                    .filter_map(|s| {
                        let rva = if s.addr >= image_base {
                            s.addr - image_base
                        } else {
                            s.addr
                        };
                        let rva = u32::try_from(rva).ok()?;
                        let size = u32::try_from(s.size.vm).unwrap_or(u32::MAX);
                        Some((rva, size, s.name.clone()))
                    })
                    .collect();
                (list, "the PDB")
            }
            Err(turbonop_program::Error::NoDebug { .. }) if debug.is_none() => (Vec::new(), "none"),
            Err(e) => return Err(e.to_string()),
        }
    };
    raw.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
    raw.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);
    // Sizes not known: to the next address in the section, or its end.
    let section_end = |rva: u32| {
        sections
            .iter()
            .find(|&&(s, n)| rva >= s && rva < s.saturating_add(n))
            .map(|&(s, n)| s.saturating_add(n))
    };
    let mut list = Vec::with_capacity(raw.len());
    for i in 0..raw.len() {
        let (rva, size, ref name) = raw[i];
        let size = if size > 0 {
            size
        } else {
            let next = raw[i + 1..].iter().find(|n| n.0 > rva).map(|n| n.0);
            let end = section_end(rva).unwrap_or(rva);
            next.map_or(end, |n| n.min(end.max(rva)))
                .saturating_sub(rva)
        };
        list.push(Named {
            rva,
            size,
            name: name.clone(),
            sized: raw[i].1 > 0,
        });
    }
    Ok(Names {
        list,
        source: source.to_owned(),
    })
}
