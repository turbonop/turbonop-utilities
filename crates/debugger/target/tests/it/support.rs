//! A minidump writer, for tests: streams laid out as `MiniDumpWriteDump`
//! and Breakpad lay them out.

pub fn testdata(path: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(path);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[derive(Default)]
pub struct Writer {
    data: Vec<u8>,
    streams: Vec<(u32, u32, u32)>,
}

pub struct ThreadSpec<'a> {
    pub id: u32,
    pub teb: u64,
    pub stack_start: u64,
    pub stack: &'a [u8],
    pub context: &'a [u8],
}

pub struct ModuleSpec<'a> {
    pub base: u64,
    pub size: u32,
    pub time_date_stamp: u32,
    pub name: &'a str,
    pub cv: &'a [u8],
}

impl Writer {
    pub fn new() -> Writer {
        // The header, written last.
        Writer {
            data: vec![0; 32],
            streams: Vec::new(),
        }
    }

    /// Appends bytes (4-byte aligned), returning their RVA.
    pub fn blob(&mut self, bytes: &[u8]) -> u32 {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let rva = u32::try_from(self.data.len()).unwrap();
        self.data.extend_from_slice(bytes);
        rva
    }

    /// A `MINIDUMP_STRING`.
    pub fn string(&mut self, s: &str) -> u32 {
        let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut b = u32::try_from(units.len()).unwrap().to_le_bytes().to_vec();
        b.extend(units);
        b.extend([0, 0]);
        self.blob(&b)
    }

    pub fn stream(&mut self, kind: u32, bytes: &[u8]) {
        let rva = self.blob(bytes);
        self.streams
            .push((kind, u32::try_from(bytes.len()).unwrap(), rva));
    }

    pub fn system_info(&mut self, architecture: u16, platform: u32) {
        let mut b = vec![0u8; 56];
        b[0..2].copy_from_slice(&architecture.to_le_bytes());
        b[20..24].copy_from_slice(&platform.to_le_bytes());
        self.stream(7, &b);
    }

    pub fn threads(&mut self, threads: &[ThreadSpec<'_>]) {
        let mut b = u32::try_from(threads.len()).unwrap().to_le_bytes().to_vec();
        let mut entries = Vec::new();
        for t in threads {
            let stack = self.blob(t.stack);
            let context = self.blob(t.context);
            let mut e = vec![0u8; 48];
            e[0..4].copy_from_slice(&t.id.to_le_bytes());
            e[16..24].copy_from_slice(&t.teb.to_le_bytes());
            e[24..32].copy_from_slice(&t.stack_start.to_le_bytes());
            e[32..36].copy_from_slice(&u32::try_from(t.stack.len()).unwrap().to_le_bytes());
            e[36..40].copy_from_slice(&stack.to_le_bytes());
            e[40..44].copy_from_slice(&u32::try_from(t.context.len()).unwrap().to_le_bytes());
            e[44..48].copy_from_slice(&context.to_le_bytes());
            entries.extend(e);
        }
        b.extend(entries);
        self.stream(3, &b);
    }

    pub fn thread_names(&mut self, names: &[(u32, &str)]) {
        let mut b = u32::try_from(names.len()).unwrap().to_le_bytes().to_vec();
        for &(id, name) in names {
            let rva = self.string(name);
            b.extend(id.to_le_bytes());
            b.extend(u64::from(rva).to_le_bytes());
        }
        self.stream(24, &b);
    }

    pub fn modules(&mut self, modules: &[ModuleSpec<'_>]) {
        let mut b = u32::try_from(modules.len()).unwrap().to_le_bytes().to_vec();
        for m in modules {
            let name = self.string(m.name);
            let cv = self.blob(m.cv);
            let mut e = vec![0u8; 108];
            e[0..8].copy_from_slice(&m.base.to_le_bytes());
            e[8..12].copy_from_slice(&m.size.to_le_bytes());
            e[16..20].copy_from_slice(&m.time_date_stamp.to_le_bytes());
            e[20..24].copy_from_slice(&name.to_le_bytes());
            e[76..80].copy_from_slice(&u32::try_from(m.cv.len()).unwrap().to_le_bytes());
            e[80..84].copy_from_slice(&cv.to_le_bytes());
            b.extend(e);
        }
        self.stream(4, &b);
    }

    pub fn memory(&mut self, ranges: &[(u64, &[u8])]) {
        let mut b = u32::try_from(ranges.len()).unwrap().to_le_bytes().to_vec();
        for &(start, bytes) in ranges {
            let rva = self.blob(bytes);
            b.extend(start.to_le_bytes());
            b.extend(u32::try_from(bytes.len()).unwrap().to_le_bytes());
            b.extend(rva.to_le_bytes());
        }
        self.stream(5, &b);
    }

    pub fn exception(&mut self, thread: u32, code: u32, address: u64, context: &[u8]) {
        let context = self.blob(context);
        let len = self.data.len() - usize::try_from(context).unwrap();
        let mut b = vec![0u8; 168];
        b[0..4].copy_from_slice(&thread.to_le_bytes());
        b[8..12].copy_from_slice(&code.to_le_bytes());
        b[24..32].copy_from_slice(&address.to_le_bytes());
        b[32..36].copy_from_slice(&2u32.to_le_bytes());
        b[40..48].copy_from_slice(&1u64.to_le_bytes());
        b[48..56].copy_from_slice(&0xdeadu64.to_le_bytes());
        b[160..164].copy_from_slice(&u32::try_from(len).unwrap().to_le_bytes());
        b[164..168].copy_from_slice(&context.to_le_bytes());
        self.stream(6, &b);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let mut dir = Vec::new();
        for (kind, size, rva) in &self.streams {
            dir.extend(kind.to_le_bytes());
            dir.extend(size.to_le_bytes());
            dir.extend(rva.to_le_bytes());
        }
        let dir_rva = self.blob(&dir);
        let count = u32::try_from(self.streams.len()).unwrap();
        self.data[0..4].copy_from_slice(b"MDMP");
        self.data[4..8].copy_from_slice(&0xA793u32.to_le_bytes());
        self.data[8..12].copy_from_slice(&count.to_le_bytes());
        self.data[12..16].copy_from_slice(&dir_rva.to_le_bytes());
        self.data
    }
}

/// An x64 `CONTEXT` (all groups) with RIP and RSP.
pub fn amd64_context(rip: u64, rsp: u64) -> Vec<u8> {
    let mut b = vec![0u8; 0x4D0];
    b[0x30..0x34].copy_from_slice(&0x0010_000Fu32.to_le_bytes());
    b[0x98..0xA0].copy_from_slice(&rsp.to_le_bytes());
    b[0xF8..0x100].copy_from_slice(&rip.to_le_bytes());
    b
}

/// An ARM64 `CONTEXT` (all groups) with PC and SP.
pub fn arm64_context(pc: u64, sp: u64) -> Vec<u8> {
    let mut b = vec![0u8; 0x390];
    b[0..4].copy_from_slice(&0x0040_0007u32.to_le_bytes());
    b[0x100..0x108].copy_from_slice(&sp.to_le_bytes());
    b[0x108..0x110].copy_from_slice(&pc.to_le_bytes());
    b
}
