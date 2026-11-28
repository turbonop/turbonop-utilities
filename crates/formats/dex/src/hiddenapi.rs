//! Hidden-API flags (`hiddenapi_class_data_item`): which of the platform's
//! fields and methods apps may use, written into the boot class path's DEX
//! files by the platform build's `hiddenapi` tool.

use turbonop_core::{Error, ErrorKind, Result};

use crate::ClassData;
use crate::reader::{Cursor, u32_at};

/// The restriction values (the flags' low three bits), as ART names them.
pub const API_LIST_NAMES: [&str; 8] = [
    "sdk",
    "unsupported",
    "blocked",
    "max-target-o",
    "max-target-p",
    "max-target-q",
    "max-target-r",
    "max-target-s",
];

/// The domain flags (the bits above the value), as ART names them.
pub const DOMAIN_NAMES: [(u32, &str); 2] = [(1 << 3, "core-platform-api"), (1 << 4, "test-api")];

/// The flags' names, `,`-separated (`blocked,core-platform-api`); bits
/// that have none in hex.
pub fn flag_names(flags: u32) -> String {
    let mut names = vec![API_LIST_NAMES[(flags & 7) as usize].to_string()];
    let mut rest = flags & !7;
    for (bit, name) in DOMAIN_NAMES {
        if rest & bit != 0 {
            names.push(name.to_string());
            rest &= !bit;
        }
    }
    if rest != 0 {
        names.push(format!("{rest:#x}"));
    }
    names.join(",")
}

/// A DEX file's hidden-API flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HiddenApi<'d> {
    /// Where the section is.
    pub offset: u32,
    /// The section's bytes (`size` of them, from its start).
    data: &'d [u8],
    /// How many classes it has offsets for.
    class_count: u32,
}

impl<'d> HiddenApi<'d> {
    /// Parses the section at `off` of `data`, for `class_count` classes.
    pub fn parse(data: &'d [u8], off: u32, class_count: u32) -> Result<Self> {
        let mut c = Cursor::at(data, off as usize);
        let size = c.u32("hiddenapi size")?;
        let bytes = Cursor::at(data, off as usize).bytes(size as usize, "hiddenapi class data")?;
        let need = u64::from(class_count) * 4 + 4;
        if u64::from(size) < need {
            return Err(Error::new(
                ErrorKind::Malformed,
                u64::from(off),
                "hiddenapi size",
            ));
        }
        Ok(Self {
            offset: off,
            data: bytes,
            class_count,
        })
    }

    /// The section's size in bytes.
    pub fn size(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let n = self.data.len() as u32;
        n
    }

    /// Where class `class_def_idx`'s flags are in the section (0: it has
    /// none).
    pub fn class_offset(&self, class_def_idx: u32) -> Result<u32> {
        if class_def_idx >= self.class_count {
            return Err(Error::new(
                ErrorKind::Malformed,
                u64::from(class_def_idx),
                "hiddenapi class index",
            ));
        }
        u32_at(self.data, 4 + class_def_idx as usize * 4).ok_or(Error::new(
            ErrorKind::Truncated,
            u64::from(self.offset),
            "hiddenapi offsets",
        ))
    }

    /// Class `class_def_idx`'s flags, if it has them: one for each of its
    /// fields (static, then instance) and methods (direct, then virtual),
    /// in its class data's order.
    pub fn class_flags(&self, class_def_idx: u32, class_data: &ClassData) -> Result<Option<Vec<u32>>> {
        let at = self.class_offset(class_def_idx)?;
        if at == 0 {
            return Ok(None);
        }
        let n = class_data.static_fields.len()
            + class_data.instance_fields.len()
            + class_data.direct_methods.len()
            + class_data.virtual_methods.len();
        let mut c = Cursor::at(self.data, at as usize);
        #[allow(clippy::cast_possible_truncation)]
        let n = c.check_count(n as u32, 1, "hiddenapi flags")?;
        let mut flags = Vec::with_capacity(n);
        for _ in 0..n {
            flags.push(c.uleb128("hiddenapi flags").map_err(|e| {
                Error::new(e.kind, e.offset + u64::from(self.offset), e.what)
            })?);
        }
        Ok(Some(flags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EncodedField, EncodedMethod};

    #[test]
    fn flags_by_class() {
        let mut d = vec![0u8; 4];
        // Two classes: the first has none, the second's at 12.
        for v in [16u32, 0, 12] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0x02, 0x09, 0x80, 0x01]);
        let h = HiddenApi::parse(&d, 4, 2).unwrap();
        assert_eq!(h.size(), 16);
        let cd = ClassData {
            static_fields: vec![EncodedField { field_idx: 0, access_flags: 0 }],
            virtual_methods: vec![
                EncodedMethod { method_idx: 0, access_flags: 0, code_off: 0 },
                EncodedMethod { method_idx: 1, access_flags: 0, code_off: 0 },
            ],
            ..ClassData::default()
        };
        assert_eq!(h.class_flags(0, &cd).unwrap(), None);
        assert_eq!(h.class_flags(1, &cd).unwrap(), Some(vec![2, 9, 0x80]));
        assert!(h.class_flags(2, &cd).is_err());
        assert!(HiddenApi::parse(&d, 4, 4).is_err());
        assert!(HiddenApi::parse(&d[..10], 4, 2).is_err());
        assert_eq!(flag_names(0), "sdk");
        assert_eq!(flag_names(2 | 8), "blocked,core-platform-api");
        assert_eq!(flag_names(16 | 7 | 0x100), "max-target-s,test-api,0x100");
    }
}
