//! Mach-O: `turbolib compare-images` for two linked Mach-O images
//! ([`compare`]), as for ELF programs ([`crate::elf`]): everything by what
//! addresses name, since folding moves code between members and the linker
//! lays it out in another order.
//!
//! **Names.** Every address is named by a label at or below it and the
//! distance from it: the image's symbols (defined in a section; not the
//! anchors, `~turbolib.anchor.`, which are absolute), external ones by
//! their names, local ones by theirs where no other symbol has them, else
//! an LSDA (`GCC_except_table*`) by the function whose unwind information
//! names it (`lsda:` and its name), anything else by the object the debug
//! map puts it in (`name@object`, and an ordinal where that repeats); then
//! the linker's stubs and pointers by the import their indirect symbol
//! names (`stub:`, `got:`, `lazy:`); then sections' starts. An address in
//! no symbol's extent in a section that isn't code is named by its
//! contents (string literals, which have no symbols).
//!
//! **Compared:**
//!
//! - **Functions and data**: each symbol's extent (to the next symbol in
//!   its section), paired by key, its bytes compared as ELF programs' are
//!   ([`crate::image::bytes`]: x86-64's and arm64's PC-relative operands,
//!   arm64's `adrp` with the `add` or load that completes it, by what they
//!   name), its fixups first (rebased pointers by what their targets name,
//!   bound ones by their imports and addends), its alignment padding left
//!   out (x86-64's NOPs, arm64's `nop`s or zeros after the last
//!   instruction, which differ with what follows a function);
//! - **symbols** as a set (name, external or not, section), the anchors
//!   apart;
//! - **initializers**: `__mod_init_func`'s entries, by what they name, in
//!   order;
//! - **unwind information**: each function's compact unwind encoding
//!   (a DWARF-mode one's FDE offset taken out), personality and LSDA by
//!   name, and a DWARF-mode function's FDE (its call frame instructions);
//! - **imports**: every bound name;
//! - **DWARF**, when both images have a dSYM (`<image>.dSYM`, as
//!   `dsymutil` makes it next to the image): as for ELF programs
//!   ([`crate::elf`]'s), every live subprogram with its subtree and line
//!   rows, variables at fixed addresses, and named types, by content;
//! - section sizes, reported.
//!
//! What it can't prove: nothing was run; members loaded, and their order,
//! are the link's to report (`ld -t`); and stubs and the lazy-binding
//! helper are compared only through what calls them.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use turbonop_dwarf::Sections;
use turbonop_macho::MachO;
use turbonop_macho::fixups::FixupKind;

use crate::elf::dwarf::Addresses;
use crate::elf::program::CONTENT;
use crate::image::bytes::{self, Fields, Isa, Names, Side};
use crate::report::{Report, show};

const ANCHOR: &[u8] = b"~turbolib.anchor.";
const N_EXT: u8 = 0x01;
const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x400;
const S_NON_LAZY_SYMBOL_POINTERS: u32 = 0x6;
const S_LAZY_SYMBOL_POINTERS: u32 = 0x7;
const S_SYMBOL_STUBS: u32 = 0x8;
const S_MOD_INIT_FUNC_POINTERS: u32 = 0x9;
const S_THREAD_LOCAL_VARIABLE_POINTERS: u32 = 0x14;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000C;
const UNWIND_MODE_MASK: u32 = 0x0F00_0000;
const INDIRECT_SYMBOL_LOCAL: u32 = 0x8000_0000;
const INDIRECT_SYMBOL_ABS: u32 = 0x4000_0000;
/// How many bytes a name by contents holds at most.
const CONTENT_BYTES: usize = 4096;

/// Whether `data` is a Mach-O image (not an object, nor a universal file).
pub fn is_macho_image(data: &[u8]) -> bool {
    turbonop_macho::is_macho(data)
        && MachO::parse(data).is_ok_and(|m| !m.is_object() && m.header().is64)
}

/// A section of an image.
#[derive(Clone, Debug)]
struct Sect<'a> {
    seg: &'a [u8],
    name: &'a [u8],
    addr: u64,
    size: u64,
    flags: u32,
    reserved1: u32,
    reserved2: u32,
    /// Its bytes (empty for zero-fill).
    data: &'a [u8],
    zerofill: bool,
}

impl Sect<'_> {
    fn code(&self) -> bool {
        self.flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) != 0
    }
    fn kind(&self) -> u32 {
        self.flags & 0xFF
    }
    fn full_name(&self) -> String {
        format!("{},{}", show(self.seg), show(self.name))
    }
}

/// A label of an address.
#[derive(Clone, Debug)]
struct Label {
    addr: u64,
    name: Box<[u8]>,
    /// Lower is preferred: 0 external symbols, 1 local ones, 2 the
    /// linker's stubs and pointers, 3 sections' starts.
    rank: u8,
    /// A symbol's (a function's or a datum's): its extent is compared.
    region: bool,
}

/// A fixup in an image: a rebased pointer (its target), or a bound one
/// (its import, addend, and whether weak).
#[derive(Clone, Debug)]
struct Fix {
    addr: u64,
    width: u8,
    rebase: Option<u64>,
    bind: Option<(Box<[u8]>, i64, bool)>,
}

impl Fix {
    /// The fixup as text: a rebased pointer by what its target names, a
    /// bound one by its import.
    fn text(&self, img: &Image<'_>) -> String {
        match (&self.rebase, &self.bind) {
            (Some(target), _) => {
                // (arm64's type-name pointers of non-unique RTTI have the
                // top bit set.)
                let top = target & (1 << 63);
                let t = target & !(1 << 63);
                format!(
                    "rebase {}{}",
                    img.render_of(t),
                    if top != 0 { " | 1 << 63" } else { "" }
                )
            }
            (None, Some((symbol, addend, weak))) => format!(
                "bind {}{addend:+}{}",
                show(symbol),
                if *weak { " weak" } else { "" }
            ),
            (None, None) => String::new(),
        }
    }
}

/// A function's or datum's extent: its key, address, size, and whether code.
type Region = (Box<[u8]>, u64, u64, bool);

/// One linked image, read for comparison.
pub(crate) struct Image<'a> {
    mach: MachO<'a>,
    cputype: u32,
    base: u64,
    sections: Vec<Sect<'a>>,
    labels: Vec<Label>,
    /// The preferred label at each distinct address, by address.
    primary: Vec<usize>,
    /// Fixups, by address.
    fixes: Vec<Fix>,
    /// Each region: (key, address, size, code), and the symbols as
    /// (name, external, section).
    regions: Vec<(Box<[u8]>, u64, u64, bool)>,
    symbols: Vec<(Box<[u8]>, bool, String)>,
    /// Bound import names (each with whether it is a weak import).
    imports: BTreeSet<(Box<[u8]>, bool)>,
    /// Each datum's extent bound, from both images (the shorter of its
    /// two extents to the next symbol: it holds the whole datum): past it,
    /// an address is named by its contents.
    limits: BTreeMap<Box<[u8]>, u64>,
}

fn le(b: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(n)?)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(s);
    Some(u64::from_le_bytes(v))
}

impl<'a> Image<'a> {
    fn read(data: &'a [u8], what: &str) -> Result<Self, String> {
        let mach = MachO::parse(data).map_err(|e| format!("{what}: {e:?}"))?;
        let h = *mach.header();
        if !h.is64 || mach.is_object() {
            return Err(format!("{what}: not a 64-bit Mach-O image"));
        }
        let cputype = h.cputype;
        let base = mach
            .segments()
            .iter()
            .find(|s| s.name == b"__TEXT")
            .map_or(0, |s| s.vmaddr);
        let sections: Vec<Sect<'a>> = mach
            .sections()
            .into_iter()
            .map(|s| {
                let zerofill = s.is_zerofill();
                let data = if zerofill {
                    &[][..]
                } else {
                    let o = s.offset as usize;
                    data.get(o..o.saturating_add(s.size as usize))
                        .unwrap_or(&[])
                };
                Sect {
                    seg: s.segname,
                    name: s.sectname,
                    addr: s.addr,
                    size: s.size,
                    flags: s.flags,
                    reserved1: s.reserved1,
                    reserved2: s.reserved2,
                    data,
                    zerofill,
                }
            })
            .collect();
        let symbols = mach.symbols();
        // The indirect symbol table: stubs' and pointers' imports.
        let indirect: Vec<u32> = mach
            .dysymtab()
            .map(|d| {
                (0..d.nindirectsyms)
                    .filter_map(|i| {
                        let at = d.indirectsymoff as usize + 4 * i as usize;
                        le(data, at, 4).map(|v| v as u32)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut labels: Vec<Label> = Vec::new();
        // Symbols: externals by name; locals by name where unique.
        let defined: Vec<&turbonop_macho::Nlist<'a>> = symbols
            .iter()
            .filter(|y| {
                y.is_section_defined()
                    && !y.name.is_empty()
                    && !y.name.starts_with(ANCHOR)
                    && y.n_sect > 0
                    && usize::from(y.n_sect) <= sections.len()
            })
            .collect();
        let mut local_count: BTreeMap<&[u8], usize> = BTreeMap::new();
        for y in &defined {
            if y.n_type & N_EXT == 0 {
                *local_count.entry(y.name).or_default() += 1;
            }
        }
        // The objects the debug map puts addresses in.
        let mut object_of: BTreeMap<u64, &[u8]> = BTreeMap::new();
        for o in mach.debug_map() {
            let short = o.path.rsplit(|&c| c == b'/').next().unwrap_or(o.path);
            for s in o.symbols {
                object_of.entry(s.addr).or_insert(short);
            }
        }
        // LSDAs by the function their unwind information is for.
        let unwind = mach.unwind_info().unwrap_or_default();
        let mut lsda_of: BTreeMap<u64, u64> = BTreeMap::new();
        for e in &unwind.entries {
            if let Some(l) = e.lsda {
                lsda_of.entry(l).or_insert(e.function);
            }
        }
        let mut seen: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
        let mut pending_lsda: Vec<(usize, u64)> = Vec::new();
        for y in &defined {
            let si = usize::from(y.n_sect) - 1;
            let external = y.n_type & N_EXT != 0;
            let mut name: Vec<u8> = y.name.to_vec();
            if !external && let Some(&f) = lsda_of.get(&y.n_value) {
                // (An LSDA, by its function, wherever it is.)
                pending_lsda.push((labels.len(), f));
            } else if !external
                && local_count.get(y.name).copied().unwrap_or(0) > 1
                && let Some(o) = object_of.get(&y.n_value)
            {
                name.push(b'@');
                name.extend_from_slice(o);
            }
            let n = seen.entry(name.clone()).or_default();
            *n += 1;
            if *n > 1 {
                name.extend_from_slice(format!("#{n}").as_bytes());
            }
            labels.push(Label {
                addr: y.n_value,
                name: name.into_boxed_slice(),
                rank: if external { 0 } else { 1 },
                region: !matches!(
                    sections[si].kind(),
                    S_SYMBOL_STUBS | S_NON_LAZY_SYMBOL_POINTERS | S_LAZY_SYMBOL_POINTERS
                ),
            });
        }
        // Stubs and the linker's pointers.
        for s in &sections {
            let (prefix, size): (&[u8], u64) = match s.kind() {
                S_SYMBOL_STUBS => (b"stub:", u64::from(s.reserved2.max(1))),
                S_NON_LAZY_SYMBOL_POINTERS => (b"got:", 8),
                S_LAZY_SYMBOL_POINTERS => (b"lazy:", 8),
                S_THREAD_LOCAL_VARIABLE_POINTERS => (b"tlvp:", 8),
                _ => continue,
            };
            for k in 0..s.size / size {
                let Some(&i) = indirect.get(s.reserved1 as usize + k as usize) else {
                    break;
                };
                let target: &[u8] = if i & (INDIRECT_SYMBOL_LOCAL | INDIRECT_SYMBOL_ABS) != 0 {
                    b"(local)"
                } else {
                    symbols.get(i as usize).map_or(&b"?"[..], |y| y.name)
                };
                labels.push(Label {
                    addr: s.addr + k * size,
                    name: [prefix, target].concat().into_boxed_slice(),
                    rank: 2,
                    region: false,
                });
            }
            labels.push(Label {
                addr: s.addr,
                name: format!("section:{}", s.full_name()).into_bytes().into(),
                rank: 3,
                region: false,
            });
        }
        for s in &sections {
            if !matches!(
                s.kind(),
                S_SYMBOL_STUBS | S_NON_LAZY_SYMBOL_POINTERS | S_LAZY_SYMBOL_POINTERS
            ) {
                labels.push(Label {
                    addr: s.addr,
                    name: format!("section:{}", s.full_name()).into_bytes().into(),
                    rank: 3,
                    region: false,
                });
            }
        }
        labels.sort_by(|a, b| (a.addr, a.rank, &a.name).cmp(&(b.addr, b.rank, &b.name)));
        let mut img = Image {
            mach,
            cputype,
            base,
            sections,
            labels,
            primary: Vec::new(),
            fixes: Vec::new(),
            regions: Vec::new(),
            symbols: Vec::new(),
            imports: BTreeSet::new(),
            limits: BTreeMap::new(),
        };
        img.index();
        // LSDAs named by their functions (once those have names).
        if !pending_lsda.is_empty() {
            let mut renamed = Vec::new();
            for (li, f) in &pending_lsda {
                let _ = li;
                let func = img.render(*f);
                renamed.push((*f, format!("lsda:{func}")));
            }
            let by_func: BTreeMap<u64, String> = renamed.into_iter().collect();
            for l in &mut img.labels {
                if let Some(&f) = lsda_of.get(&l.addr)
                    && l.rank == 1
                    && let Some(n) = by_func.get(&f)
                {
                    l.name = n.clone().into_bytes().into();
                }
            }
            img.labels
                .sort_by(|a, b| (a.addr, a.rank, &a.name).cmp(&(b.addr, b.rank, &b.name)));
            img.index();
        }
        // Symbols, as a set (an LSDA's by its function: its own name is
        // numbered by its object).
        for y in &defined {
            let si = usize::from(y.n_sect) - 1;
            let name: Box<[u8]> = match lsda_of.get(&y.n_value) {
                Some(&f) if y.n_type & N_EXT == 0 => {
                    format!("lsda:{}", img.render(f)).into_bytes().into()
                }
                _ => y.name.into(),
            };
            img.symbols
                .push((name, y.n_type & N_EXT != 0, img.sections[si].full_name()));
        }
        img.symbols.sort();
        // Regions: each symbol's extent, to the next symbol of its section.
        let starts: Vec<(u64, usize)> = img
            .labels
            .iter()
            .enumerate()
            .filter(|(_, l)| l.rank <= 1)
            .map(|(i, l)| (l.addr, i))
            .collect();
        let mut regions = Vec::new();
        for (k, &(addr, i)) in starts.iter().enumerate() {
            let l = &img.labels[i];
            if !l.region || starts.get(k + 1).is_some_and(|n| n.0 == addr) {
                continue;
            }
            let Some(s) = img.section_at(addr) else {
                continue;
            };
            let end = starts
                .iter()
                .skip(k + 1)
                .map(|x| x.0)
                .find(|&n| n > addr)
                .unwrap_or(s.addr + s.size)
                .min(s.addr + s.size);
            regions.push((l.name.clone(), addr, end - addr, s.code()));
        }
        img.regions = regions;
        // Fixups.
        let mut fixes = Vec::new();
        for f in img.mach.fixups() {
            let (rebase, bind) = match f.kind {
                FixupKind::Rebase { target } => (Some(target), None),
                FixupKind::Bind {
                    symbol,
                    addend,
                    weak,
                    ..
                } => {
                    img.imports.insert((symbol.into(), weak));
                    (None, Some((symbol.into(), addend, weak)))
                }
            };
            fixes.push(Fix {
                addr: f.addr,
                width: f.width,
                rebase,
                bind,
            });
        }
        fixes.sort_by_key(|f| f.addr);
        img.fixes = fixes;
        Ok(img)
    }

    /// The preferred label of each distinct address.
    fn index(&mut self) {
        self.primary.clear();
        for (i, l) in self.labels.iter().enumerate() {
            if self
                .primary
                .last()
                .is_none_or(|&p| self.labels[p].addr != l.addr)
            {
                self.primary.push(i);
            }
        }
    }

    fn section_at(&self, addr: u64) -> Option<&Sect<'a>> {
        self.sections
            .iter()
            .find(|s| addr >= s.addr && addr - s.addr < s.size)
    }

    fn bytes(&self, addr: u64, len: u64) -> Option<&'a [u8]> {
        let s = self.section_at(addr)?;
        let at = (addr - s.addr) as usize;
        s.data.get(at..at.checked_add(len as usize)?)
    }

    /// The name of `addr`: a label and the distance from it, or its
    /// contents.
    fn name_of(&self, addr: u64) -> Option<(&[u8], u32)> {
        let k = self
            .primary
            .partition_point(|&i| self.labels[i].addr <= addr);
        let i = *self.primary.get(k.checked_sub(1)?)?;
        let l = &self.labels[i];
        let s = self.section_at(addr)?;
        // (A label of another section is no name. And a symbol has no size:
        // past a local one's start, in data, may be data no symbol names,
        // a literal or a table, whose neighbor follows the layout: named
        // by its contents.)
        let past = !s.code()
            && addr != l.addr
            && l.rank <= 1
            && match self.limits.get(&l.name) {
                Some(&lim) => addr - l.addr >= lim,
                None => l.rank == 1,
            };
        if l.addr < s.addr || (l.rank == 3 && addr != l.addr) || past {
            return self.content_name(s, addr);
        }
        let off = u32::try_from(addr - l.addr).ok()?;
        (off < CONTENT).then_some((&l.name, off))
    }

    /// A name by contents for an address in a section that isn't code.
    fn content_name(&self, s: &Sect<'a>, addr: u64) -> Option<(&[u8], u32)> {
        if s.code() || s.zerofill {
            return None;
        }
        let at = usize::try_from(addr - s.addr).ok()?;
        let rest = s.data.get(at..)?;
        let n = rest.len().min(CONTENT_BYTES);
        let n = rest[..n].iter().position(|&b| b == 0).map_or(n, |z| z + 1);
        Some((&rest[..n], CONTENT))
    }

    fn render_of(&self, addr: u64) -> String {
        match self.name_of(addr) {
            Some((n, CONTENT)) => format!("[{}]", show(n)),
            Some((n, 0)) => show(n),
            Some((n, o)) => format!("{}+{o:#x}", show(n)),
            None => format!("{addr:#x}"),
        }
    }

    fn fixes_in(&self, addr: u64, len: u64) -> &[Fix] {
        let lo = self.fixes.partition_point(|f| f.addr < addr);
        let hi = self.fixes.partition_point(|f| f.addr < addr + len);
        &self.fixes[lo..hi]
    }

    fn isa(&self) -> Isa {
        match self.cputype {
            CPU_TYPE_X86_64 => Isa::X64,
            CPU_TYPE_ARM64 => Isa::Arm64,
            _ => Isa::Data,
        }
    }

    /// The DWARF of this image's dSYM (`dsym`'s `__DWARF` sections).
    fn dwarf_of<'d>(dsym: &MachO<'d>) -> Sections<'d> {
        let mut s = Sections::new(turbonop_core::Endian::Little);
        for x in dsym.sections() {
            if x.segname != b"__DWARF" {
                continue;
            }
            let Some(name) = x.sectname.strip_prefix(b"__debug_") else {
                continue;
            };
            let o = x.offset as usize;
            let data = dsym
                .data()
                .get(o..o.saturating_add(x.size as usize))
                .unwrap_or(&[]);
            let name = match name {
                b"str_offs" => &b"str_offsets"[..],
                n => n,
            };
            s.set(name, data);
        }
        s
    }
}

impl Addresses for Image<'_> {
    fn section_kind(&self, addr: u64) -> Option<bool> {
        self.section_at(addr).map(Sect::code)
    }
    fn name(&self, addr: u64) -> Option<(&[u8], u32)> {
        self.name_of(addr)
    }
    fn render(&self, addr: u64) -> String {
        self.render_of(addr)
    }
}

impl Names for Image<'_> {
    fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        self.name_of(self.base + u64::from(rva))
    }
    fn in_code(&self, rva: u32) -> bool {
        self.section_at(self.base + u64::from(rva))
            .is_some_and(Sect::code)
    }
    fn image_base(&self) -> u64 {
        self.base
    }
    fn relocations_in(&self, _rva: u32, _len: u32) -> &[(u32, u8)] {
        // (Fixups are compared, and masked, before.)
        &[]
    }
}

/// The length of `b` without the alignment padding at its end: x86-64's
/// NOPs and `int3`s after its last other instruction (decoded),
/// arm64's `nop`s and zero words.
fn unpadded(b: &[u8], isa: Isa) -> usize {
    match isa {
        Isa::Arm64 => {
            let mut n = b.len() & !3;
            while n >= 4 && matches!(&b[n - 4..n], [0x1f, 0x20, 0x03, 0xd5] | [0, 0, 0, 0]) {
                n -= 4;
            }
            if n == b.len() & !3 { b.len() } else { n }
        }
        Isa::X64 => {
            // Decoded from the start: the end of the last instruction that
            // isn't a NOP (or int3); everything (no trimming) if the bytes
            // don't decode.
            use turbonop_x86::{Mode, decode};
            let mut at = 0;
            let mut end = 0;
            while at < b.len() {
                // (Zeros from an instruction's end on: the linker's padding
                // between sections of other objects.)
                if b[at..].iter().all(|&x| x == 0) {
                    break;
                }
                let Ok(i) = decode(&b[at..], Mode::Bits64, at as u64) else {
                    return b.len();
                };
                let n = i.len();
                if n == 0 {
                    return b.len();
                }
                if !matches!(i.mnemonic().name(), "nop" | "int3") {
                    end = at + n;
                }
                at += n;
            }
            end
        }
        Isa::Data => b.len(),
    }
}

/// The bytes of a region with its fixups' places zeroed, and the fixups as
/// (offset, text).
fn masked<'a>(img: &Image<'_>, addr: u64, bytes: &'a [u8]) -> (Cow<'a, [u8]>, Vec<String>) {
    let fx = img.fixes_in(addr, bytes.len() as u64);
    if fx.is_empty() {
        return (Cow::Borrowed(bytes), Vec::new());
    }
    let mut out = bytes.to_vec();
    let mut list = Vec::with_capacity(fx.len());
    for f in fx {
        let at = (f.addr - addr) as usize;
        let end = (at + usize::from(f.width)).min(out.len());
        out[at..end].fill(0);
        list.push(format!("+{at:#x} {}", f.text(img)));
    }
    (Cow::Owned(out), list)
}

fn region(
    a: &Image<'_>,
    b: &Image<'_>,
    ra: &(Box<[u8]>, u64, u64, bool),
    rb: &(Box<[u8]>, u64, u64, bool),
    rep: &mut Report,
) {
    let key = &ra.0;
    rep.checked("functions and data", 1);
    let isa = if ra.3 { a.isa() } else { Isa::Data };
    let (ba, bb) = (a.bytes(ra.1, ra.2), b.bytes(rb.1, rb.2));
    let (ba, bb) = match (ba, bb) {
        (Some(x), Some(y)) => (x, y),
        (None, None) => {
            // Zero-fill: the sizes only.
            if ra.2 != rb.2 {
                rep.fail("functions or data of another size", || {
                    format!("{}: {:#x} and {:#x}", show(key), ra.2, rb.2)
                });
            }
            return;
        }
        _ => {
            rep.fail("data uninitialized in only one image", || show(key));
            return;
        }
    };
    let (ba, bb) = if ra.3 {
        (&ba[..unpadded(ba, isa)], &bb[..unpadded(bb, isa)])
    } else {
        (ba, bb)
    };
    // (A datum's extent runs to the next symbol, which follows the layout:
    // what lies between, data no symbol names, may differ. Both extents
    // hold the whole datum, so the shorter is compared.)
    let (ba, bb) = if !ra.3 && ba.len() != bb.len() {
        rep.expect("data whose extents (to the next symbol) differ", || {
            format!("{}: {:#x} and {:#x}", show(key), ba.len(), bb.len())
        });
        let n = ba.len().min(bb.len());
        (&ba[..n], &bb[..n])
    } else {
        (ba, bb)
    };
    if ba.len() != bb.len() {
        rep.fail("functions or data of another size", || {
            format!("{}: {:#x} and {:#x}", show(key), ba.len(), bb.len())
        });
        return;
    }
    rep.checked("bytes of functions and data", ba.len() as u64);
    let (ma, la) = masked(a, ra.1, ba);
    let (mb, lb) = masked(b, rb.1, bb);
    rep.checked("fixups in functions and data", la.len() as u64);
    if la != lb {
        rep.fail("functions or data whose fixups name other things", || {
            let at = la.iter().zip(&lb).position(|(x, y)| x != y).unwrap_or(0);
            format!(
                "{}: {} and {}",
                show(key),
                la.get(at).map_or("nothing", String::as_str),
                lb.get(at).map_or("nothing", String::as_str)
            )
        });
        return;
    }
    // Data's PC-relative words (an LSDA's type table, through the GOT):
    // differing 4-byte words that, read relative to themselves, name the
    // same thing in both.
    let (ma, mb) = if ra.3 {
        (ma, mb)
    } else {
        let (mut xa, mut xb) = (ma.into_owned(), mb.into_owned());
        let n = pc_relative_words(a, ra.1, &mut xa, b, rb.1, &mut xb);
        rep.checked("data's PC-relative words naming the same thing", n);
        (Cow::Owned(xa), Cow::Owned(xb))
    };
    let sa = Side {
        image: a,
        rva: (ra.1 - a.base) as u32,
        bytes: &ma,
    };
    let sb = Side {
        image: b,
        rva: (rb.1 - b.base) as u32,
        bytes: &mb,
    };
    let f: Fields = bytes::compare(&sa, &sb, isa, key, rep);
    rep.checked("PC-relative operands matched", f.relative);
    rep.checked("other words read as addresses, matched", f.inferred);
}

/// Zeroes, in both, each differing 4-byte word of `xa` and `xb` (at `aa`
/// and `ab`) that names the same thing read as an offset from itself; how
/// many.
fn pc_relative_words(
    a: &Image<'_>,
    aa: u64,
    xa: &mut [u8],
    b: &Image<'_>,
    ab: u64,
    xb: &mut [u8],
) -> u64 {
    let len = xa.len().min(xb.len());
    let mut n = 0;
    let mut i = 0;
    while i < len {
        if xa[i] == xb[i] {
            i += 1;
            continue;
        }
        let mut found = None;
        for s in i.saturating_sub(3)..=i {
            if s + 4 > len {
                break;
            }
            let wa = i64::from(le(xa, s, 4).unwrap_or(0) as u32 as i32);
            let wb = i64::from(le(xb, s, 4).unwrap_or(0) as u32 as i32);
            let ta = (aa + s as u64).wrapping_add(wa as u64);
            let tb = (ab + s as u64).wrapping_add(wb as u64);
            let (na, nb) = (a.name_of(ta), b.name_of(tb));
            if na.is_some() && na == nb {
                found = Some(s);
                break;
            }
        }
        match found {
            Some(s) => {
                xa[s..s + 4].fill(0);
                xb[s..s + 4].fill(0);
                n += 1;
                i = s + 4;
            }
            None => i += 1,
        }
    }
    n
}

/// `__mod_init_func`'s entries, by what they name.
fn initializers(img: &Image<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for s in &img.sections {
        if s.kind() != S_MOD_INIT_FUNC_POINTERS {
            continue;
        }
        for k in 0..s.size / 8 {
            let at = s.addr + 8 * k;
            let v = le(s.data, 8 * k as usize, 8).unwrap_or(0);
            out.push(match img.fixes_in(at, 8).first() {
                Some(f) => f.text(img),
                None => img.render_of(v),
            });
        }
    }
    out
}

/// Each function's unwind information, by its name: its encoding (a
/// DWARF-mode one's FDE offset out), personality, LSDA, and FDE.
fn unwind(img: &Image<'_>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(u) = img.mach.unwind_info() else {
        return out;
    };
    let eh = img
        .sections
        .iter()
        .find(|s| s.seg == b"__TEXT" && s.name == b"__eh_frame");
    let dwarf_mode = match img.cputype {
        CPU_TYPE_X86_64 => 0x0400_0000,
        _ => 0x0300_0000,
    };
    // Each function's: the entry at or below its start (entries are runs
    // of functions with one encoding, as the layout puts them together).
    let mut entries = u.entries.clone();
    entries.sort_by_key(|e| e.function);
    for (key, addr, _, code) in &img.regions {
        if !code {
            continue;
        }
        let k = entries.partition_point(|e| e.function <= *addr);
        let Some(e) = k.checked_sub(1).map(|k| entries[k]) else {
            continue;
        };
        let mut enc = e.encoding;
        let mut fde = String::new();
        if enc & UNWIND_MODE_MASK == dwarf_mode {
            let off = enc & 0x00FF_FFFF;
            enc &= !0x00FF_FFFF;
            if let Some(eh) = eh {
                fde = canonical_fde(img, eh, off as usize);
            }
        }
        // The personality: its index's GOT entry, by what it names.
        let p = (enc >> 28) & 3;
        let personality = if p > 0 {
            u.personalities
                .get(p as usize - 1)
                .map(|&o| {
                    let at = img.base + u64::from(o);
                    img.fixes_in(at, 8)
                        .first()
                        .map_or_else(|| img.render_of(at), |f| f.text(img))
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let enc = enc & !(3 << 28);
        let lsda = e
            .lsda
            .filter(|_| e.function == *addr)
            .map(|l| img.render_of(l))
            .unwrap_or_default();
        out.insert(show(key), format!("{enc:#x} {personality} {lsda} {fde}"));
    }
    out
}

/// An FDE's call frame instructions and its CIE's (pointers out), as
/// text.
fn canonical_fde(img: &Image<'_>, eh: &Sect<'_>, off: usize) -> String {
    let d = eh.data;
    let Some(len) = le(d, off, 4) else {
        return "?".into();
    };
    let end = off + 4 + len as usize;
    let Some(back) = le(d, off + 4, 4) else {
        return "?".into();
    };
    let cie = (off + 4).wrapping_sub(back as usize);
    let cie_len = le(d, cie, 4).unwrap_or(0) as usize;
    let ptrs = turbonop_macho_object::ehframe::pointers(d, true, turbonop_core::Endian::Little)
        .unwrap_or_default();
    let mut rec = d.get(off..end).unwrap_or(&[]).to_vec();
    let mut cie_bytes = d.get(cie..cie + 4 + cie_len).unwrap_or(&[]).to_vec();
    if rec.len() >= 8 {
        rec[4..8].fill(0);
    }
    let mut named = Vec::new();
    for p in ptrs {
        let (buf, base) = if p.record.0 == off {
            (&mut rec, off)
        } else if p.record.0 == cie {
            (&mut cie_bytes, cie)
        } else {
            continue;
        };
        let at = p.offset - base;
        let n = usize::from(p.size);
        if let Some(x) = buf.get_mut(at..at + n) {
            let v = le(x, 0, n).unwrap_or(0);
            let signed = if n < 8 {
                ((v << (64 - 8 * n)) as i64) >> (64 - 8 * n)
            } else {
                v as i64
            };
            let t = if p.encoding & 0x70 == 0x10 {
                eh.addr
                    .wrapping_add(p.offset as u64)
                    .wrapping_add(signed as u64)
            } else {
                v
            };
            // (The FDE's range and the personality's GOT entry.)
            if p.kind != turbonop_macho_object::ehframe::PointerKind::PcBegin {
                named.push(img.render_of(t));
            }
            x.fill(0);
        }
    }
    if let Some(x) = rec.get_mut(8..16) {
        // pc_begin's value (its pointer is out); pc_range stays.
        let _ = x;
    }
    format!("{} {} {}", hex(&rec), hex(&cie_bytes), named.join(","))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Compares two linked Mach-O images (see the module's documentation); the
/// DWARF of `dsyms` if given (each image's dSYM).
pub fn compare(a: &[u8], b: &[u8], dsyms: Option<(&[u8], &[u8])>) -> Result<Report, String> {
    let (ia, ib) = turbonop_par::join(
        || Image::read(a, "the first image"),
        || Image::read(b, "the second image"),
    );
    let (mut ia, mut ib) = (ia?, ib?);
    // Data's extent bounds, from both.
    let sizes: BTreeMap<&[u8], u64> = ib
        .regions
        .iter()
        .filter(|r| !r.3)
        .map(|r| (&*r.0, r.2))
        .collect();
    let limits: BTreeMap<Box<[u8]>, u64> = ia
        .regions
        .iter()
        .filter(|r| !r.3)
        .filter_map(|r| sizes.get(&*r.0).map(|&s| (r.0.clone(), s.min(r.2))))
        .collect();
    ia.limits.clone_from(&limits);
    ib.limits = limits;
    let mut rep = Report::new();
    if ia.cputype != ib.cputype {
        rep.fail("images of other CPU types", String::new);
    }
    let (mut first, second) = turbonop_par::join(
        || {
            let mut r = Report::new();
            contents(&ia, &ib, &mut r);
            tables(&ia, &ib, &mut r);
            r
        },
        || match dsyms {
            Some((da, db)) => {
                let (Ok(ma), Ok(mb)) = (MachO::parse(da), MachO::parse(db)) else {
                    let mut r = Report::new();
                    r.fail("dSYMs that don't read", String::new);
                    return r;
                };
                let (sa, sb) = (Image::dwarf_of(&ma), Image::dwarf_of(&mb));
                crate::elf::dwarf::compare_with(&ia, sa, &ib, sb)
            }
            None => {
                let mut r = Report::new();
                r.unchecked("DWARF (no dSYMs)", String::new);
                r
            }
        },
    );
    first.merge(second);
    rep.merge(first);
    Ok(rep)
}

fn contents(a: &Image<'_>, b: &Image<'_>, rep: &mut Report) {
    let by_key: BTreeMap<&[u8], &Region> = b.regions.iter().map(|r| (&*r.0, r)).collect();
    let keys_a: BTreeSet<&[u8]> = a.regions.iter().map(|r| &*r.0).collect();
    let mut pairs = Vec::with_capacity(a.regions.len());
    for r in &a.regions {
        match by_key.get(&*r.0) {
            Some(s) => pairs.push((r, *s)),
            None => rep.fail("functions or data only in the first image", || show(&r.0)),
        }
    }
    for r in &b.regions {
        if !keys_a.contains(&*r.0) {
            rep.fail("functions or data only in the second image", || show(&r.0));
        }
    }
    let reports: Vec<Report> = turbonop_par::map_balanced(&pairs, 256, |&(ra, rb)| {
        let mut r = Report::new();
        region(a, b, ra, rb, &mut r);
        r
    });
    for r in reports {
        rep.merge(r);
    }
}

fn tables(a: &Image<'_>, b: &Image<'_>, rep: &mut Report) {
    // Symbols.
    let set = |i: &Image<'_>| -> BTreeMap<(Box<[u8]>, bool, String), usize> {
        let mut m = BTreeMap::new();
        for y in &i.symbols {
            *m.entry(y.clone()).or_default() += 1;
        }
        m
    };
    let (sa, sb) = (set(a), set(b));
    rep.checked("symbols", a.symbols.len() as u64);
    for (k, n) in &sa {
        if sb.get(k) != Some(n) {
            rep.fail(
                "symbols only in the first image (or fewer in the second)",
                || format!("{} ({})", show(&k.0), k.2),
            );
        }
    }
    for (k, n) in &sb {
        if sa.get(k) != Some(n) {
            rep.fail(
                "symbols only in the second image (or fewer in the first)",
                || format!("{} ({})", show(&k.0), k.2),
            );
        }
    }
    // Initializers.
    let (xa, xb) = (initializers(a), initializers(b));
    rep.checked("initializers", xa.len() as u64);
    if xa != xb {
        rep.fail("initializers that differ (or in another order)", || {
            let k = xa
                .iter()
                .zip(&xb)
                .position(|(x, y)| x != y)
                .unwrap_or(xa.len().min(xb.len()));
            format!(
                "at {k}: {} and {}",
                xa.get(k).map_or("nothing", String::as_str),
                xb.get(k).map_or("nothing", String::as_str)
            )
        });
    }
    // Unwind information.
    let (ua, ub) = (unwind(a), unwind(b));
    rep.checked("functions' unwind information", ua.len() as u64);
    for (f, x) in &ua {
        match ub.get(f) {
            Some(y) if y == x => {}
            Some(y) => rep.fail("functions whose unwind information differs", || {
                format!("{f}: {x} and {y}")
            }),
            None => rep.fail("unwind information only in the first image", || f.clone()),
        }
    }
    for f in ub.keys() {
        if !ua.contains_key(f) {
            rep.fail("unwind information only in the second image", || f.clone());
        }
    }
    // Imports.
    rep.checked("imports", a.imports.len() as u64);
    for i in a.imports.symmetric_difference(&b.imports) {
        rep.fail("imports only in one image", || show(&i.0));
    }
    // Section sizes.
    let sizes = |i: &Image<'_>| -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for s in &i.sections {
            *m.entry(s.full_name()).or_default() += s.size;
        }
        m
    };
    let (za, zb) = (sizes(a), sizes(b));
    for (k, v) in &za {
        let w = zb.get(k).copied().unwrap_or(0);
        if w != *v {
            rep.expect("sections of other sizes", || {
                format!("{k}: {v:#x} to {w:#x}")
            });
        }
    }
}
