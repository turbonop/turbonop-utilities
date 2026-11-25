//! Minidumps as targets: Windows' (`MiniDumpWriteDump`), Breakpad's, and
//! Crashpad's, from any of their platforms.

use alloc::string::String;
use alloc::vec::Vec;

use turbonop_abi::Os;
use turbonop_core::{Arch, Result};
use turbonop_minidump::context::Context;
use turbonop_minidump::{CodeView, MemoryMap, Minidump, cpu, stream};

use crate::model::{
    Capabilities, DebugId, Exception, ImageId, Memory, Module, ModuleSource, Provenance, Snapshot,
    Target, Thread,
};
use crate::{Registers, context, linux_maps, os};

/// A minidump's captured memory.
pub struct DumpMemory<'d>(MemoryMap<'d>);

impl Memory for DumpMemory<'_> {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        let Some(d) = self.0.at(address) else {
            return 0;
        };
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        n
    }

    fn next_available(&self, address: u64) -> Option<u64> {
        let ranges = self.0.ranges();
        let i = ranges.partition_point(|r| r.start <= address);
        ranges.get(i).map(|r| r.start)
    }

    fn provenance(&self) -> Provenance {
        Provenance::Dump
    }
}

/// A minidump, as a target.
pub struct MinidumpTarget<'d> {
    dump: Minidump<'d>,
    arch: Arch,
    os: Option<Os>,
    threads: Vec<Thread>,
    modules: Vec<Module>,
    exception: Option<Exception>,
    memory: DumpMemory<'d>,
}

impl<'d> MinidumpTarget<'d> {
    /// Reads a minidump's threads, modules, exception, and memory.
    ///
    /// # Errors
    ///
    /// If it isn't a minidump, or its streams are malformed.
    pub fn new(data: &'d [u8]) -> Result<MinidumpTarget<'d>> {
        let dump = Minidump::parse(data)?;
        let system = dump.system_info()?;
        let os = system.as_ref().and_then(|s| os::of_minidump(s.platform_id));
        let architecture = system.as_ref().map(|s| s.processor_architecture);
        let decode = |raw: &[u8]| context::registers(&Context::parse(raw, architecture));
        let names = dump.thread_names().unwrap_or_default();
        let mut arch = architecture.and_then(arch_of);
        let mut threads = Vec::new();
        for t in dump.threads()? {
            let registers = match decode(t.context) {
                Some((a, r)) => {
                    arch.get_or_insert(a);
                    r
                }
                None => Registers::new(),
            };
            let stack = (!t.stack.data.is_empty())
                .then(|| t.stack.start..t.stack.start + t.stack.data.len() as u64);
            threads.push(Thread {
                id: u64::from(t.id),
                name: names
                    .iter()
                    .find(|(id, _)| *id == t.id)
                    .map(|(_, n)| n.clone()),
                registers,
                stack,
                thread_pointer: (t.teb != 0).then_some(t.teb),
            });
        }
        let exception = dump.exception()?.map(|e| Exception {
            thread: u64::from(e.thread_id),
            code: e.code,
            flags: e.flags,
            address: e.address,
            parameters: e.parameters,
            registers: decode(e.context).map(|(_, r)| r),
        });
        let mut modules: Vec<Module> = dump
            .modules()?
            .iter()
            .map(|m| {
                let (image_id, debug_id) = identity(os, m);
                Module {
                    path: m.name.clone(),
                    base: m.base,
                    size: u64::from(m.size),
                    image_id,
                    debug_id,
                    source: ModuleSource::ModuleList,
                }
            })
            .collect();
        if let Some(maps) = dump.stream(stream::LINUX_MAPS) {
            for found in linux_maps::modules(maps.data) {
                if !modules.iter().any(|m| m.contains(found.base)) {
                    modules.push(Module {
                        path: found.path,
                        base: found.base,
                        size: found.size,
                        image_id: None,
                        debug_id: None,
                        source: ModuleSource::MemoryMap,
                    });
                }
            }
        }
        modules.sort_by_key(|m| m.base);
        Ok(MinidumpTarget {
            arch: arch.unwrap_or(Arch::Unknown),
            os,
            threads,
            modules,
            exception,
            memory: DumpMemory(dump.memory()?),
            dump,
        })
    }

    /// The minidump itself, for what the model doesn't hold (its other
    /// streams).
    pub fn dump(&self) -> &Minidump<'d> {
        &self.dump
    }
}

impl Target for MinidumpTarget<'_> {
    fn arch(&self) -> Arch {
        self.arch
    }

    fn os(&self) -> Option<Os> {
        self.os
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { generation: 0 }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::DUMP
    }

    fn threads(&self) -> &[Thread] {
        &self.threads
    }

    fn modules(&self) -> &[Module] {
        &self.modules
    }

    fn exception(&self) -> Option<&Exception> {
        self.exception.as_ref()
    }

    fn memory(&self) -> &dyn Memory {
        &self.memory
    }
}

/// The architecture a `ProcessorArchitecture` names.
fn arch_of(architecture: u16) -> Option<Arch> {
    Some(match architecture {
        cpu::X86 => Arch::X86,
        cpu::AMD64 => Arch::X64,
        cpu::ARM => Arch::Arm,
        cpu::ARM64 | cpu::ARM64_OLD => Arch::Arm64,
        _ => return None,
    })
}

/// A module's image and debug identities, by its CodeView record (and,
/// for a PE image, its time stamp and size), read as its operating
/// system's writers write them:
///
/// - Windows: the image's `TimeDateStamp` and `SizeOfImage`; the PDB's
///   GUID and age.
/// - Linux and Android: Breakpad's and Crashpad's `BpEL` record, the
///   build ID (for both the image and its debug file). Older Breakpad
///   wrote an `RSDS` record whose GUID is the build ID's first 16 bytes.
/// - macOS and iOS: an `RSDS` record whose GUID is the Mach-O UUID, its
///   first three fields little-endian (as Crashpad writes it; see
///   [`uuid_of_guid`]).
fn identity(
    os: Option<Os>,
    m: &turbonop_minidump::Module<'_>,
) -> (Option<ImageId>, Option<DebugId>) {
    let path = |name: &[u8]| String::from_utf8_lossy(name).into_owned();
    let darwin = os.is_some_and(Os::is_darwin);
    let elf = matches!(os, Some(Os::Linux | Os::Android));
    match m.code_view() {
        Some(CodeView::Elf { build_id }) => (
            Some(ImageId::BuildId(build_id.to_vec())),
            Some(DebugId::BuildId(build_id.to_vec())),
        ),
        Some(CodeView::Pdb70 { guid, .. }) if darwin => {
            let uuid = uuid_of_guid(guid);
            (Some(ImageId::Uuid(uuid)), Some(DebugId::Uuid(uuid)))
        }
        Some(CodeView::Pdb70 { guid, .. }) if elf => (
            Some(ImageId::BuildId(guid.to_vec())),
            Some(DebugId::BuildId(guid.to_vec())),
        ),
        Some(CodeView::Pdb70 { guid, age, name }) => (
            pe_id(m),
            Some(DebugId::Pdb {
                guid,
                age,
                path: path(name),
            }),
        ),
        Some(CodeView::Pdb20 {
            signature,
            age,
            name,
        }) => (
            pe_id(m),
            Some(DebugId::Pdb20 {
                signature,
                age,
                path: path(name),
            }),
        ),
        _ if darwin || elf => (None, None),
        _ => (pe_id(m), None),
    }
}

fn pe_id(m: &turbonop_minidump::Module<'_>) -> Option<ImageId> {
    (m.time_date_stamp != 0 || m.size != 0).then_some(ImageId::Pe {
        time_date_stamp: m.time_date_stamp,
        size_of_image: m.size,
    })
}

/// A UUID's bytes from a GUID's (as a minidump holds it: its first three
/// fields little-endian, its last eight bytes as they are), as Crashpad
/// writes a Mach-O UUID. (A UUID's bytes are big-endian throughout.)
pub fn uuid_of_guid(guid: [u8; 16]) -> [u8; 16] {
    let mut u = guid;
    u[..4].reverse();
    u[4..6].reverse();
    u[6..8].reverse();
    u
}
