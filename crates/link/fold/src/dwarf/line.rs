//! Line programs as rows: decoded from a line table's program, and encoded
//! again as LLVM's `MCDwarfLineAddr` encodes them (so a program LLVM wrote,
//! decoded and encoded unchanged, is the same bytes), for programs whose
//! addresses move by different amounts (code cut out of a section) or
//! whose rows are taken apart (a moved function's rows).

use std::ops::Range;

use turbonop_dwarf::Reader;

/// A line table's header fields that the program depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Params {
    pub version: u16,
    pub address_size: u8,
    pub min_inst: u8,
    pub max_ops: u8,
    pub default_is_stmt: bool,
    pub line_base: i8,
    pub line_range: u8,
    pub opcode_base: u8,
    /// `standard_opcode_lengths`.
    pub lengths: Vec<u8>,
}

/// A line table of a section: where it lies and what its program depends
/// on.
#[derive(Clone, Debug)]
pub(crate) struct Table {
    /// The whole table, its length field included.
    pub range: Range<usize>,
    /// Its program.
    pub program: Range<usize>,
    pub params: Params,
}

/// A row of the line-number matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub address: u64,
    pub file: u64,
    pub line: u64,
    pub column: u64,
    pub is_stmt: bool,
    pub basic_block: bool,
    pub end_sequence: bool,
    pub prologue_end: bool,
    pub epilogue_begin: bool,
    pub isa: u64,
    pub discriminator: u64,
}

impl Row {
    fn start(p: &Params) -> Row {
        Row {
            address: 0,
            file: 1,
            line: 1,
            column: 0,
            is_stmt: p.default_is_stmt,
            basic_block: false,
            end_sequence: false,
            prologue_end: false,
            epilogue_begin: false,
            isa: 0,
            discriminator: 0,
        }
    }
}

/// A sequence: its rows (the last its `end_sequence`), and where its first
/// `DW_LNE_set_address`'s operand lies in the section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sequence {
    pub rows: Vec<Row>,
    pub set_address: Option<usize>,
}

type R<T> = Result<T, &'static str>;

const BAD: &str = "a line table that doesn't read";

/// The line table at `at` of `data` (`.debug_line`'s bytes): 32-bit DWARF,
/// versions 2 to 5. `address_size` is the unit's (DWARF 5's header says).
pub(crate) fn table(
    data: &[u8],
    at: usize,
    address_size: u8,
    endian: turbonop_core::Endian,
) -> R<Table> {
    let mut r = Reader::new(data, at, endian);
    let (_, end, dwarf64) = r.initial_length().ok_or(BAD)?;
    if dwarf64 {
        return Err("64-bit DWARF");
    }
    if end > data.len() {
        return Err(BAD);
    }
    let version = u16::try_from(r.uint(2).ok_or(BAD)?).map_err(|_| BAD)?;
    if !(2..=5).contains(&version) {
        return Err(BAD);
    }
    let mut asz = address_size;
    if version >= 5 {
        asz = r.u8().ok_or(BAD)?;
        r.u8().ok_or(BAD)?;
    }
    let header_length = r.uint(4).ok_or(BAD)? as usize;
    let program = r.pos.checked_add(header_length).ok_or(BAD)?;
    let min_inst = r.u8().ok_or(BAD)?;
    let max_ops = if version >= 4 { r.u8().ok_or(BAD)? } else { 1 };
    let default_is_stmt = r.u8().ok_or(BAD)? != 0;
    let line_base = r.u8().ok_or(BAD)? as i8;
    let line_range = r.u8().ok_or(BAD)?;
    let opcode_base = r.u8().ok_or(BAD)?;
    let mut lengths = Vec::new();
    for _ in 1..opcode_base {
        lengths.push(r.u8().ok_or(BAD)?);
    }
    if program > end || line_range == 0 || opcode_base == 0 {
        return Err(BAD);
    }
    Ok(Table {
        range: at..end,
        program: program..end,
        params: Params {
            version,
            address_size: asz,
            min_inst,
            max_ops,
            default_is_stmt,
            line_base,
            line_range,
            opcode_base,
            lengths,
        },
    })
}

/// The standard opcodes' operand counts this module expects (DWARF 3's
/// and later), which LLVM writes.
const STANDARD: [u8; 12] = [0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1];

/// Decodes a table's program into sequences. `address` reads a
/// `DW_LNE_set_address` operand at an offset of the section (the caller's
/// relocations decide it); `None` for one that doesn't read.
pub(crate) fn decode(
    data: &[u8],
    t: &Table,
    endian: turbonop_core::Endian,
    address: &dyn Fn(usize, u8) -> Option<u64>,
) -> R<Vec<Sequence>> {
    let p = &t.params;
    if p.max_ops != 1 {
        return Err("a line table for VLIW code");
    }
    // Opcodes with the operands this module knows.
    let known = p.lengths.len().min(STANDARD.len());
    if p.lengths[..known] != STANDARD[..known] {
        return Err("a line table's opcodes of other lengths");
    }
    let mut r = Reader::new(&data[..t.program.end], t.program.start, endian);
    let mut out = Vec::new();
    let mut rows = Vec::new();
    let mut set_address = None;
    let mut s = Row::start(p);
    let min = u64::from(p.min_inst);
    let line_range = u64::from(p.line_range);
    let emit = |s: &mut Row, rows: &mut Vec<Row>| {
        rows.push(*s);
        s.basic_block = false;
        s.prologue_end = false;
        s.epilogue_begin = false;
        s.discriminator = 0;
    };
    while r.pos < t.program.end {
        let op = r.u8().ok_or(BAD)?;
        if op >= p.opcode_base {
            let adjusted = u64::from(op - p.opcode_base);
            s.address = s.address.wrapping_add((adjusted / line_range) * min);
            s.line = s
                .line
                .wrapping_add((i64::from(p.line_base) + (adjusted % line_range) as i64) as u64);
            emit(&mut s, &mut rows);
            continue;
        }
        match op {
            0 => {
                let len = r.uleb().ok_or(BAD)? as usize;
                let at = r.pos;
                let end = at.checked_add(len).ok_or(BAD)?;
                if len == 0 || end > t.program.end {
                    return Err(BAD);
                }
                let sub = r.u8().ok_or(BAD)?;
                match sub {
                    1 => {
                        s.end_sequence = true;
                        rows.push(s);
                        out.push(Sequence {
                            rows: std::mem::take(&mut rows),
                            set_address: set_address.take(),
                        });
                        s = Row::start(p);
                    }
                    2 => {
                        let n = (len - 1) as u8;
                        if n != p.address_size {
                            return Err("a DW_LNE_set_address of another size");
                        }
                        if set_address.is_none() && rows.is_empty() {
                            set_address = Some(r.pos);
                        } else {
                            return Err("a DW_LNE_set_address inside a sequence");
                        }
                        s.address = address(r.pos, n).ok_or(BAD)?;
                    }
                    4 => s.discriminator = r.uleb().ok_or(BAD)?,
                    _ => return Err("a line program's extended opcode not known"),
                }
                r.pos = end;
            }
            1 => emit(&mut s, &mut rows),
            2 => {
                let v = r.uleb().ok_or(BAD)?;
                s.address = s.address.wrapping_add(v.wrapping_mul(min));
            }
            3 => {
                let v = r.sleb().ok_or(BAD)?;
                s.line = s.line.wrapping_add(v as u64);
            }
            4 => s.file = r.uleb().ok_or(BAD)?,
            5 => s.column = r.uleb().ok_or(BAD)?,
            6 => s.is_stmt = !s.is_stmt,
            7 => s.basic_block = true,
            8 => {
                let adjusted = u64::from(255 - p.opcode_base);
                s.address = s.address.wrapping_add((adjusted / line_range) * min);
            }
            9 => {
                let v = r.uint(2).ok_or(BAD)?;
                s.address = s.address.wrapping_add(v);
            }
            10 => s.prologue_end = true,
            11 => s.epilogue_begin = true,
            12 => s.isa = r.uleb().ok_or(BAD)?,
            _ => return Err("a line program's standard opcode not known"),
        }
    }
    if !rows.is_empty() {
        return Err("a line program's sequence without its end");
    }
    Ok(out)
}

fn uleb(out: &mut Vec<u8>, v: u64) {
    crate::elf::put_uleb(out, v);
}

fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        out.push(if done { b } else { b | 0x80 });
        if done {
            return;
        }
    }
}

/// `MCDwarfLineAddr::encode`: a line delta (`None`: the end of the
/// sequence) and an address delta (in units of the minimum instruction
/// length).
fn advance(p: &Params, line: Option<i64>, addr: u64, out: &mut Vec<u8>) {
    let max_special = u64::from((255 - p.opcode_base) / p.line_range);
    let Some(mut line) = line else {
        if addr == max_special {
            out.push(8);
        } else if addr != 0 {
            out.push(2);
            uleb(out, addr);
        }
        out.extend_from_slice(&[0, 1, 1]);
        return;
    };
    let base = i64::from(p.line_base);
    let range = i64::from(p.line_range);
    let opcode_base = i64::from(p.opcode_base);
    let mut temp = line - base;
    let mut need_copy = false;
    if temp >= range || temp + opcode_base > 255 || temp < 0 {
        out.push(3);
        sleb(out, line);
        line = 0;
        temp = -base;
        need_copy = true;
    }
    if line == 0 && addr == 0 {
        out.push(1);
        return;
    }
    temp += opcode_base;
    if addr < 256 + max_special {
        let opcode = temp + addr as i64 * range;
        if opcode <= 255 {
            out.push(opcode as u8);
            return;
        }
        let opcode = temp + (addr as i64 - max_special as i64) * range;
        if opcode <= 255 {
            out.push(8);
            out.push(opcode as u8);
            return;
        }
    }
    out.push(2);
    uleb(out, addr);
    if need_copy {
        out.push(1);
    } else {
        out.push(temp as u8);
    }
}

/// Encodes `rows` (each sequence's, the last its end) as LLVM writes a
/// program: each sequence a `DW_LNE_set_address` of its first row's
/// address, then its rows. The bytes, and each sequence's
/// `DW_LNE_set_address` operand's offset in them. Fails on rows whose
/// addresses go back, or advance by other than multiples of the minimum
/// instruction length.
pub(crate) fn encode(
    p: &Params,
    sequences: &[&[Row]],
    endian: turbonop_core::Endian,
) -> R<(Vec<u8>, Vec<usize>)> {
    let mut out = Vec::new();
    let mut at = Vec::with_capacity(sequences.len());
    let min = u64::from(p.min_inst.max(1));
    for rows in sequences {
        let Some(first) = rows.first() else { continue };
        let n = usize::from(p.address_size);
        // (LLVM sets the address after the first row's other changes.)
        let set_address = |out: &mut Vec<u8>, at: &mut Vec<usize>| {
            out.extend_from_slice(&[0, p.address_size + 1, 2]);
            at.push(out.len());
            let b = match endian {
                turbonop_core::Endian::Little => first.address.to_le_bytes(),
                turbonop_core::Endian::Big => first.address.to_be_bytes(),
            };
            match endian {
                turbonop_core::Endian::Little => out.extend_from_slice(&b[..n]),
                turbonop_core::Endian::Big => out.extend_from_slice(&b[8 - n..]),
            }
        };
        let mut last = Row::start(p);
        last.address = first.address;
        let mut started = false;
        for row in *rows {
            let delta = row
                .address
                .checked_sub(last.address)
                .ok_or("line rows whose addresses go back")?;
            if delta % min != 0 {
                return Err("a line row's address between instructions");
            }
            if row.end_sequence {
                if !started {
                    set_address(&mut out, &mut at);
                }
                advance(p, None, delta / min, &mut out);
                break;
            }
            if row.file != last.file {
                out.push(4);
                uleb(&mut out, row.file);
            }
            if row.column != last.column {
                out.push(5);
                uleb(&mut out, row.column);
            }
            if row.discriminator != 0 && p.version >= 4 {
                let mut d = Vec::new();
                uleb(&mut d, row.discriminator);
                out.push(0);
                uleb(&mut out, d.len() as u64 + 1);
                out.push(4);
                out.extend_from_slice(&d);
            }
            if row.isa != last.isa {
                out.push(12);
                uleb(&mut out, row.isa);
            }
            if row.is_stmt != last.is_stmt {
                out.push(6);
            }
            if row.basic_block {
                out.push(7);
            }
            if row.prologue_end {
                out.push(10);
            }
            if row.epilogue_begin {
                out.push(11);
            }
            if !started {
                set_address(&mut out, &mut at);
                started = true;
            }
            let line = row.line.wrapping_sub(last.line) as i64;
            advance(p, Some(line), delta / min, &mut out);
            last = *row;
        }
    }
    Ok((out, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params {
            version: 4,
            address_size: 8,
            min_inst: 1,
            max_ops: 1,
            default_is_stmt: true,
            line_base: -5,
            line_range: 14,
            opcode_base: 13,
            lengths: STANDARD.to_vec(),
        }
    }

    fn row(address: u64, line: u64) -> Row {
        Row {
            address,
            line,
            ..Row::start(&params())
        }
    }

    #[test]
    fn rows_round_trip() {
        let p = params();
        let mut end = row(0x1000, 2);
        end.end_sequence = true;
        let mut far = row(0x400, 2);
        far.file = 3;
        far.column = 7;
        far.is_stmt = false;
        far.discriminator = 2;
        far.prologue_end = true;
        let rows = vec![
            row(0x10, 5),
            row(0x10, 6),
            row(0x20, 1000),
            far,
            row(0x500, 2),
            end,
        ];
        let (bytes, at) = encode(&p, &[&rows], turbonop_core::Endian::Little).unwrap();
        assert_eq!(at, [3]);
        // A table around it.
        let mut t = Vec::new();
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&4u16.to_le_bytes());
        let header = [1u8, 1, 1, (-5i8) as u8, 14, 13]
            .iter()
            .copied()
            .chain(STANDARD)
            .chain([0, 0])
            .collect::<Vec<u8>>();
        t.extend_from_slice(&(header.len() as u32).to_le_bytes());
        t.extend_from_slice(&header);
        let program = t.len();
        t.extend_from_slice(&bytes);
        let len = (t.len() - 4) as u32;
        t[..4].copy_from_slice(&len.to_le_bytes());
        let table = table(&t, 0, 8, turbonop_core::Endian::Little).unwrap();
        assert_eq!(table.program, program..t.len());
        let read = |at: usize, n: u8| {
            let mut v = [0u8; 8];
            v[..usize::from(n)].copy_from_slice(&t[at..at + usize::from(n)]);
            Some(u64::from_le_bytes(v))
        };
        let seqs = decode(&t, &table, turbonop_core::Endian::Little, &read).unwrap();
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].rows, rows);
        assert_eq!(seqs[0].set_address, Some(program + 3));
        // Addresses that go back don't encode.
        let back = vec![row(0x10, 5), row(0x8, 6)];
        assert!(encode(&p, &[&back], turbonop_core::Endian::Little).is_err());
    }
}
