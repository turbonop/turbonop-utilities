//! Portable PDBs (the Portable PDB v1.0 specification): the `#Pdb` stream,
//! document names, and sequence points.
//!
//! A portable PDB is metadata like an assembly's ([`Metadata::parse`] reads
//! it), whose tables are the debug ones (Document, MethodDebugInformation,
//! LocalScope, ...) and whose `#Pdb` stream holds the PDB's ID, the entry
//! point, and the row counts of the assembly's type-system tables (which
//! set the widths of indexes into them).

use turbonop_core::{ErrorKind, Result};

use crate::heaps::BlobIndex;
use crate::metadata::Metadata;
use crate::reader::Cursor;
use crate::tables::{DocumentRow, MethodDebugInformationRow, TableId, Token};

/// The `#Pdb` stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdbStream<'d> {
    /// The PDB ID: the GUID and stamp the assembly's CodeView debug
    /// directory entry names.
    pub id: [u8; 20],
    /// The entry point's MethodDef token (0 if none).
    pub entry_point: Token,
    /// Which of the assembly's tables it counts.
    pub referenced_type_system_tables: u64,
    /// Their row counts (by table number; 0 for those not referenced).
    pub type_system_rows: [u32; 64],
    /// The stream's bytes.
    pub data: &'d [u8],
}

impl<'d> PdbStream<'d> {
    /// Parses the stream (`base`: its offset in the metadata, for errors).
    pub fn parse(data: &'d [u8], base: u64) -> Result<Self> {
        let mut c = Cursor::with_base(data, base);
        let id = c.array::<20>("#Pdb stream")?;
        let entry_point = Token(c.u32("#Pdb stream")?);
        let referenced = c.u64("#Pdb stream")?;
        let mut rows = [0u32; 64];
        for (i, r) in rows.iter_mut().enumerate() {
            if referenced & (1 << i) != 0 {
                *r = c.u32("#Pdb type system table rows")?;
            }
        }
        Ok(Self {
            id,
            entry_point,
            referenced_type_system_tables: referenced,
            type_system_rows: rows,
            data,
        })
    }
}

/// A document name blob (a separator, then blobs of UTF-8 parts) as a
/// string: the parts joined by the separator.
pub fn document_name(md: &Metadata<'_>, name: BlobIndex) -> Result<String> {
    let blob = md.blob(name)?;
    let mut c = Cursor::new(blob);
    let sep = c.u8("document name separator")?;
    if sep > 0x7F {
        return Err(c.error(ErrorKind::Malformed, "document name separator"));
    }
    let mut out = Vec::new();
    let mut first = true;
    while !c.is_empty() {
        if !first && sep != 0 {
            out.push(sep);
        }
        first = false;
        let part = c.compressed_u32("document name part")?;
        out.extend_from_slice(md.blob(BlobIndex(part))?);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The line SRM reports for a hidden sequence point.
pub const HIDDEN_LINE: u32 = 0x00FE_EFEE;

/// A sequence point: where an IL offset's source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencePoint {
    /// The IL offset.
    pub il_offset: u32,
    /// The Document row.
    pub document: u32,
    /// The start line (1-based), or [`HIDDEN_LINE`].
    pub start_line: u32,
    /// The start column (1-based; 0 if hidden).
    pub start_column: u32,
    /// The end line.
    pub end_line: u32,
    /// The end column.
    pub end_column: u32,
}

impl SequencePoint {
    /// Whether it is hidden (no source).
    pub fn is_hidden(&self) -> bool {
        self.start_line == HIDDEN_LINE
    }
}

/// A method's sequence points (its MethodDebugInformation row's blob),
/// and the local signature it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencePoints {
    /// The StandAloneSig row of its locals (0 if none).
    pub local_signature: u32,
    /// The points, in IL order.
    pub points: Vec<SequencePoint>,
}

const MAX_LINE: i64 = 0x2000_0000;
const MAX_COLUMN: i64 = 0x1_0000;

fn checked(v: i64, max: i64, c: &Cursor<'_>, what: &'static str) -> Result<u32> {
    if !(0..max).contains(&v) {
        return Err(c.error(ErrorKind::Malformed, what));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(v as u32)
}

/// Decodes a sequence points blob; `document` is the row's Document (0
/// if the blob names its initial one).
pub fn sequence_points(blob: &[u8], document: u32) -> Result<SequencePoints> {
    let mut c = Cursor::new(blob);
    let mut points = Vec::new();
    let local_signature = if c.is_empty() {
        0
    } else {
        c.compressed_u32("sequence points header")?
    };
    let mut document = document;
    if document == 0 && !c.is_empty() {
        document = c.compressed_u32("sequence points document")?;
    }
    let mut previous: Option<(u32, u32)> = None;
    let mut offset = 0u32;
    let mut first = true;
    while !c.is_empty() {
        if first {
            offset = c.compressed_u32("sequence point IL offset")?;
            first = false;
        } else {
            let mut delta = c.compressed_u32("sequence point IL offset")?;
            while delta == 0 {
                document = c.compressed_u32("sequence point document")?;
                delta = c.compressed_u32("sequence point IL offset")?;
            }
            offset = offset
                .checked_add(delta)
                .ok_or_else(|| c.error(ErrorKind::Overflow, "sequence point IL offset"))?;
        }
        let delta_lines = c.compressed_u32("sequence point lines")?;
        let delta_columns = if delta_lines == 0 {
            i64::from(c.compressed_u32("sequence point columns")?)
        } else {
            i64::from(c.compressed_i32("sequence point columns")?)
        };
        if delta_lines == 0 && delta_columns == 0 {
            points.push(SequencePoint {
                il_offset: offset,
                document,
                start_line: HIDDEN_LINE,
                start_column: 0,
                end_line: HIDDEN_LINE,
                end_column: 0,
            });
            continue;
        }
        let (start_line, start_column) = match previous {
            None => (
                checked(
                    i64::from(c.compressed_u32("sequence point line")?),
                    MAX_LINE,
                    &c,
                    "sequence point line",
                )?,
                checked(
                    i64::from(c.compressed_u32("sequence point column")?),
                    MAX_COLUMN,
                    &c,
                    "sequence point column",
                )?,
            ),
            Some((line, column)) => (
                checked(
                    i64::from(line) + i64::from(c.compressed_i32("sequence point line")?),
                    MAX_LINE,
                    &c,
                    "sequence point line",
                )?,
                checked(
                    i64::from(column) + i64::from(c.compressed_i32("sequence point column")?),
                    MAX_COLUMN,
                    &c,
                    "sequence point column",
                )?,
            ),
        };
        previous = Some((start_line, start_column));
        points.push(SequencePoint {
            il_offset: offset,
            document,
            start_line,
            start_column,
            end_line: checked(
                i64::from(start_line) + i64::from(delta_lines),
                MAX_LINE,
                &c,
                "sequence point end line",
            )?,
            end_column: checked(
                i64::from(start_column) + delta_columns,
                MAX_COLUMN,
                &c,
                "sequence point end column",
            )?,
        });
    }
    Ok(SequencePoints {
        local_signature,
        points,
    })
}

/// Method `method`'s sequence points (portable PDBs), if it has any.
pub fn method_sequence_points(md: &Metadata<'_>, method: u32) -> Result<Option<SequencePoints>> {
    let Some(row) = md.row::<MethodDebugInformationRow>(method) else {
        return Ok(None);
    };
    if row.sequence_points.0 == 0 {
        return Ok(None);
    }
    sequence_points(md.blob(row.sequence_points)?, row.document).map(Some)
}

/// Document `rid`'s name.
pub fn document_row_name(md: &Metadata<'_>, rid: u32) -> Result<Option<String>> {
    match md.row::<DocumentRow>(rid) {
        Some(d) => document_name(md, d.name).map(Some),
        None => Ok(None),
    }
}

/// Whether `md` is a portable PDB's (it has a `#Pdb` stream or Document
/// rows).
pub fn is_pdb(md: &Metadata<'_>) -> bool {
    md.pdb().is_some() || md.tables().rows(TableId::Document) > 0
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use crate::testutil::MetadataBuilder;

    #[test]
    fn pdb_stream() {
        let mut d = vec![7u8; 20];
        d.extend_from_slice(&0x0600_0001u32.to_le_bytes());
        d.extend_from_slice(&((1u64 << 2) | (1 << 6)).to_le_bytes());
        d.extend_from_slice(&5u32.to_le_bytes());
        d.extend_from_slice(&9u32.to_le_bytes());
        let p = PdbStream::parse(&d, 0).unwrap();
        assert_eq!(p.entry_point, Token(0x0600_0001));
        assert_eq!(p.type_system_rows[2], 5);
        assert_eq!(p.type_system_rows[6], 9);
        for n in 0..d.len() {
            assert!(PdbStream::parse(&d[..n], 0).is_err());
        }
    }

    #[test]
    fn document_names() {
        let mut b = MetadataBuilder::new();
        let c = b.blob(b"C:");
        let src = b.blob(b"src");
        let a = b.blob(b"a.cs");
        let mut name = vec![b'\\'];
        for part in [c, src, a] {
            crate::reader::encode_compressed_u32(part, &mut name);
        }
        let n = b.blob(&name);
        let none = b.blob(&[0, c as u8, a as u8]);
        let bad = b.blob(&[0x80, 1]);
        let data = b.build();
        let md = Metadata::parse(&data).unwrap();
        assert_eq!(document_name(&md, BlobIndex(n)).unwrap(), "C:\\src\\a.cs");
        assert_eq!(document_name(&md, BlobIndex(none)).unwrap(), "C:a.cs");
        assert!(document_name(&md, BlobIndex(bad)).is_err());
        assert!(document_name(&md, BlobIndex(0)).is_err());
    }

    #[test]
    fn sequence_point_records() {
        // Local sig 3; document 1 (initial); IL 0 at 10:5-10:9; IL 4
        // hidden; IL 6 switching to document 2, at 12:1-14:3; IL 7 at
        // 11:2 (deltas from 12:1).
        let blob = [
            3, 1, // header
            0, 0, 4, 10, 5, // IL 0, 0 lines, 4 columns, line 10, column 5
            4, 0, 0, // +4, hidden
            0, 2, 2, 2, 2, 4, 0x7D, // doc 2; +2 IL, 2 lines, +1 column, line +2, column -2
            1, 0, 1, 0x7F, 2, // +1, 0 lines, 1 column, line -1, column +1
        ];
        let s = sequence_points(&blob, 0).unwrap();
        assert_eq!(s.local_signature, 3);
        let p = &s.points;
        assert_eq!(p.len(), 4);
        assert_eq!(
            (
                p[0].il_offset,
                p[0].start_line,
                p[0].start_column,
                p[0].end_line,
                p[0].end_column
            ),
            (0, 10, 5, 10, 9)
        );
        assert!(p[1].is_hidden() && p[1].il_offset == 4);
        assert_eq!(
            (
                p[2].il_offset,
                p[2].document,
                p[2].start_line,
                p[2].start_column,
                p[2].end_line,
                p[2].end_column
            ),
            (6, 2, 12, 3, 14, 4)
        );
        assert_eq!(
            (p[3].start_line, p[3].start_column, p[3].end_column),
            (11, 4, 5)
        );
        for n in 0..blob.len() {
            let _ = sequence_points(&blob[..n], 0);
        }
        // A start line out of range.
        assert!(sequence_points(&[0, 0, 0, 1, 2, 0xDF, 0xFF, 0xFF, 0xFF, 1], 1).is_err());
        // A column that goes negative.
        assert!(sequence_points(&[0, 0, 0, 1, 2, 1, 1, 0, 1, 0, 0x7B], 1).is_err());
        assert!(sequence_points(&[], 1).unwrap().points.is_empty());
    }
}
