//! Resources: `.res` files (what `rc.exe` compiles), turned into the
//! image's `.rsrc` section as `link.exe` does (through `cvtres`): a tree of
//! directories (type, then name, then language), each level's named
//! entries before its numbered ones and each kind sorted (the loader
//! searches them by halves), then the data entries, the names, and the
//! data.

/// A resource's type or name: a number, or a string (UTF-16).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Id {
    /// A string, as UTF-16 code units (sorted before numbers).
    Name(Vec<u16>),
    /// A number.
    Number(u16),
}

/// One resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// Its type (`RT_VERSION` is 16, `RT_MANIFEST` 24).
    pub ty: Id,
    /// Its name.
    pub name: Id,
    /// Its language.
    pub language: u16,
    /// Its data.
    pub data: &'static [u8],
}

/// Reads one `.res` file's resources (the empty entry that starts every
/// file is skipped).
pub fn parse_res(data: &'static [u8]) -> Result<Vec<Resource>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let u32_at = |o: usize| -> Result<u32, String> {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "a resource header past the end".to_owned())
    };
    let u16_at = |o: usize| -> Result<u16, String> {
        data.get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "a resource header past the end".to_owned())
    };
    let id_at = |o: &mut usize| -> Result<Id, String> {
        if u16_at(*o)? == 0xFFFF {
            let n = u16_at(*o + 2)?;
            *o += 4;
            return Ok(Id::Number(n));
        }
        let mut s = Vec::new();
        loop {
            let c = u16_at(*o)?;
            *o += 2;
            if c == 0 {
                break;
            }
            s.push(c);
        }
        Ok(Id::Name(s))
    };
    while at + 8 <= data.len() {
        let size = u32_at(at)? as usize;
        let header = u32_at(at + 4)? as usize;
        let mut o = at + 8;
        let ty = id_at(&mut o)?;
        let name = id_at(&mut o)?;
        o = (o + 3) & !3;
        // DataVersion, MemoryFlags, LanguageId, Version, Characteristics.
        let language = u16_at(o + 6)?;
        let start = at + header;
        let body = data
            .get(start..start + size)
            .ok_or("a resource's data past the end")?;
        let empty = size == 0 && ty == Id::Number(0) && name == Id::Number(0);
        if !empty {
            out.push(Resource {
                ty,
                name,
                language,
                data: body,
            });
        }
        at = (start + size + 3) & !3;
    }
    Ok(out)
}

/// The `.rsrc` section's plan: the resources sorted, and the section's
/// size.
#[derive(Clone, Debug)]
pub struct Rsrc {
    /// By type, name, language.
    resources: Vec<Resource>,
    /// The size of the section.
    pub size: u32,
    /// The directory's size (with its strings, to 16 bytes): where the
    /// data starts (link.exe's `.rsrc$01` group, the data `.rsrc$02`).
    pub directory_size: u32,
}

/// The order of entries in a directory: names (by their upper-case
/// code units, as the loader compares them) before numbers (ascending).
fn key(id: &Id) -> (u8, Vec<u16>, u16) {
    match id {
        Id::Name(s) => (
            0,
            s.iter()
                .map(|&c| {
                    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) {
                        c - 32
                    } else {
                        c
                    }
                })
                .collect(),
            0,
        ),
        Id::Number(n) => (1, Vec::new(), *n),
    }
}

/// Plans `.rsrc` for `resources` (an error for two of one type, name, and
/// language, which `link.exe` refuses: CVT1100).
pub fn plan(mut resources: Vec<Resource>) -> Result<Rsrc, String> {
    resources.sort_by(|a, b| {
        (key(&a.ty), key(&a.name), a.language).cmp(&(key(&b.ty), key(&b.name), b.language))
    });
    for w in resources.windows(2) {
        if w[0].ty == w[1].ty && w[0].name == w[1].name && w[0].language == w[1].language {
            return Err(format!(
                "duplicate resource: type {:?}, name {:?}, language {:#x}",
                w[0].ty, w[0].name, w[0].language
            ));
        }
    }
    let t = Tree::of(&resources);
    let size = t.data_end(&resources);
    let directory_size = t.data_start();
    Ok(Rsrc {
        resources,
        size,
        directory_size,
    })
}

/// A type's names, each with the indices of its languages' resources.
type Names = Vec<(Id, Vec<usize>)>;

/// Where each part of the tree goes.
struct Tree {
    /// Per type: its name directories; per name: the indices (into the
    /// sorted resources) of its languages.
    types: Vec<(Id, Names)>,
    /// Offsets: the directories end here; then data entries, names, data.
    directories: u32,
    entries: u32,
    strings: u32,
}

impl Tree {
    fn of(resources: &[Resource]) -> Tree {
        let mut types: Vec<(Id, Names)> = Vec::new();
        for (i, r) in resources.iter().enumerate() {
            if types.last().is_none_or(|t| t.0 != r.ty) {
                types.push((r.ty.clone(), Vec::new()));
            }
            let names = &mut types.last_mut().expect("just pushed").1;
            if names.last().is_none_or(|n| n.0 != r.name) {
                names.push((r.name.clone(), Vec::new()));
            }
            names.last_mut().expect("just pushed").1.push(i);
        }
        let table = |n: usize| 16 + 8 * crate::n32(n);
        let mut directories = table(types.len());
        for (_, names) in &types {
            directories += table(names.len());
            for (_, langs) in names {
                directories += table(langs.len());
            }
        }
        let entries = directories + 16 * crate::n32(resources.len());
        let mut strings = entries;
        let mut seen = turbonop_hash::Set::default();
        for (ty, names) in &types {
            for id in std::iter::once(ty).chain(names.iter().map(|n| &n.0)) {
                if let Id::Name(s) = id
                    && seen.insert(s.clone())
                {
                    strings += 2 + 2 * crate::n32(s.len());
                }
            }
        }
        Tree {
            types,
            directories,
            entries,
            strings,
        }
    }

    /// Where the data starts: after the directory, on 16 bytes (as
    /// link.exe places it; each resource's data then on 8).
    fn data_start(&self) -> u32 {
        (self.strings + 15) & !15
    }

    /// The section's size: the data's end, on 8 bytes (as link.exe's).
    fn data_end(&self, resources: &[Resource]) -> u32 {
        let mut at = self.data_start();
        for r in resources {
            at = ((at + 7) & !7) + crate::n32(r.data.len());
        }
        (at + 7) & !7
    }
}

impl Rsrc {
    /// Writes the section (at `rva`) into `out`.
    pub fn write(&self, out: &mut [u8], rva: u32) {
        let t = Tree::of(&self.resources);
        let put32 = |out: &mut [u8], at: u32, v: u32| {
            out[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put16 = |out: &mut [u8], at: u32, v: u16| {
            out[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
        };
        // Strings: each distinct name once, length-prefixed.
        let mut string_at: turbonop_hash::Map<Vec<u16>, u32> = turbonop_hash::Map::default();
        let mut next_string = t.entries;
        let mut string = |out: &mut [u8], s: &[u16]| -> u32 {
            if let Some(&o) = string_at.get(s) {
                return o;
            }
            let o = next_string;
            put16(out, o, crate::n16(s.len()));
            for (k, &c) in s.iter().enumerate() {
                put16(out, o + 2 + 2 * crate::n32(k), c);
            }
            next_string += 2 + 2 * crate::n32(s.len());
            string_at.insert(s.to_vec(), o);
            o
        };
        // A directory table at `at` with `ids`; returns where each entry's
        // offset field is.
        let table = |out: &mut [u8],
                     at: u32,
                     ids: &[&Id],
                     string: &mut dyn FnMut(&mut [u8], &[u16]) -> u32|
         -> Vec<u32> {
            let named = ids.iter().filter(|i| matches!(i, Id::Name(_))).count();
            put16(out, at + 12, crate::n16(named));
            put16(out, at + 14, crate::n16(ids.len() - named));
            let mut fields = Vec::with_capacity(ids.len());
            for (k, id) in ids.iter().enumerate() {
                let e = at + 16 + 8 * crate::n32(k);
                let name = match id {
                    Id::Number(n) => u32::from(*n),
                    Id::Name(s) => 0x8000_0000 | string(out, s),
                };
                put32(out, e, name);
                fields.push(e + 4);
            }
            fields
        };
        // Breadth first: the types' table, then each type's names' table,
        // then each name's languages' table.
        let ty_ids: Vec<&Id> = t.types.iter().map(|x| &x.0).collect();
        let type_fields = table(out, 0, &ty_ids, &mut string);
        let mut at = 16 + 8 * crate::n32(t.types.len());
        let mut name_tables = Vec::new();
        for ((_, names), field) in t.types.iter().zip(&type_fields) {
            put32(out, *field, 0x8000_0000 | at);
            let ids: Vec<&Id> = names.iter().map(|x| &x.0).collect();
            name_tables.push((at, table(out, at, &ids, &mut string)));
            at += 16 + 8 * crate::n32(names.len());
        }
        let mut data_at = t.data_start();
        let mut entry = 0u32;
        for ((_, names), (_, fields)) in t.types.iter().zip(&name_tables) {
            for ((_, langs), field) in names.iter().zip(fields) {
                put32(out, *field, 0x8000_0000 | at);
                let lang_ids: Vec<Id> = langs
                    .iter()
                    .map(|&i| Id::Number(self.resources[i].language))
                    .collect();
                let refs: Vec<&Id> = lang_ids.iter().collect();
                let lang_fields = table(out, at, &refs, &mut string);
                at += 16 + 8 * crate::n32(langs.len());
                for (&i, lf) in langs.iter().zip(&lang_fields) {
                    let r = &self.resources[i];
                    let e = t.directories + 16 * entry;
                    entry += 1;
                    put32(out, *lf, e);
                    data_at = (data_at + 7) & !7;
                    put32(out, e, rva + data_at);
                    put32(out, e + 4, crate::n32(r.data.len()));
                    out[data_at as usize..data_at as usize + r.data.len()].copy_from_slice(r.data);
                    data_at += crate::n32(r.data.len());
                }
            }
        }
        debug_assert_eq!(at, t.directories);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.res` entry.
    fn entry(out: &mut Vec<u8>, ty: &Id, name: &Id, lang: u16, data: &[u8]) {
        let mut h = Vec::new();
        for id in [ty, name] {
            match id {
                Id::Number(n) => {
                    h.extend_from_slice(&0xFFFFu16.to_le_bytes());
                    h.extend_from_slice(&n.to_le_bytes());
                }
                Id::Name(s) => {
                    for c in s {
                        h.extend_from_slice(&c.to_le_bytes());
                    }
                    h.extend_from_slice(&0u16.to_le_bytes());
                }
            }
        }
        while (8 + h.len()) % 4 != 0 {
            h.push(0);
        }
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&0x30u16.to_le_bytes());
        h.extend_from_slice(&lang.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(crate::n32(data.len())).to_le_bytes());
        out.extend_from_slice(&(crate::n32(8 + h.len())).to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }

    fn name(s: &str) -> Id {
        Id::Name(s.encode_utf16().collect())
    }

    #[test]
    fn res_files_to_a_tree() {
        let mut res = Vec::new();
        entry(&mut res, &Id::Number(0), &Id::Number(0), 0, b"");
        entry(
            &mut res,
            &Id::Number(16),
            &Id::Number(1),
            0x409,
            b"version!",
        );
        entry(&mut res, &name("MYTYPE"), &name("b"), 0x409, b"bee");
        entry(&mut res, &name("MYTYPE"), &Id::Number(5), 0x409, b"five");
        entry(&mut res, &name("MYTYPE"), &name("A"), 0x407, b"a-de");
        entry(&mut res, &name("MYTYPE"), &name("A"), 0x409, b"a-us");
        let res: &'static [u8] = Box::leak(res.into_boxed_slice());
        let parsed = parse_res(res).unwrap();
        assert_eq!(parsed.len(), 5);
        let r = plan(parsed.clone()).unwrap();
        let mut out = vec![0u8; r.size as usize];
        r.write(&mut out, 0x9000);
        let u16_at = |o: u32| u16::from_le_bytes([out[o as usize], out[o as usize + 1]]);
        let u32_at =
            |o: u32| u32::from_le_bytes(out[o as usize..o as usize + 4].try_into().unwrap());
        // The root: one named type (MYTYPE), one numbered (16).
        assert_eq!((u16_at(12), u16_at(14)), (1, 1));
        assert_eq!(u32_at(16 + 8), 16, "RT_VERSION's ID");
        // MYTYPE's names: "A", "b" (by upper case), then 5.
        let mytype = u32_at(16 + 4) & 0x7FFF_FFFF;
        assert_eq!((u16_at(mytype + 12), u16_at(mytype + 14)), (2, 1));
        let s = u32_at(mytype + 16) & 0x7FFF_FFFF;
        assert_eq!((u16_at(s), u16_at(s + 2)), (1, u16::from(b'A')));
        // A's languages: 0x407, then 0x409; the latter's data is "a-us".
        let a = u32_at(mytype + 16 + 4) & 0x7FFF_FFFF;
        assert_eq!(u32_at(a + 16 + 8), 0x409);
        let e = u32_at(a + 16 + 8 + 4);
        let data = u32_at(e) - 0x9000;
        assert_eq!(&out[data as usize..data as usize + 4], b"a-us");
        assert_eq!(u32_at(e + 4), 4);
        assert_eq!(data % 8, 0);
        // A duplicate is refused.
        let mut twice = parsed.clone();
        twice.push(parsed[0].clone());
        assert!(plan(twice).is_err());
        assert!(parse_res(b"\x10\0\0\0").is_ok());
    }
}
