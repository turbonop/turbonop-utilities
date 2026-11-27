//! Metadata built in memory, for the unit tests: small tables (every index
//! two bytes) and heaps.

#![allow(clippy::cast_possible_truncation)]

use crate::reader::encode_compressed_u32;
use crate::tables::{ColumnKind, TableId};

pub(crate) struct MetadataBuilder {
    strings: Vec<u8>,
    user_strings: Vec<u8>,
    guids: Vec<u8>,
    blobs: Vec<u8>,
    rows: Vec<Vec<Vec<u32>>>,
    pub(crate) version: &'static [u8],
    pub(crate) extra_streams: Vec<(&'static [u8], Vec<u8>)>,
    pub(crate) uncompressed: bool,
}

impl MetadataBuilder {
    pub(crate) fn new() -> Self {
        Self {
            strings: vec![0],
            user_strings: vec![0],
            guids: Vec::new(),
            blobs: vec![0],
            rows: vec![Vec::new(); 64],
            version: b"v4.0.30319",
            extra_streams: Vec::new(),
            uncompressed: false,
        }
    }

    pub(crate) fn string(&mut self, s: &str) -> u32 {
        let at = self.strings.len() as u32;
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        at
    }

    pub(crate) fn blob(&mut self, b: &[u8]) -> u32 {
        let at = self.blobs.len() as u32;
        encode_compressed_u32(b.len() as u32, &mut self.blobs);
        self.blobs.extend_from_slice(b);
        at
    }

    #[allow(dead_code)]
    pub(crate) fn user_string(&mut self, s: &str) -> u32 {
        let at = self.user_strings.len() as u32;
        let units: Vec<u16> = s.encode_utf16().collect();
        encode_compressed_u32(units.len() as u32 * 2 + 1, &mut self.user_strings);
        for u in units {
            self.user_strings.extend_from_slice(&u.to_le_bytes());
        }
        self.user_strings.push(0);
        at
    }

    #[allow(dead_code)]
    pub(crate) fn guid(&mut self, g: [u8; 16]) -> u32 {
        self.guids.extend_from_slice(&g);
        (self.guids.len() / 16) as u32
    }

    /// Adds a row; returns its rid.
    pub(crate) fn row(&mut self, table: TableId, columns: &[u32]) -> u32 {
        assert_eq!(columns.len(), table.columns().len(), "{table}");
        let t = &mut self.rows[table as usize];
        t.push(columns.to_vec());
        t.len() as u32
    }

    fn tables(&self) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0, 2, 0, 0, 1];
        let valid = (0..64).fold(0u64, |m, i| {
            if self.rows[i].is_empty() {
                m
            } else {
                m | 1 << i
            }
        });
        v.extend_from_slice(&valid.to_le_bytes());
        v.extend_from_slice(&0u64.to_le_bytes());
        for t in &self.rows {
            if !t.is_empty() {
                v.extend_from_slice(&(t.len() as u32).to_le_bytes());
            }
        }
        for &table in TableId::ALL {
            for row in &self.rows[table as usize] {
                for (col, &value) in table.columns().iter().zip(row) {
                    if col.kind == ColumnKind::U32 {
                        v.extend_from_slice(&value.to_le_bytes());
                    } else {
                        v.extend_from_slice(&(value as u16).to_le_bytes());
                    }
                }
            }
        }
        v
    }

    pub(crate) fn build(&self) -> Vec<u8> {
        let mut streams: Vec<(&[u8], Vec<u8>)> = vec![
            (if self.uncompressed { b"#-" } else { b"#~" }, self.tables()),
            (b"#Strings", self.strings.clone()),
            (b"#US", self.user_strings.clone()),
            (b"#GUID", self.guids.clone()),
            (b"#Blob", self.blobs.clone()),
        ];
        for (n, d) in &self.extra_streams {
            streams.push((n, d.clone()));
        }
        for s in &mut streams {
            while !s.1.len().is_multiple_of(4) {
                s.1.push(0);
            }
        }
        let mut root = Vec::new();
        root.extend_from_slice(b"BSJB");
        root.extend_from_slice(&1u16.to_le_bytes());
        root.extend_from_slice(&1u16.to_le_bytes());
        root.extend_from_slice(&0u32.to_le_bytes());
        let mut version = self.version.to_vec();
        version.push(0);
        while !version.len().is_multiple_of(4) {
            version.push(0);
        }
        root.extend_from_slice(&(version.len() as u32).to_le_bytes());
        root.extend_from_slice(&version);
        root.extend_from_slice(&0u16.to_le_bytes());
        root.extend_from_slice(&(streams.len() as u16).to_le_bytes());
        let header_size: usize = streams
            .iter()
            .map(|(n, _)| 8 + (n.len() + 1).next_multiple_of(4))
            .sum();
        let mut at = root.len() + header_size;
        for (n, d) in &streams {
            root.extend_from_slice(&(at as u32).to_le_bytes());
            root.extend_from_slice(&(d.len() as u32).to_le_bytes());
            root.extend_from_slice(n);
            root.push(0);
            while !root.len().is_multiple_of(4) {
                root.push(0);
            }
            at += d.len();
        }
        for (_, d) in &streams {
            root.extend_from_slice(d);
        }
        root
    }
}
