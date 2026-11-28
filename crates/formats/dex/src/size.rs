//! Each class's and method's bytes, for size by package, class, and
//! method: a class's definition, class data, interfaces, annotations,
//! and static values; a method's code item and debug information.
//!
//! What classes share (an annotation set or item, a type list, debug
//! information that d8 wrote once for several) is counted for the first
//! class, in class definition order, that refers to it, so that the
//! classes' sizes add up to the bytes they take.

use turbonop_core::Result;

use crate::annotations::{AnnotationsDirectory, annotation_item, annotation_set, annotation_set_ref_list};
use crate::debug::DebugInfoItem;
use crate::{ClassDef, Dex, names};

/// A method's bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MethodSize {
    /// The method (a method index).
    pub method_idx: u32,
    /// Its code item's size (0: no code).
    pub code: u32,
    /// Its debug information's (0: none, or counted for another method).
    pub debug_info: u32,
}

impl MethodSize {
    /// The method's bytes.
    pub fn total(&self) -> u64 {
        u64::from(self.code) + u64::from(self.debug_info)
    }
}

/// A class's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassSize {
    /// The class definition (its index).
    pub class_def_idx: u32,
    /// The class (a type index).
    pub class_idx: u32,
    /// Its definition (32 bytes).
    pub class_def: u32,
    /// Its class data.
    pub class_data: u32,
    /// Its interfaces' type list.
    pub interfaces: u32,
    /// Its annotations: the directory, sets, ref lists, and annotations.
    pub annotations: u32,
    /// Its static values.
    pub static_values: u32,
    /// Its methods' bytes, direct then virtual.
    pub methods: Vec<MethodSize>,
}

impl ClassSize {
    /// The class's bytes, its methods' included.
    pub fn total(&self) -> u64 {
        u64::from(self.class_def)
            + u64::from(self.class_data)
            + u64::from(self.interfaces)
            + u64::from(self.annotations)
            + u64::from(self.static_values)
            + self.methods.iter().map(MethodSize::total).sum::<u64>()
    }
}

/// An item a class refers to: where, how big, and which part of the
/// class it's for.
#[derive(Clone, Copy)]
enum Part {
    Interfaces,
    Annotations,
    StaticValues,
    Debug(usize),
}

struct Pieces {
    size: ClassSize,
    shared: Vec<(u32, u32, Part)>,
}

fn pieces(dex: &Dex<'_>, i: u32, class: &ClassDef) -> Result<Pieces> {
    let data = dex.data();
    let mut size = ClassSize {
        class_def_idx: i,
        class_idx: class.class_idx,
        class_def: 32,
        ..ClassSize::default()
    };
    let mut shared = Vec::new();
    if let Some(cd) = dex.class_data(class)? {
        size.class_data = cd.size;
        for m in cd.methods() {
            let mut ms = MethodSize {
                method_idx: m.method_idx,
                ..MethodSize::default()
            };
            if m.code_off != 0 {
                let code = dex.code_item(m.code_off)?;
                ms.code = code.size;
                if code.debug_info_off != 0 {
                    let d = DebugInfoItem::parse(data, code.debug_info_off)?;
                    shared.push((d.offset, d.size, Part::Debug(size.methods.len())));
                }
            }
            size.methods.push(ms);
        }
    }
    let interfaces = dex.interfaces(class)?;
    if !interfaces.is_empty() {
        #[allow(clippy::cast_possible_truncation)]
        shared.push((class.interfaces_off, interfaces.byte_size() as u32, Part::Interfaces));
    }
    if class.static_values_off != 0 {
        let (_, n) = crate::value::parse_array_sized(data, class.static_values_off)?;
        shared.push((class.static_values_off, n, Part::StaticValues));
    }
    if class.annotations_off != 0 {
        let dir = AnnotationsDirectory::parse(data, class.annotations_off)?;
        shared.push((dir.offset, dir.size, Part::Annotations));
        let set = |off: u32, shared: &mut Vec<(u32, u32, Part)>| -> Result<()> {
            if off == 0 {
                return Ok(());
            }
            let items = annotation_set(data, off)?;
            #[allow(clippy::cast_possible_truncation)]
            shared.push((off, 4 + 4 * items.len() as u32, Part::Annotations));
            for a in items {
                let item = annotation_item(data, a)?;
                shared.push((a, item.size, Part::Annotations));
            }
            Ok(())
        };
        set(dir.class_annotations_off, &mut shared)?;
        for &(_, off) in dir.fields.iter().chain(&dir.methods) {
            set(off, &mut shared)?;
        }
        for &(_, off) in &dir.parameters {
            let list = annotation_set_ref_list(data, off)?;
            #[allow(clippy::cast_possible_truncation)]
            shared.push((off, 4 + 4 * list.len() as u32, Part::Annotations));
            for s in list {
                set(s, &mut shared)?;
            }
        }
    }
    Ok(Pieces { size, shared })
}

/// Every class's bytes (in class definition order), read on every core;
/// what's shared counted once, for the first class that refers to it.
pub fn class_sizes(dex: &Dex<'_>) -> Vec<Result<ClassSize>> {
    let n = dex.class_def_count();
    let all = turbonop_par::map_range(n as usize, 64, |i| {
        #[allow(clippy::cast_possible_truncation)]
        let i = i as u32;
        let class = dex.class_def(i)?;
        pieces(dex, i, &class)
    });
    let mut seen = std::collections::HashSet::new();
    all.into_iter()
        .map(|p| {
            let Pieces { mut size, shared } = p?;
            for (off, n, part) in shared {
                if !seen.insert(off) {
                    continue;
                }
                match part {
                    Part::Interfaces => size.interfaces += n,
                    Part::Annotations => size.annotations += n,
                    Part::StaticValues => size.static_values += n,
                    Part::Debug(m) => size.methods[m].debug_info += n,
                }
            }
            Ok(size)
        })
        .collect()
}

/// The classes' bytes by package (Java's name, `""` for the default
/// package), largest first.
pub fn package_totals(dex: &Dex<'_>, sizes: &[ClassSize]) -> Vec<(String, u64)> {
    let mut by: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for s in sizes {
        let d = dex
            .type_descriptor(s.class_idx)
            .map(crate::mutf8::decode_lossy)
            .unwrap_or_default();
        *by.entry(names::package_of(&d)).or_default() += s.total();
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}
