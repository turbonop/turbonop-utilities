//! Class data (`class_data_item`): a class's static and instance fields
//! and its direct and virtual methods, as `uleb128`s: each index a
//! difference from the one before it in its list.

use turbonop_core::Result;

use crate::reader::Cursor;

/// A field a class defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EncodedField {
    /// Its field reference (the index, the differences added up).
    pub field_idx: u32,
    /// Its access flags.
    pub access_flags: u32,
}

/// A method a class defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EncodedMethod {
    /// Its method reference (the index, the differences added up).
    pub method_idx: u32,
    /// Its access flags.
    pub access_flags: u32,
    /// Where its code item is (0: abstract or native, no code).
    pub code_off: u32,
}

/// A class's fields and methods.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassData {
    /// Where it is.
    pub offset: u32,
    /// Its size in bytes.
    pub size: u32,
    /// Its static fields, in order.
    pub static_fields: Vec<EncodedField>,
    /// Its instance fields.
    pub instance_fields: Vec<EncodedField>,
    /// Its direct methods (static, private, constructors).
    pub direct_methods: Vec<EncodedMethod>,
    /// Its virtual methods.
    pub virtual_methods: Vec<EncodedMethod>,
}

impl ClassData {
    /// Parses the class data at `off` of `data`.
    pub fn parse(data: &[u8], off: u32) -> Result<Self> {
        let mut c = Cursor::at(data, off as usize);
        let sizes = [
            c.uleb128("static_fields_size")?,
            c.uleb128("instance_fields_size")?,
            c.uleb128("direct_methods_size")?,
            c.uleb128("virtual_methods_size")?,
        ];
        let fields = |c: &mut Cursor<'_>, n: u32| -> Result<Vec<EncodedField>> {
            // (Two bytes each at least.)
            let n = c.check_count(n, 2, "encoded fields")?;
            let mut out = Vec::with_capacity(n);
            let mut idx = 0u32;
            for _ in 0..n {
                idx = idx.wrapping_add(c.uleb128("field_idx_diff")?);
                out.push(EncodedField {
                    field_idx: idx,
                    access_flags: c.uleb128("field access_flags")?,
                });
            }
            Ok(out)
        };
        let static_fields = fields(&mut c, sizes[0])?;
        let instance_fields = fields(&mut c, sizes[1])?;
        let methods = |c: &mut Cursor<'_>, n: u32| -> Result<Vec<EncodedMethod>> {
            let n = c.check_count(n, 3, "encoded methods")?;
            let mut out = Vec::with_capacity(n);
            let mut idx = 0u32;
            for _ in 0..n {
                idx = idx.wrapping_add(c.uleb128("method_idx_diff")?);
                out.push(EncodedMethod {
                    method_idx: idx,
                    access_flags: c.uleb128("method access_flags")?,
                    code_off: c.uleb128("code_off")?,
                });
            }
            Ok(out)
        };
        let direct_methods = methods(&mut c, sizes[2])?;
        let virtual_methods = methods(&mut c, sizes[3])?;
        #[allow(clippy::cast_possible_truncation)]
        let size = (c.pos() - off as usize) as u32;
        Ok(Self {
            offset: off,
            size,
            static_fields,
            instance_fields,
            direct_methods,
            virtual_methods,
        })
    }

    /// The fields: static, then instance.
    pub fn fields(&self) -> impl Iterator<Item = &EncodedField> {
        self.static_fields.iter().chain(&self.instance_fields)
    }

    /// The methods: direct, then virtual.
    pub fn methods(&self) -> impl Iterator<Item = &EncodedMethod> {
        self.direct_methods.iter().chain(&self.virtual_methods)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::encode_uleb128;

    #[test]
    fn indices_are_differences_within_each_list() {
        let mut d = vec![0xAA; 3];
        for v in [1, 2, 1, 1] {
            encode_uleb128(v, &mut d);
        }
        // Static field 5; instance fields 2, 2+7 = 9.
        for v in [5, 0x19, 2, 2, 7, 0x10] {
            encode_uleb128(v, &mut d);
        }
        // Direct method 300 (code at 0x1000); virtual method 4.
        for v in [300, 0x1_0008, 0x1000, 4, 0x401, 0] {
            encode_uleb128(v, &mut d);
        }
        let cd = ClassData::parse(&d, 3).unwrap();
        assert_eq!(cd.size as usize, d.len() - 3);
        assert_eq!(
            cd.fields()
                .map(|f| (f.field_idx, f.access_flags))
                .collect::<Vec<_>>(),
            [(5, 0x19), (2, 2), (9, 0x10)]
        );
        assert_eq!(
            cd.methods()
                .map(|m| (m.method_idx, m.access_flags, m.code_off))
                .collect::<Vec<_>>(),
            [(300, 0x1_0008, 0x1000), (4, 0x401, 0)]
        );
        for cut in 3..d.len() {
            assert!(ClassData::parse(&d[..cut], 3).is_err(), "{cut}");
        }
        // Counts beyond the data don't allocate.
        let mut big = Vec::new();
        for v in [u32::MAX, 0, 0, 0] {
            encode_uleb128(v, &mut big);
        }
        assert!(ClassData::parse(&big, 0).is_err());
    }
}
