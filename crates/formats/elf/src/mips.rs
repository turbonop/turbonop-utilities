//! MIPS's register information: the value of `$gp` the image's code
//! assumes (its small data and GOT are `$gp`-relative), as `.reginfo`
//! (`SHT_MIPS_REGINFO`, o32's: `Elf32_RegInfo`) or `.MIPS.options`
//! (`SHT_MIPS_OPTIONS`, n64's and n32's: an `ODK_REGINFO` option, its
//! `Elf64_RegInfo`) records it, or else the `_gp` symbol says.

use hexcavator_core::Endian;

/// `SHT_MIPS_REGINFO`.
pub const SHT_MIPS_REGINFO: u32 = 0x7000_0006;
/// `SHT_MIPS_OPTIONS`.
pub const SHT_MIPS_OPTIONS: u32 = 0x7000_000D;
/// `ODK_REGINFO`: an option of register information.
pub const ODK_REGINFO: u8 = 1;

fn u32_at(d: &[u8], at: usize, endian: Endian) -> Option<u32> {
    let b: [u8; 4] = d.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(match endian {
        Endian::Little => u32::from_le_bytes(b),
        Endian::Big => u32::from_be_bytes(b),
    })
}

fn u64_at(d: &[u8], at: usize, endian: Endian) -> Option<u64> {
    let b: [u8; 8] = d.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(match endian {
        Endian::Little => u64::from_le_bytes(b),
        Endian::Big => u64::from_be_bytes(b),
    })
}

/// `$gp`'s value in a `.reginfo` section's data: `Elf32_RegInfo`'s
/// `ri_gp_value`, after its masks (`ri_gprmask`, `ri_cprmask[4]`).
pub fn reginfo_gp(data: &[u8], endian: Endian) -> Option<u64> {
    u32_at(data, 20, endian).map(u64::from)
}

/// `$gp`'s value in a `.MIPS.options` section's data: its first
/// `ODK_REGINFO` option's `Elf64_RegInfo`'s `ri_gp_value` (each option a
/// kind, its size with its header, a section, and its information; then
/// `ri_gprmask`, a pad, `ri_cprmask[4]`, and `ri_gp_value`).
pub fn options_gp(data: &[u8], endian: Endian) -> Option<u64> {
    let mut at = 0usize;
    while at.checked_add(8)? <= data.len() {
        let kind = data[at];
        let size = usize::from(data[at + 1]);
        if size < 8 {
            return None;
        }
        if kind == ODK_REGINFO {
            return u64_at(data, at + 8 + 24, endian);
        }
        at = at.checked_add(size)?;
    }
    None
}

impl crate::ElfFile<'_> {
    /// The value of `$gp` a MIPS image's code assumes, if it says: as its
    /// `.reginfo` or `.MIPS.options` records it, or its `_gp` symbol (none
    /// for another machine's image).
    pub fn mips_gp(&self) -> Option<u64> {
        let machine = self.header().machine;
        if machine != crate::consts::EM_MIPS && machine != crate::consts::EM_MIPS_RS3_LE {
            return None;
        }
        let endian = self.endian();
        let sections: alloc::vec::Vec<_> = self.sections().collect();
        let recorded = sections.iter().find_map(|s| match s.kind {
            SHT_MIPS_REGINFO => reginfo_gp(self.section_data(s), endian),
            SHT_MIPS_OPTIONS => options_gp(self.section_data(s), endian),
            _ => None,
        });
        // (0: an object's, not yet linked.)
        recorded.filter(|&gp| gp != 0).or_else(|| {
            sections
                .iter()
                .filter(|s| s.kind == crate::consts::SHT_SYMTAB)
                .flat_map(|s| self.symbols(s))
                .find(|sym| sym.name == b"_gp")
                .map(|sym| sym.value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_information() {
        // Elf32_RegInfo: masks, then gp 0x482e0 (little-endian).
        let mut r = alloc::vec![0u8; 20];
        r.extend_from_slice(&0x0004_82e0u32.to_le_bytes());
        assert_eq!(reginfo_gp(&r, Endian::Little), Some(0x4_82e0));
        assert_eq!(reginfo_gp(&r[..23], Endian::Little), None);
        // An option of another kind (8 bytes), then ODK_REGINFO's (40):
        // big-endian gp 0x1_0000_8000.
        let mut o = alloc::vec![2u8, 8, 0, 0, 0, 0, 0, 0];
        o.extend_from_slice(&[ODK_REGINFO, 40, 0, 0, 0, 0, 0, 0]);
        o.extend_from_slice(&[0; 24]);
        o.extend_from_slice(&0x1_0000_8000u64.to_be_bytes());
        assert_eq!(options_gp(&o, Endian::Big), Some(0x1_0000_8000));
        // Damaged: none, and no panic.
        for n in 0..o.len() {
            let _ = options_gp(&o[..n], Endian::Big);
        }
        assert_eq!(options_gp(&[1, 0, 0, 0, 0, 0, 0, 0], Endian::Big), None);
    }
}
