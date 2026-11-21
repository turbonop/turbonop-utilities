//! RISC-V's build attributes (`.riscv.attributes`, `SHT_RISCV_ATTRIBUTES`):
//! what the code was compiled for, as the psABI lays it out (the generic
//! attributes format: `A`, then subsections, each of a vendor's, `riscv`;
//! in it, the file's attributes, each a tag and a value: a string for
//! odd tags, a ULEB128 for even ones, as the psABI says of the tags it
//! doesn't name).

use alloc::vec::Vec;

use hexcavator_core::Endian;

/// `Tag_RISCV_stack_align`: the stack's alignment, in bytes.
pub const TAG_STACK_ALIGN: u64 = 4;
/// `Tag_RISCV_arch`: the ISA string (`rv64i2p1_m2p0_a2p1_c2p0`).
pub const TAG_ARCH: u64 = 5;
/// `Tag_RISCV_unaligned_access`: whether code may access memory unaligned.
pub const TAG_UNALIGNED_ACCESS: u64 = 6;
/// `Tag_RISCV_priv_spec`, `_minor`, `_revision`: the privileged spec's
/// version.
pub const TAG_PRIV_SPEC: u64 = 8;
pub const TAG_PRIV_SPEC_MINOR: u64 = 10;
pub const TAG_PRIV_SPEC_REVISION: u64 = 12;
/// `Tag_RISCV_atomic_abi`: the atomics' mapping (0 unknown, 1 A6C, 2 A6S, 3
/// A7).
pub const TAG_ATOMIC_ABI: u64 = 14;
/// `Tag_RISCV_x3_reg_usage`: what x3 (`gp`) is for.
pub const TAG_X3_REG_USAGE: u64 = 16;

/// An attribute's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value<'d> {
    Int(u64),
    Str(&'d [u8]),
}

/// An attribute: its tag and value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute<'d> {
    pub tag: u64,
    pub value: Value<'d>,
}

/// The most attributes read.
const MAX: usize = 1 << 12;

fn uleb(d: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *d.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= u64::from(b & 0x7F) << shift;
        }
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 70 {
            return None;
        }
    }
}

fn ntbs<'d>(d: &'d [u8], at: &mut usize) -> Option<&'d [u8]> {
    let rest = d.get(*at..)?;
    let n = rest.iter().position(|&b| b == 0)?;
    *at += n + 1;
    Some(&rest[..n])
}

/// The file's attributes in `data` (an `SHT_RISCV_ATTRIBUTES` section's),
/// `riscv`'s; what is damaged ends them.
pub fn attributes(data: &[u8], endian: Endian) -> Vec<Attribute<'_>> {
    let mut out = Vec::new();
    if data.first() != Some(&b'A') {
        return out;
    }
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(match endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    };
    let mut at = 1usize;
    while at < data.len() && out.len() < MAX {
        let Some(len) = u32_at(at) else { break };
        let end = at.saturating_add(len as usize).min(data.len());
        if len < 4 {
            break;
        }
        let mut p = at + 4;
        let vendor = ntbs(data, &mut p);
        if vendor == Some(&b"riscv"[..]) {
            // Its sub-subsections: a tag (1, the file's), and a size.
            while p < end && out.len() < MAX {
                let start = p;
                let Some(tag) = uleb(data, &mut p) else { break };
                let Some(size) = u32_at(p) else { break };
                p += 4;
                let sub_end = start.saturating_add(size as usize).min(end);
                if tag != 1 {
                    p = sub_end;
                    continue;
                }
                let sub = &data[..sub_end];
                while p < sub_end && out.len() < MAX {
                    let Some(tag) = uleb(sub, &mut p) else { break };
                    let value = if tag % 2 == 1 {
                        match ntbs(sub, &mut p) {
                            Some(s) => Value::Str(s),
                            None => break,
                        }
                    } else {
                        match uleb(sub, &mut p) {
                            Some(v) => Value::Int(v),
                            None => break,
                        }
                    };
                    out.push(Attribute { tag, value });
                }
                p = sub_end;
            }
        }
        at = end;
    }
    out
}

impl<'data> crate::ElfFile<'data> {
    /// A RISC-V file's build attributes (none for another machine's, or
    /// a file without them).
    pub fn riscv_attributes(&self) -> Vec<Attribute<'data>> {
        if self.header().machine != crate::consts::EM_RISCV {
            return Vec::new();
        }
        self.sections()
            .find(|s| s.kind == crate::consts::SHT_ARM_ATTRIBUTES)
            .map(|s| attributes(self.section_data(&s), self.endian()))
            .unwrap_or_default()
    }

    /// The ISA string a RISC-V file's attributes name
    /// (`rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0_zicsr2p0`), if they do.
    pub fn riscv_arch(&self) -> Option<&'data str> {
        self.riscv_attributes()
            .into_iter()
            .find_map(|a| match a.value {
                Value::Str(s) if a.tag == TAG_ARCH => core::str::from_utf8(s).ok(),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::cast_possible_truncation, reason = "a small test section")]
    fn reads_attributes() {
        // `A`, a subsection (length 31, `riscv`), the file's (tag 1,
        // size 20): stack_align 16, arch "rv32imc", priv_spec 1.
        let mut d = alloc::vec![b'A'];
        let mut sub = alloc::vec![1u8];
        let attrs: &[u8] = &[4, 16, 5, b'r', b'v', b'3', b'2', b'i', b'm', b'c', 0, 8, 1];
        sub.extend_from_slice(&((5 + attrs.len()) as u32).to_le_bytes());
        sub.extend_from_slice(attrs);
        let mut section = b"riscv\0".to_vec();
        section.extend_from_slice(&sub);
        d.extend_from_slice(&((4 + section.len()) as u32).to_le_bytes());
        d.extend_from_slice(&section);
        let a = attributes(&d, Endian::Little);
        assert_eq!(
            a,
            [
                Attribute {
                    tag: TAG_STACK_ALIGN,
                    value: Value::Int(16)
                },
                Attribute {
                    tag: TAG_ARCH,
                    value: Value::Str(b"rv32imc")
                },
                Attribute {
                    tag: TAG_PRIV_SPEC,
                    value: Value::Int(1)
                },
            ]
        );
        // Damaged anywhere: what was read before.
        for n in 0..d.len() {
            let _ = attributes(&d[..n], Endian::Little);
        }
        assert!(attributes(b"B", Endian::Little).is_empty());
    }
}
