//! A dump of any bitstream, as `llvm-bcanalyzer` writes it: every block
//! and record (`--dump`), and the per-block summary and record histogram
//! that follow. The output is llvm-bcanalyzer's, byte for byte.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use turbonop_core::{Error, ErrorKind, Result};

use crate::bitstream::{AbbrevOp, BlockInfo, Cursor, Entry, Stream, StreamKind, low32};
use crate::tables::{
    BLOCKINFO_BLOCK_ID, DEFINE_ABBREV, FIRST_APPLICATION_BLOCKID, METADATA_BLOCK_ID,
    METADATA_INDEX, METADATA_INDEX_OFFSET, METADATA_STRINGS, UNABBREV_RECORD, block_name,
    record_name,
};

/// What to write (llvm-bcanalyzer's options).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpOptions {
    /// Every block and record (`--dump`); otherwise only the summary.
    pub records: bool,
    /// Names only, without numeric IDs (the default; `--non-symbolic`
    /// clears it).
    pub symbolic: bool,
    /// `BLOCKINFO`'s records too (`--dump-blockinfo`).
    pub blockinfo: bool,
    /// Blobs as escaped bytes (`--show-binary-blobs`).
    pub binary_blobs: bool,
    /// Each block's record histogram (unless `--disable-histogram`).
    pub histogram: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            records: true,
            symbolic: true,
            blockinfo: false,
            binary_blobs: false,
            histogram: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RecordStats {
    instances: u64,
    bits: u64,
    abbreviated: u64,
}

#[derive(Clone, Debug, Default)]
struct BlockStats {
    instances: u64,
    bits: u64,
    sub_blocks: u64,
    abbrevs: u64,
    records: u64,
    abbreviated: u64,
    codes: BTreeMap<u32, RecordStats>,
}

struct Frame {
    id: u32,
    indent: usize,
    start: u64,
    /// Where it started, in its parent (after its ID), when it has one.
    entered_at: Option<u64>,
    dump: bool,
    name: Option<Vec<u8>>,
    index_offset: u64,
}

struct Analyzer<'a, 'd> {
    c: Cursor<'d>,
    kind: StreamKind,
    opts: DumpOptions,
    out: &'a mut Vec<u8>,
    stats: BTreeMap<u32, BlockStats>,
    info: Arc<BlockInfo>,
    top_blocks: u64,
}

fn is_print(b: u8) -> bool {
    (0x20..=0x7E).contains(&b)
}

/// `raw_ostream::write_escaped`, with hex escapes.
fn write_escaped(out: &mut Vec<u8>, s: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &c in s {
        match c {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'"' => out.extend_from_slice(b"\\\""),
            c if is_print(c) => out.push(c),
            c => out.extend_from_slice(&[
                b'\\',
                b'x',
                HEX[usize::from(c >> 4)],
                HEX[usize::from(c & 15)],
            ]),
        }
    }
}

/// `%e`, as C's printf writes it.
fn exp(x: f64) -> String {
    let s = format!("{x:.6e}");
    match s.split_once('e') {
        Some((m, e)) => {
            let e: i32 = e.parse().unwrap_or(0);
            let sign = if e < 0 { '-' } else { '+' };
            format!("{m}e{sign}{:02}", e.unsigned_abs())
        }
        None => s,
    }
}

fn size_bits(bits: u64) -> String {
    format!("{bits}b/{:.2}B/{}W", bits as f64 / 8.0, bits / 32)
}

fn size_avg(bits: f64) -> String {
    // (unsigned long)(Bits / 32): truncated toward zero.
    format!("{bits:.2}/{:.2}B/{}W", bits / 8.0, (bits / 32.0).trunc())
}

impl Analyzer<'_, '_> {
    fn block_name(&self, id: u32) -> Option<Vec<u8>> {
        if id < FIRST_APPLICATION_BLOCKID {
            return (id == BLOCKINFO_BLOCK_ID).then(|| b"BLOCKINFO_BLOCK".to_vec());
        }
        if let Some(n) = self.info.block_name(id).filter(|n| !n.is_empty()) {
            return Some(n.to_vec());
        }
        if self.kind != StreamKind::LlvmIr {
            return None;
        }
        block_name(id).map(|n| n.as_bytes().to_vec())
    }

    fn record_name(&self, block: u32, code: u32) -> Option<Vec<u8>> {
        if block < FIRST_APPLICATION_BLOCKID {
            if block == BLOCKINFO_BLOCK_ID {
                return record_name(block, code).map(|n| n.as_bytes().to_vec());
            }
            return None;
        }
        if let Some(n) = self.info.record_name(block, code) {
            return Some(n.to_vec());
        }
        if self.kind != StreamKind::LlvmIr {
            return None;
        }
        record_name(block, code).map(|n| n.as_bytes().to_vec())
    }

    fn put(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn indent(&mut self, n: usize) {
        self.out.resize(self.out.len() + n, b' ');
    }

    fn open(&mut self, id: u32, indent: usize, entered_at: Option<u64>) -> Result<Frame> {
        let start = self.c.position();
        self.stats.entry(id).or_default().instances += 1;
        let mut dump = self.opts.records;
        if id == BLOCKINFO_BLOCK_ID {
            if self.opts.records && !self.opts.blockinfo {
                self.indent(indent);
                self.put("<BLOCKINFO_BLOCK/>\n");
            }
            let info = Arc::new(self.c.read_block_info()?);
            self.c.set_block_info(Arc::clone(&info));
            self.info = info;
            self.c.seek(start)?;
            dump = self.opts.records && self.opts.blockinfo;
        }
        let header = self.c.enter_block(id)?;
        let name = if dump { self.block_name(id) } else { None };
        if dump {
            self.indent(indent);
            self.out.push(b'<');
            match &name {
                Some(n) => self.out.extend_from_slice(n),
                None => self.put(&format!("UnknownBlock{id}")),
            }
            if !self.opts.symbolic && name.is_some() {
                self.put(&format!(" BlockID={id}"));
            }
            self.put(&format!(
                " NumWords={} BlockCodeSize={}>\n",
                header.num_words, header.abbrev_width
            ));
        }
        Ok(Frame {
            id,
            indent,
            start,
            entered_at,
            dump,
            name,
            index_offset: 0,
        })
    }

    fn run(&mut self) -> Result<()> {
        while !self.c.at_end() {
            let at = self.c.position();
            match self.c.advance_with(true)? {
                Entry::SubBlock(id) => {
                    self.block(id)?;
                    self.top_blocks += 1;
                }
                _ => {
                    return Err(Error::new(
                        ErrorKind::Malformed,
                        at / 8,
                        "record at the top level",
                    ));
                }
            }
        }
        Ok(())
    }

    fn block(&mut self, id: u32) -> Result<()> {
        let mut stack = alloc::vec![self.open(id, 0, None)?];
        let mut ops = Vec::new();
        while let Some(frame) = stack.last_mut() {
            if self.c.at_end() {
                return Err(Error::new(
                    ErrorKind::Truncated,
                    self.c.position() / 8,
                    "bitstream (premature end)",
                ));
            }
            let record_start = self.c.position();
            match self.c.advance_with(true)? {
                Entry::EndBlock => {
                    let end = self.c.position();
                    let Some(frame) = stack.pop() else { break };
                    self.stats.entry(frame.id).or_default().bits += end - frame.start;
                    if frame.dump {
                        self.indent(frame.indent);
                        self.put("</");
                        match &frame.name {
                            Some(n) => self.out.extend_from_slice(n),
                            None => self.put(&format!("UnknownBlock{}", frame.id)),
                        }
                        self.put(">\n");
                    }
                    if let (Some(parent), Some(entered)) = (stack.last_mut(), frame.entered_at) {
                        self.stats.entry(parent.id).or_default().sub_blocks += 1;
                        // A sub-block's bits aren't its parent's.
                        parent.start += end - entered;
                    }
                }
                Entry::SubBlock(sub) => {
                    let entered = self.c.position();
                    let indent = frame.indent + 2;
                    let f = self.open(sub, indent, Some(entered))?;
                    stack.push(f);
                }
                Entry::Record(DEFINE_ABBREV) => {
                    self.c.read_abbrev()?;
                    self.stats.entry(frame.id).or_default().abbrevs += 1;
                }
                Entry::Record(abbrev_id) => {
                    let block = frame.id;
                    ops.clear();
                    let head = self.c.read_record(abbrev_id, &mut ops)?;
                    let after = self.c.position();
                    let stats = self.stats.entry(block).or_default();
                    stats.records += 1;
                    let rec = stats.codes.entry(head.code).or_default();
                    rec.instances += 1;
                    rec.bits += after - record_start;
                    if abbrev_id != UNABBREV_RECORD {
                        rec.abbreviated += 1;
                        stats.abbreviated += 1;
                    }
                    let Some(frame) = stack.last_mut() else { break };
                    if !frame.dump {
                        continue;
                    }
                    let indent = frame.indent;
                    let mut line = Vec::new();
                    line.resize(indent, b' ');
                    line.extend_from_slice(b"  <");
                    let name = self.record_name(block, head.code);
                    match &name {
                        Some(n) => line.extend_from_slice(n),
                        None => {
                            line.extend_from_slice(format!("UnknownCode{}", head.code).as_bytes());
                        }
                    }
                    if !self.opts.symbolic && name.is_some() {
                        line.extend_from_slice(format!(" codeid={}", head.code).as_bytes());
                    }
                    let abbrev = if abbrev_id == UNABBREV_RECORD {
                        None
                    } else {
                        line.extend_from_slice(format!(" abbrevid={abbrev_id}").as_bytes());
                        Some(Arc::clone(self.c.abbrev(abbrev_id)?))
                    };
                    for (i, &v) in ops.iter().enumerate() {
                        line.extend_from_slice(format!(" op{i}={}", v as i64).as_bytes());
                    }
                    if block == METADATA_BLOCK_ID {
                        if head.code == METADATA_INDEX_OFFSET {
                            if ops.len() == 2 {
                                let offset = ops[0].wrapping_add(ops[1].wrapping_shl(32));
                                frame.index_offset = after.wrapping_add(offset);
                            } else {
                                line.extend_from_slice(b"(Invalid record)");
                            }
                        }
                        if head.code == METADATA_INDEX {
                            line.extend_from_slice(b" (offset ");
                            if frame.index_offset == record_start {
                                line.extend_from_slice(b"match)");
                            } else {
                                line.extend_from_slice(
                                    format!(
                                        "mismatch: {} vs {})",
                                        frame.index_offset, record_start
                                    )
                                    .as_bytes(),
                                );
                            }
                        }
                    }
                    line.extend_from_slice(b"/>");
                    if let Some(a) = &abbrev
                        && let Some(i) = a.ops.iter().skip(1).position(|o| *o == AbbrevOp::Array)
                    {
                        // Array op i + 1 holds record values from i on.
                        let from = i.min(ops.len());
                        let chars: Vec<u8> =
                            ops[from..].iter().map(|&v| v.to_le_bytes()[0]).collect();
                        if chars.iter().all(|&b| is_print(b)) {
                            line.extend_from_slice(b" record string = '");
                            line.extend_from_slice(&chars);
                            line.push(b'\'');
                        }
                    }
                    self.out.extend_from_slice(&line);
                    if let Some(blob) = head.blob {
                        if block == METADATA_BLOCK_ID && head.code == METADATA_STRINGS {
                            self.metadata_strings(indent, &ops, blob)?;
                        } else {
                            self.put(" blob data = ");
                            if self.opts.binary_blobs {
                                self.out.push(b'\'');
                                write_escaped(self.out, blob);
                                self.out.push(b'\'');
                            } else if blob.iter().all(|&b| is_print(b)) {
                                self.out.push(b'\'');
                                self.out.extend_from_slice(blob);
                                self.out.push(b'\'');
                            } else {
                                self.put(&format!("unprintable, {} bytes.", blob.len()));
                            }
                        }
                    }
                    self.out.push(b'\n');
                }
            }
        }
        Ok(())
    }

    fn metadata_strings(&mut self, indent: usize, ops: &[u64], blob: &[u8]) -> Result<()> {
        let at = self.c.position() / 8;
        let bad = |what| Error::new(ErrorKind::Malformed, at, what);
        if blob.is_empty() {
            return Err(bad("metadata strings (empty blob)"));
        }
        if ops.len() != 2 {
            return Err(bad("metadata strings record"));
        }
        let count = low32(ops[0]);
        let offset = crate::bitstream::idx(u64::from(low32(ops[1]))).min(blob.len());
        self.put(&format!(" num-strings = {count} {{\n"));
        let mut lengths = crate::bitstream::BitReader::new(&blob[..offset]);
        let mut chars = &blob[offset..];
        let mut left = count;
        loop {
            if lengths.at_end() {
                return Err(bad("metadata strings (bad length)"));
            }
            let n = lengths.vbr32(6)? as usize;
            if chars.len() < n {
                return Err(bad("metadata strings (truncated chars)"));
            }
            self.indent(indent);
            self.put("    '");
            write_escaped(self.out, &chars[..n]);
            self.put("'\n");
            chars = &chars[n..];
            left = left.wrapping_sub(1);
            if left == 0 {
                break;
            }
        }
        self.indent(indent);
        self.put("  }");
        Ok(())
    }

    fn summary(&mut self, file_name: Option<&str>, total_bits: u64) {
        self.put("Summary ");
        if let Some(n) = file_name {
            self.put(&format!("of {n}:\n"));
        }
        self.put(&format!("         Total size: {}\n", size_bits(total_bits)));
        self.put(&format!(
            "        Stream type: {}\n",
            match self.kind {
                StreamKind::Unknown => "unknown",
                StreamKind::LlvmIr => "LLVM IR",
                StreamKind::ClangAst => "Clang Serialized AST",
                StreamKind::ClangDiagnostics => "Clang Serialized Diagnostics",
                StreamKind::Remarks => "LLVM Remarks",
            }
        ));
        self.put(&format!("  # Toplevel Blocks: {}\n\n", self.top_blocks));
        self.put("Per-block Summary:\n");
        let stats = core::mem::take(&mut self.stats);
        for (&id, s) in &stats {
            self.put(&format!("  Block ID #{id}"));
            if let Some(n) = self.block_name(id) {
                self.put(" (");
                self.out.extend_from_slice(&n);
                self.put(")");
            }
            self.put(":\n");
            self.put(&format!("      Num Instances: {}\n", s.instances));
            self.put(&format!("         Total Size: {}\n", size_bits(s.bits)));
            let pct = s.bits as f64 * 100.0 / total_bits as f64;
            self.put(&format!("    Percent of file: {pct:2.4}%\n"));
            if s.instances > 1 {
                let n = s.instances as f64;
                self.put(&format!(
                    "       Average Size: {}\n",
                    size_avg(s.bits as f64 / n)
                ));
                self.put(&format!(
                    "  Tot/Avg SubBlocks: {}/{}\n",
                    s.sub_blocks,
                    exp(s.sub_blocks as f64 / n)
                ));
                self.put(&format!(
                    "    Tot/Avg Abbrevs: {}/{}\n",
                    s.abbrevs,
                    exp(s.abbrevs as f64 / n)
                ));
                self.put(&format!(
                    "    Tot/Avg Records: {}/{}\n",
                    s.records,
                    exp(s.records as f64 / n)
                ));
            } else {
                self.put(&format!("      Num SubBlocks: {}\n", s.sub_blocks));
                self.put(&format!("        Num Abbrevs: {}\n", s.abbrevs));
                self.put(&format!("        Num Records: {}\n", s.records));
            }
            if s.records > 0 {
                let pct = s.abbreviated as f64 * 100.0 / s.records as f64;
                self.put(&format!("    Percent Abbrevs: {pct:2.4}%\n"));
            }
            self.put("\n");
            if self.opts.histogram && !s.codes.is_empty() {
                let mut freq: Vec<(u64, u32)> =
                    s.codes.iter().map(|(&c, r)| (r.instances, c)).collect();
                freq.sort_unstable();
                freq.reverse();
                self.put("\tRecord Histogram:\n");
                self.put("\t\t  Count    # Bits     b/Rec   % Abv  Record Kind\n");
                for (_, code) in freq {
                    let r = s.codes[&code];
                    self.put(&format!("\t\t{:7} {:9}", r.instances, r.bits));
                    if r.instances > 1 {
                        self.put(&format!(" {:9.1}", r.bits as f64 / r.instances as f64));
                    } else {
                        self.put("          ");
                    }
                    if r.abbreviated > 0 {
                        self.put(&format!(
                            " {:7.2}",
                            r.abbreviated as f64 / r.instances as f64 * 100.0
                        ));
                    } else {
                        self.put("        ");
                    }
                    self.put("  ");
                    match self.record_name(id, code) {
                        Some(n) => self.out.extend_from_slice(&n),
                        None => self.put(&format!("UnknownCode{code}")),
                    }
                    self.put("\n");
                }
                self.put("\n");
            }
        }
        self.stats = stats;
    }
}

/// Writes `data`'s bitstream as `llvm-bcanalyzer` would (with `--dump`
/// when [`DumpOptions::records`] is set) to `out`: the dump, then the
/// summary, which names `file_name`. On an error, `out` holds what was
/// written before it (as llvm-bcanalyzer's output would, without its
/// summary).
pub fn analyze(
    data: &[u8],
    opts: &DumpOptions,
    file_name: Option<&str>,
    out: &mut Vec<u8>,
) -> Result<()> {
    let stream = Stream::new(data)?;
    if let (Some(w), true) = (stream.wrapper, opts.records) {
        out.extend_from_slice(
            format!(
                "<BITCODE_WRAPPER_HEADER Magic={:#010x} Version={:#010x} Offset={:#010x} Size={:#010x} CPUType={:#010x}/>\n",
                w.magic, w.version, w.offset, w.size, w.cpu_type
            )
            .as_bytes(),
        );
    }
    let total_bits = stream.data.len() as u64 * 8;
    let mut a = Analyzer {
        c: stream.cursor(),
        kind: stream.kind,
        opts: *opts,
        out,
        stats: BTreeMap::new(),
        info: Arc::new(BlockInfo::default()),
        top_blocks: 0,
    };
    a.run()?;
    if opts.records {
        a.out.extend_from_slice(b"\n\n");
    }
    a.summary(file_name, total_bits);
    Ok(())
}
