//! ReadyToRun images (native code compiled ahead of time beside the IL,
//! as crossgen2 writes them): the `READYTORUN_HEADER` the CLI header's
//! ManagedNativeHeader points to, its flags, and its sections.
//!
//! Decoding the sections themselves (method entry points, fixups, the
//! runtime functions, the inlining and debug information) is not done
//! yet; composite images, whose header is found by the `RTR_HEADER`
//! export instead, aren't either.

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;

/// The header's signature, `RTR\0`.
pub const READYTORUN_SIGNATURE: u32 = 0x0052_5452;

/// Header flags (`READYTORUN_FLAG_*`).
pub mod flags {
    /// Compiled from a platform-neutral (AnyCPU) assembly.
    pub const PLATFORM_NEUTRAL_SOURCE: u32 = 0x0000_0001;
    /// Type validation was skipped.
    pub const SKIP_TYPE_VALIDATION: u32 = 0x0000_0002;
    /// Only some methods are compiled.
    pub const PARTIAL: u32 = 0x0000_0004;
    /// P/Invoke stubs are compiled into the image.
    pub const NONSHARED_PINVOKE_STUBS: u32 = 0x0000_0008;
    /// The IL is embedded (a component of a composite image).
    pub const EMBEDDED_MSIL: u32 = 0x0000_0010;
    /// A component of a composite image.
    pub const COMPONENT: u32 = 0x0000_0020;
    /// Its version bubble spans modules.
    pub const MULTIMODULE_VERSION_BUBBLE: u32 = 0x0000_0040;
    /// It holds code for methods it doesn't define.
    pub const UNRELATED_R2R_CODE: u32 = 0x0000_0080;
}

/// The flags' names.
pub const FLAG_NAMES: &[(u32, &str)] = &[
    (flags::PLATFORM_NEUTRAL_SOURCE, "platform-neutral source"),
    (flags::SKIP_TYPE_VALIDATION, "skip type validation"),
    (flags::PARTIAL, "partial"),
    (flags::NONSHARED_PINVOKE_STUBS, "non-shared P/Invoke stubs"),
    (flags::EMBEDDED_MSIL, "embedded IL"),
    (flags::COMPONENT, "component"),
    (
        flags::MULTIMODULE_VERSION_BUBBLE,
        "multi-module version bubble",
    ),
    (flags::UNRELATED_R2R_CODE, "unrelated code"),
];

/// A section type's name (`ReadyToRunSectionType`).
pub fn section_name(ty: u32) -> Option<&'static str> {
    Some(match ty {
        100 => "CompilerIdentifier",
        101 => "ImportSections",
        102 => "RuntimeFunctions",
        103 => "MethodDefEntryPoints",
        104 => "ExceptionInfo",
        105 => "DebugInfo",
        106 => "DelayLoadMethodCallThunks",
        107 => "AvailableTypes (obsolete)",
        108 => "AvailableTypes",
        109 => "InstanceMethodEntryPoints",
        110 => "InliningInfo",
        111 => "ProfileDataInfo",
        112 => "ManifestMetadata",
        113 => "AttributePresence",
        114 => "InliningInfo2",
        115 => "ComponentAssemblies",
        116 => "OwnerCompositeExecutable",
        117 => "PgoInstrumentationData",
        118 => "ManifestAssemblyMvids",
        119 => "CrossModuleInlineInfo",
        120 => "HotColdMap",
        121 => "MethodIsGenericMap",
        122 => "EnclosingTypeMap",
        123 => "TypeGenericInfoMap",
        _ => return None,
    })
}

/// A section: its type, and where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadyToRunSection {
    /// Its type (100 and up).
    pub ty: u32,
    /// Its RVA.
    pub rva: u32,
    /// Its size.
    pub size: u32,
}

/// The header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyToRunHeader {
    /// Major version (5 and up: .NET Core 3 and later).
    pub major_version: u16,
    /// Minor version.
    pub minor_version: u16,
    /// Flags ([`flags`]).
    pub flags: u32,
    /// The sections.
    pub sections: Vec<ReadyToRunSection>,
}

impl ReadyToRunHeader {
    /// Parses a header (its signature first).
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(data);
        if c.u32("ReadyToRun signature")? != READYTORUN_SIGNATURE {
            return Err(c.error(ErrorKind::BadMagic, "ReadyToRun signature"));
        }
        let major_version = c.u16("ReadyToRun header")?;
        let minor_version = c.u16("ReadyToRun header")?;
        let flags = c.u32("ReadyToRun header")?;
        let n = c.u32("ReadyToRun section count")? as usize;
        let n = c.check_count(n, 12, "ReadyToRun section count")?;
        let mut sections = Vec::with_capacity(n);
        for _ in 0..n {
            sections.push(ReadyToRunSection {
                ty: c.u32("ReadyToRun section")?,
                rva: c.u32("ReadyToRun section")?,
                size: c.u32("ReadyToRun section")?,
            });
        }
        Ok(Self {
            major_version,
            minor_version,
            flags,
            sections,
        })
    }

    /// The section of type `ty`.
    pub fn section(&self, ty: u32) -> Option<&ReadyToRunSection> {
        self.sections.iter().find(|s| s.ty == ty)
    }

    /// The names of its flags that are set.
    pub fn flag_names(&self) -> Vec<&'static str> {
        FLAG_NAMES
            .iter()
            .filter(|(bit, _)| self.flags & bit != 0)
            .map(|&(_, n)| n)
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn header() {
        let mut d = READYTORUN_SIGNATURE.to_le_bytes().to_vec();
        d.extend_from_slice(&9u16.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&(flags::PLATFORM_NEUTRAL_SOURCE | flags::PARTIAL).to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        for v in [100u32, 0x2000, 0x10] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let h = ReadyToRunHeader::parse(&d).unwrap();
        assert_eq!((h.major_version, h.minor_version), (9, 2));
        assert_eq!(h.flag_names(), ["platform-neutral source", "partial"]);
        assert_eq!(h.section(100).unwrap().rva, 0x2000);
        assert_eq!(section_name(100), Some("CompilerIdentifier"));
        for n in 0..d.len() {
            assert!(ReadyToRunHeader::parse(&d[..n]).is_err());
        }
        d[12] = 0xFF;
        assert!(ReadyToRunHeader::parse(&d).is_err());
        assert!(ReadyToRunHeader::parse(b"RTR").is_err());
    }
}
