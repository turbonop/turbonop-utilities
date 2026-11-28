//! Code items (`code_item`): a method's registers, its instructions, and
//! its try blocks with their catch handlers (`encoded_catch_handler_list`).

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;

/// A try block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TryItem {
    /// Its first instruction (in code units).
    pub start_addr: u32,
    /// How many code units it covers.
    pub insn_count: u16,
    /// Its handlers': their offset in the handler list.
    pub handler_off: u16,
}

/// What a try block's exceptions go to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CatchHandler {
    /// Its offset in the handler list.
    pub offset: u32,
    /// The exception types caught (type indices), each with its
    /// handler's address, in order.
    pub handlers: Vec<(u32, u32)>,
    /// The catch-all handler's address, if there is one.
    pub catch_all: Option<u32>,
}

/// A method's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeItem<'d> {
    /// Where it is.
    pub offset: u32,
    /// How many registers it uses.
    pub registers_size: u16,
    /// How many of them are its arguments (the last ones).
    pub ins_size: u16,
    /// How many argument words the calls it makes need at most.
    pub outs_size: u16,
    /// Where its debug information is (0: none).
    pub debug_info_off: u32,
    /// Its instructions: `insns_size` code units of 16 bits, little-endian.
    pub insns: &'d [u8],
    /// Its try blocks.
    pub tries: Vec<TryItem>,
    /// Its handler lists, each with its offset (`handler_off`).
    pub handlers: Vec<CatchHandler>,
    /// Its size in bytes: the header, the instructions, the try blocks,
    /// and the handlers.
    pub size: u32,
}

impl<'d> CodeItem<'d> {
    /// Parses the code item at `off` of `data`.
    pub fn parse(data: &'d [u8], off: u32) -> Result<Self> {
        if off % 4 != 0 {
            return Err(Error::new(
                ErrorKind::Malformed,
                u64::from(off),
                "code item alignment",
            ));
        }
        let mut c = Cursor::at(data, off as usize);
        let registers_size = c.u16("registers_size")?;
        let ins_size = c.u16("ins_size")?;
        let outs_size = c.u16("outs_size")?;
        let tries_size = c.u16("tries_size")?;
        let debug_info_off = c.u32("debug_info_off")?;
        let insns_size = c.u32("insns_size")?;
        let n = c.check_count(insns_size, 2, "instructions")?;
        let insns = c.bytes(n * 2, "instructions")?;
        let mut tries = Vec::new();
        let mut handlers = Vec::new();
        if tries_size != 0 {
            if insns_size % 2 != 0 {
                c.skip(2, "code item padding")?;
            }
            let n = c.check_count(u32::from(tries_size), 8, "try items")?;
            tries.reserve(n);
            for _ in 0..n {
                tries.push(TryItem {
                    start_addr: c.u32("try start_addr")?,
                    insn_count: c.u16("try insn_count")?,
                    handler_off: c.u16("try handler_off")?,
                });
            }
            let list = c.pos();
            let n = c.uleb128("handlers size")?;
            let n = c.check_count(n, 1, "catch handlers")?;
            handlers.reserve(n);
            for _ in 0..n {
                #[allow(clippy::cast_possible_truncation)]
                let offset = (c.pos() - list) as u32;
                let size = c.sleb128("catch handler size")?;
                let typed = c.check_count(size.unsigned_abs(), 2, "catch handler")?;
                let mut h = CatchHandler {
                    offset,
                    handlers: Vec::with_capacity(typed),
                    catch_all: None,
                };
                for _ in 0..typed {
                    let ty = c.uleb128("catch type_idx")?;
                    h.handlers.push((ty, c.uleb128("catch addr")?));
                }
                if size <= 0 {
                    h.catch_all = Some(c.uleb128("catch_all_addr")?);
                }
                handlers.push(h);
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let size = (c.pos() - off as usize) as u32;
        Ok(Self {
            offset: off,
            registers_size,
            ins_size,
            outs_size,
            debug_info_off,
            insns,
            tries,
            handlers,
            size,
        })
    }

    /// How many code units of instructions.
    pub fn insns_size(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let n = (self.insns.len() / 2) as u32;
        n
    }

    /// Where the instructions are in the file.
    pub fn insns_offset(&self) -> u32 {
        self.offset.wrapping_add(16)
    }

    /// The handlers at `handler_off` of the handler list (a try block's).
    pub fn handler(&self, handler_off: u16) -> Option<&CatchHandler> {
        self.handlers
            .iter()
            .find(|h| h.offset == u32::from(handler_off))
    }

    /// The `i`th code unit.
    pub fn unit(&self, i: u32) -> Option<u16> {
        crate::reader::u16_at(self.insns, (i as usize).checked_mul(2)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{encode_sleb128, encode_uleb128};

    fn item(insns: &[u16], tries: &[(u32, u16, u16)], handlers: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 4];
        d.extend_from_slice(&3u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        #[allow(clippy::cast_possible_truncation)]
        d.extend_from_slice(&(tries.len() as u16).to_le_bytes());
        d.extend_from_slice(&0x1234u32.to_le_bytes());
        #[allow(clippy::cast_possible_truncation)]
        d.extend_from_slice(&(insns.len() as u32).to_le_bytes());
        for u in insns {
            d.extend_from_slice(&u.to_le_bytes());
        }
        if !tries.is_empty() && insns.len() % 2 == 1 {
            d.extend_from_slice(&[0, 0]);
        }
        for &(s, n, h) in tries {
            d.extend_from_slice(&s.to_le_bytes());
            d.extend_from_slice(&n.to_le_bytes());
            d.extend_from_slice(&h.to_le_bytes());
        }
        d.extend_from_slice(handlers);
        d
    }

    #[test]
    fn tries_and_handlers() {
        let mut h = Vec::new();
        encode_uleb128(2, &mut h);
        // One typed handler and a catch-all; then a catch-all alone.
        encode_sleb128(-1, &mut h);
        encode_uleb128(7, &mut h);
        encode_uleb128(0x10, &mut h);
        encode_uleb128(0x20, &mut h);
        let second = h.len() as u16;
        encode_sleb128(0, &mut h);
        encode_uleb128(0x30, &mut h);
        let d = item(&[0x0e00, 0, 0], &[(0, 2, 1), (1, 1, second)], &h);
        let code = CodeItem::parse(&d, 4).unwrap();
        assert_eq!(
            (code.registers_size, code.ins_size, code.outs_size, code.debug_info_off),
            (3, 1, 2, 0x1234)
        );
        assert_eq!(code.insns_size(), 3);
        assert_eq!(code.unit(0), Some(0x0e00));
        assert_eq!(code.unit(3), None);
        assert_eq!(code.size as usize, d.len() - 4);
        let first = code.handler(1).unwrap();
        assert_eq!((first.handlers.as_slice(), first.catch_all), (&[(7, 0x10)][..], Some(0x20)));
        let other = code.handler(second).unwrap();
        assert_eq!((other.handlers.len(), other.catch_all), (0, Some(0x30)));
        assert!(code.handler(99).is_none());
        for cut in 4..d.len() {
            assert!(CodeItem::parse(&d[..cut], 4).is_err(), "{cut}");
        }
        assert!(CodeItem::parse(&d, 2).is_err());
    }

    #[test]
    fn no_tries_no_padding() {
        let d = item(&[0x0e00], &[], &[]);
        let code = CodeItem::parse(&d, 4).unwrap();
        assert_eq!(code.size, 18);
        assert!(code.tries.is_empty());
        let mut huge = d.clone();
        huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(CodeItem::parse(&huge, 4).is_err());
    }
}
