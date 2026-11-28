//! Encoded values (`encoded_value`, `encoded_array`, `encoded_annotation`):
//! annotation elements, static fields' initial values, and call sites'
//! arguments.

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;

/// The value types (`value_type`, the low five bits of the first byte).
pub mod kind {
    /// A signed byte.
    pub const BYTE: u8 = 0x00;
    /// A signed short.
    pub const SHORT: u8 = 0x02;
    /// An unsigned char.
    pub const CHAR: u8 = 0x03;
    /// A signed int.
    pub const INT: u8 = 0x04;
    /// A signed long.
    pub const LONG: u8 = 0x06;
    /// A float (its high-order bytes).
    pub const FLOAT: u8 = 0x10;
    /// A double (its high-order bytes).
    pub const DOUBLE: u8 = 0x11;
    /// A method type (a proto index).
    pub const METHOD_TYPE: u8 = 0x15;
    /// A method handle (an index).
    pub const METHOD_HANDLE: u8 = 0x16;
    /// A string (an index).
    pub const STRING: u8 = 0x17;
    /// A type (an index).
    pub const TYPE: u8 = 0x18;
    /// A field (an index).
    pub const FIELD: u8 = 0x19;
    /// A method (an index).
    pub const METHOD: u8 = 0x1A;
    /// An enum constant (a field index).
    pub const ENUM: u8 = 0x1B;
    /// An array of values.
    pub const ARRAY: u8 = 0x1C;
    /// An annotation.
    pub const ANNOTATION: u8 = 0x1D;
    /// `null`.
    pub const NULL: u8 = 0x1E;
    /// A boolean (in `value_arg`).
    pub const BOOLEAN: u8 = 0x1F;
}

/// How deep values may nest (arrays in arrays, annotations in
/// annotations) before they're taken for damage: deeper than any
/// compiler writes, shallow enough not to exhaust the stack.
pub const MAX_DEPTH: u32 = 128;

/// A value.
#[derive(Clone, Debug, PartialEq)]
pub enum EncodedValue {
    /// A `byte`.
    Byte(i8),
    /// A `short`.
    Short(i16),
    /// A `char`.
    Char(u16),
    /// An `int`.
    Int(i32),
    /// A `long`.
    Long(i64),
    /// A `float`.
    Float(f32),
    /// A `double`.
    Double(f64),
    /// A method type (a proto index).
    MethodType(u32),
    /// A method handle (an index).
    MethodHandle(u32),
    /// A string (an index).
    String(u32),
    /// A type (an index).
    Type(u32),
    /// A field (an index).
    Field(u32),
    /// A method (an index).
    Method(u32),
    /// An enum constant (a field index).
    Enum(u32),
    /// An array.
    Array(Vec<EncodedValue>),
    /// An annotation.
    Annotation(EncodedAnnotation),
    /// `null`.
    Null,
    /// A `boolean`.
    Boolean(bool),
}

/// An annotation: its type and elements.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedAnnotation {
    /// Its type (a type index).
    pub type_idx: u32,
    /// Its elements: each name (a string index) and value.
    pub elements: Vec<(u32, EncodedValue)>,
}

impl EncodedValue {
    /// Its value type ([`kind`]).
    pub fn kind(&self) -> u8 {
        match self {
            Self::Byte(_) => kind::BYTE,
            Self::Short(_) => kind::SHORT,
            Self::Char(_) => kind::CHAR,
            Self::Int(_) => kind::INT,
            Self::Long(_) => kind::LONG,
            Self::Float(_) => kind::FLOAT,
            Self::Double(_) => kind::DOUBLE,
            Self::MethodType(_) => kind::METHOD_TYPE,
            Self::MethodHandle(_) => kind::METHOD_HANDLE,
            Self::String(_) => kind::STRING,
            Self::Type(_) => kind::TYPE,
            Self::Field(_) => kind::FIELD,
            Self::Method(_) => kind::METHOD,
            Self::Enum(_) => kind::ENUM,
            Self::Array(_) => kind::ARRAY,
            Self::Annotation(_) => kind::ANNOTATION,
            Self::Null => kind::NULL,
            Self::Boolean(_) => kind::BOOLEAN,
        }
    }
}

/// `size` bytes (1 to 8) little-endian, zero-extended.
fn uint(c: &mut Cursor<'_>, size: u32, what: &'static str) -> Result<u64> {
    let b = c.bytes(size as usize, what)?;
    Ok(b.iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x)))
}

/// `size` bytes, sign-extended.
#[allow(clippy::cast_possible_wrap)]
fn int(c: &mut Cursor<'_>, size: u32, what: &'static str) -> Result<i64> {
    let v = uint(c, size, what)?;
    let shift = 64 - 8 * size;
    Ok(((v << shift) as i64) >> shift)
}

/// Reads a value.
pub(crate) fn read_value(c: &mut Cursor<'_>, depth: u32) -> Result<EncodedValue> {
    if depth > MAX_DEPTH {
        return Err(c.error(ErrorKind::Malformed, "encoded value nesting"));
    }
    let start = c.pos();
    let b = c.u8("encoded value")?;
    let (ty, arg) = (b & 0x1F, u32::from(b >> 5));
    let size = arg + 1;
    let bad = || Error::new(ErrorKind::Malformed, start as u64, "encoded value size");
    let fits = |most: u32| if size > most { Err(bad()) } else { Ok(()) };
    #[allow(clippy::cast_possible_truncation)]
    Ok(match ty {
        kind::BYTE => {
            fits(1)?;
            EncodedValue::Byte(int(c, size, "byte value")? as i8)
        }
        kind::SHORT => {
            fits(2)?;
            EncodedValue::Short(int(c, size, "short value")? as i16)
        }
        kind::CHAR => {
            fits(2)?;
            EncodedValue::Char(uint(c, size, "char value")? as u16)
        }
        kind::INT => {
            fits(4)?;
            EncodedValue::Int(int(c, size, "int value")? as i32)
        }
        kind::LONG => EncodedValue::Long(int(c, size, "long value")?),
        kind::FLOAT => {
            fits(4)?;
            // The high-order bytes: zero-extended to the right.
            let v = uint(c, size, "float value")? << (8 * (4 - size));
            EncodedValue::Float(f32::from_bits(v as u32))
        }
        kind::DOUBLE => {
            let v = uint(c, size, "double value")? << (8 * (8 - size));
            EncodedValue::Double(f64::from_bits(v))
        }
        kind::METHOD_TYPE
        | kind::METHOD_HANDLE
        | kind::STRING
        | kind::TYPE
        | kind::FIELD
        | kind::METHOD
        | kind::ENUM => {
            fits(4)?;
            let v = uint(c, size, "index value")? as u32;
            match ty {
                kind::METHOD_TYPE => EncodedValue::MethodType(v),
                kind::METHOD_HANDLE => EncodedValue::MethodHandle(v),
                kind::STRING => EncodedValue::String(v),
                kind::TYPE => EncodedValue::Type(v),
                kind::FIELD => EncodedValue::Field(v),
                kind::METHOD => EncodedValue::Method(v),
                _ => EncodedValue::Enum(v),
            }
        }
        kind::ARRAY => {
            if arg != 0 {
                return Err(bad());
            }
            EncodedValue::Array(read_array(c, depth + 1)?)
        }
        kind::ANNOTATION => {
            if arg != 0 {
                return Err(bad());
            }
            EncodedValue::Annotation(read_annotation(c, depth + 1)?)
        }
        kind::NULL => {
            if arg != 0 {
                return Err(bad());
            }
            EncodedValue::Null
        }
        kind::BOOLEAN => match arg {
            0 => EncodedValue::Boolean(false),
            1 => EncodedValue::Boolean(true),
            _ => return Err(bad()),
        },
        _ => {
            return Err(Error::new(
                ErrorKind::Malformed,
                start as u64,
                "encoded value type",
            ));
        }
    })
}

/// Reads an array (`encoded_array`).
pub(crate) fn read_array(c: &mut Cursor<'_>, depth: u32) -> Result<Vec<EncodedValue>> {
    let n = c.uleb128("encoded array size")?;
    let n = c.check_count(n, 1, "encoded array")?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read_value(c, depth)?);
    }
    Ok(out)
}

/// Reads an annotation (`encoded_annotation`).
pub(crate) fn read_annotation(c: &mut Cursor<'_>, depth: u32) -> Result<EncodedAnnotation> {
    let type_idx = c.uleb128("annotation type_idx")?;
    let n = c.uleb128("annotation size")?;
    let n = c.check_count(n, 2, "annotation elements")?;
    let mut elements = Vec::with_capacity(n);
    for _ in 0..n {
        let name = c.uleb128("annotation element name_idx")?;
        elements.push((name, read_value(c, depth)?));
    }
    Ok(EncodedAnnotation { type_idx, elements })
}

/// The value at `off` of `data`.
pub fn parse_value_at(data: &[u8], off: u32) -> Result<EncodedValue> {
    read_value(&mut Cursor::at(data, off as usize), 0)
}

/// The encoded array (`encoded_array_item`) at `off` of `data`.
pub fn parse_array_at(data: &[u8], off: u32) -> Result<Vec<EncodedValue>> {
    read_array(&mut Cursor::at(data, off as usize), 0)
}

/// The encoded array at `off` of `data`, and its size in bytes.
pub fn parse_array_sized(data: &[u8], off: u32) -> Result<(Vec<EncodedValue>, u32)> {
    let mut c = Cursor::at(data, off as usize);
    let v = read_array(&mut c, 0)?;
    #[allow(clippy::cast_possible_truncation)]
    Ok((v, (c.pos() - off as usize) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(bytes: &[u8]) -> Result<EncodedValue> {
        parse_value_at(bytes, 0)
    }

    #[test]
    fn every_kind() {
        assert_eq!(value(&[0x00, 0xFF]).unwrap(), EncodedValue::Byte(-1));
        assert_eq!(value(&[0x22, 0x00, 0x80]).unwrap(), EncodedValue::Short(-32768));
        // Shorter than their type: sign- or zero-extended.
        assert_eq!(value(&[0x02, 0x80]).unwrap(), EncodedValue::Short(-128));
        assert_eq!(value(&[0x03, 0x80]).unwrap(), EncodedValue::Char(0x80));
        assert_eq!(value(&[0x44, 0x34, 0x12, 0xFF]).unwrap(), EncodedValue::Int(-0xEDCC));
        assert_eq!(value(&[0x06, 0xFE]).unwrap(), EncodedValue::Long(-2));
        assert_eq!(
            value(&[0xE6, 1, 2, 3, 4, 5, 6, 7, 0x80]).unwrap(),
            EncodedValue::Long(i64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 0x80]))
        );
        // Floats keep their high-order bytes: 1.0f is 3f800000.
        assert_eq!(value(&[0x30, 0x80, 0x3F]).unwrap(), EncodedValue::Float(1.0));
        assert_eq!(value(&[0x31, 0xF0, 0x3F]).unwrap(), EncodedValue::Double(1.0));
        assert_eq!(value(&[0x15, 7]).unwrap(), EncodedValue::MethodType(7));
        assert_eq!(value(&[0x36, 1, 0]).unwrap(), EncodedValue::MethodHandle(1));
        assert_eq!(value(&[0x77, 1, 2, 3, 4]).unwrap(), EncodedValue::String(0x0403_0201));
        assert_eq!(value(&[0x18, 2]).unwrap(), EncodedValue::Type(2));
        assert_eq!(value(&[0x19, 3]).unwrap(), EncodedValue::Field(3));
        assert_eq!(value(&[0x1A, 4]).unwrap(), EncodedValue::Method(4));
        assert_eq!(value(&[0x1B, 5]).unwrap(), EncodedValue::Enum(5));
        assert_eq!(
            value(&[0x1C, 2, 0x1E, 0x3F]).unwrap(),
            EncodedValue::Array(vec![EncodedValue::Null, EncodedValue::Boolean(true)])
        );
        assert_eq!(
            value(&[0x1D, 9, 1, 4, 0x1F]).unwrap(),
            EncodedValue::Annotation(EncodedAnnotation {
                type_idx: 9,
                elements: vec![(4, EncodedValue::Boolean(false))]
            })
        );
        for bytes in [&[0x00, 1][..], &[0x1E], &[0x1C, 0], &[0x3F], &[0x15, 0], &[0x16, 0]] {
            assert_eq!(value(bytes).unwrap().kind(), bytes[0] & 0x1F);
        }
    }

    #[test]
    fn damaged_values() {
        for bad in [
            &[0x20, 0, 0][..], // a byte of two bytes
            &[0x63, 0, 0, 0, 0], // a char of four
            &[0x84, 0, 0, 0, 0, 0], // an int of five
            &[0x70, 0, 0, 0, 0, 0], // a float of four, ok, then ... (below)
            &[0x05],             // no such type
            &[0x3E],             // null with an argument
            &[0x5F],             // boolean 2
            &[0x3C, 0],          // array with an argument
            &[0x1C, 0xFF, 0xFF, 0xFF, 0x0F], // a count beyond the data
            &[0x06],             // truncated
        ] {
            if bad == [0x70, 0, 0, 0, 0, 0] {
                assert!(value(bad).is_ok());
                continue;
            }
            assert!(value(bad).is_err(), "{bad:?}");
        }
        // Nesting beyond MAX_DEPTH.
        let mut deep = vec![0x1C, 1].repeat(MAX_DEPTH as usize + 2);
        deep.push(0x1E);
        assert_eq!(value(&deep).unwrap_err().what, "encoded value nesting");
        let mut ok = vec![0x1C, 1].repeat(10);
        ok.push(0x1E);
        assert!(value(&ok).is_ok());
    }
}
