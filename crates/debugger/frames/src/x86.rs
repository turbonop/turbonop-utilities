//! 32-bit x86 frames: their modules' PDBs' frame data (FrameData records
//! and their programs, FPO records), which `turbonop_unwind::windows_x86`
//! unwinds by, the way dbghelp's `StackWalk64` does.

use std::collections::HashMap;
use std::path::Path;

use turbonop_mmap::Mmap;
use turbonop_unwind::windows_x86::{Fpo, FrameData, FrameTable, Record};

/// A module's x86 frame records.
pub struct X86Frames {
    table: FrameTable,
    /// FrameData programs' text, by their string table offsets.
    programs: HashMap<u32, Vec<u8>>,
}

impl X86Frames {
    /// The frame records in the PDB at `path` (None if it has none, or
    /// isn't one).
    pub fn load(path: &Path) -> Option<X86Frames> {
        let data = Mmap::open(path).ok()?;
        let pdb = turbonop_pdb::Pdb::parse(&data).ok()?;
        let dbi = pdb.dbi().ok()?;
        let read = |which: usize| -> Option<Vec<u8>> {
            let i = dbi.debug_stream(which)?;
            pdb.msf().stream(i).ok().map(|s| s.to_vec())
        };
        let fpo = read(0).map(|s| Fpo::parse(&s)).unwrap_or_default();
        let frame_data = read(9).map(|s| FrameData::parse(&s)).unwrap_or_default();
        if fpo.is_empty() && frame_data.is_empty() {
            return None;
        }
        let names = pdb.names().ok();
        let mut programs = HashMap::new();
        for d in &frame_data {
            if d.program != 0
                && !programs.contains_key(&d.program)
                && let Some(text) = names.as_ref().and_then(|n| n.get_str(d.program))
            {
                programs.insert(d.program, text.as_bytes().to_vec());
            }
        }
        Some(X86Frames {
            table: FrameTable::new(frame_data, fpo),
            programs,
        })
    }

    /// The record for the code at `rva`, and its program's text.
    pub fn find(&self, rva: u32) -> (Option<Record>, Option<&[u8]>) {
        let record = self.table.find(rva);
        let program = match record {
            Some(Record::FrameData(d)) => self.programs.get(&d.program).map(Vec::as_slice),
            _ => None,
        };
        (record, program)
    }
}
