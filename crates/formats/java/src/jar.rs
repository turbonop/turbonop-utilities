//! JARs: ZIP archives of classes and resources, with a manifest
//! (`META-INF/MANIFEST.MF`), perhaps versioned classes for later releases
//! (`META-INF/versions/N/`, in a multi-release JAR), and signature files
//! (`META-INF/*.SF` and their signature blocks).

use std::borrow::Cow;
use std::collections::BTreeMap;

use turbonop_core::Result;
use turbonop_zip::{Archive, CENTRAL_HEADER_SIZE, Entry};

use crate::class::ClassFile;
use crate::manifest::Manifest;

/// The manifest's name.
pub const MANIFEST_NAME: &str = "META-INF/MANIFEST.MF";

/// A class entry of a JAR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassEntry {
    /// The entry's index in the archive.
    pub index: usize,
    /// The entry's name (`a/b/C.class`, `META-INF/versions/17/a/b/C.class`).
    pub path: String,
    /// The class's name from its path (internal form: `a/b/C`).
    pub class_name: String,
    /// The release it's for (a multi-release JAR's `META-INF/versions/N`),
    /// or `None` for the base.
    pub release: Option<u32>,
    /// Its compressed size.
    pub compressed_size: u64,
    /// Its uncompressed size (the class file's).
    pub uncompressed_size: u64,
    /// What the archive spends on it besides its data: its local header,
    /// data descriptor, and central directory record.
    pub overhead: u64,
}

impl ClassEntry {
    /// The package (internal form; empty for the unnamed package).
    pub fn package(&self) -> &str {
        self.class_name
            .rfind('/')
            .map_or("", |i| &self.class_name[..i])
    }

    /// What it takes in the archive, all told.
    pub fn archive_size(&self) -> u64 {
        self.compressed_size + self.overhead
    }
}

/// What a signature-related file under `META-INF` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureFileKind {
    /// A signature file (`*.SF`): the manifest's digests, signed.
    SignatureFile,
    /// A signature block (`*.RSA`, `*.DSA`, `*.EC`): PKCS #7 over a
    /// signature file.
    SignatureBlock,
    /// Some other signing file (`SIG-*`).
    Other,
}

/// A signature-related file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureFile {
    /// The entry's index.
    pub index: usize,
    /// Its name.
    pub path: String,
    /// What it is.
    pub kind: SignatureFileKind,
}

/// The sizes of one package's classes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageSize {
    /// The package (internal form).
    pub package: String,
    /// Its classes.
    pub classes: usize,
    /// Their compressed data.
    pub compressed: u64,
    /// Their class files.
    pub uncompressed: u64,
    /// The archive's overhead for them.
    pub overhead: u64,
}

/// A JAR.
#[derive(Debug)]
pub struct Jar<'d> {
    archive: Archive<'d>,
    manifest: Option<Result<Manifest>>,
}

/// The release a `META-INF/versions/N/...` path is for, and the path under
/// it.
fn versioned(path: &str) -> Option<(u32, &str)> {
    let rest = path.strip_prefix("META-INF/versions/")?;
    let (n, rest) = rest.split_once('/')?;
    Some((n.parse().ok()?, rest))
}

impl<'d> Jar<'d> {
    /// Reads `data` as a ZIP archive, and its manifest if it has one.
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        Ok(Self::from_archive(Archive::parse(data)?))
    }

    /// A JAR of an archive already read.
    pub fn from_archive(archive: Archive<'d>) -> Self {
        let manifest = archive
            .entries()
            .iter()
            .find(|e| e.name().eq_ignore_ascii_case(MANIFEST_NAME))
            .map(|e| archive.read(e).and_then(|d| Manifest::parse(&d)));
        Self { archive, manifest }
    }

    /// The archive.
    pub fn archive(&self) -> &Archive<'d> {
        &self.archive
    }

    /// The manifest, if there is one and it reads.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()?.as_ref().ok()
    }

    /// Why the manifest doesn't read, if it doesn't.
    pub fn manifest_error(&self) -> Option<turbonop_core::Error> {
        self.manifest.as_ref()?.as_ref().err().copied()
    }

    /// The manifest's `Main-Class`.
    pub fn main_class(&self) -> Option<&str> {
        self.manifest()?.main_class()
    }

    /// Whether the manifest says it's a multi-release JAR.
    pub fn is_multi_release(&self) -> bool {
        self.manifest().is_some_and(Manifest::is_multi_release)
    }

    /// The releases `META-INF/versions` has classes for, in order.
    pub fn releases(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .archive
            .entries()
            .iter()
            .filter_map(|e| versioned(&e.name()).map(|(n, _)| n))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn class_entry(&self, e: &Entry<'d>) -> Option<ClassEntry> {
        let name = e.name();
        if e.is_dir() || !name.ends_with(".class") {
            return None;
        }
        let (release, path) = match versioned(&name) {
            Some((n, rest)) => (Some(n), rest),
            None if name.starts_with("META-INF/") => {
                // Only versioned classes count under META-INF (and
                // module-info under a version).
                return None;
            }
            None => (None, &*name),
        };
        let class_name = path.strip_suffix(".class").unwrap_or(path).to_string();
        let local = self.archive.extent(e).map_or(0, |x| {
            (x.data.start - x.local_header.start) + x.descriptor.map_or(0, |d| d.end - d.start)
        });
        let central =
            (CENTRAL_HEADER_SIZE + e.raw_name.len() + e.extra.len() + e.raw_comment.len()) as u64;
        Some(ClassEntry {
            index: e.index,
            path: name.into_owned(),
            class_name,
            release,
            compressed_size: e.compressed_size,
            uncompressed_size: e.uncompressed_size,
            overhead: local + central,
        })
    }

    /// The class entries (`*.class` outside `META-INF`, and the versioned
    /// ones in it), in the archive's order.
    pub fn classes(&self) -> Vec<ClassEntry> {
        self.archive
            .entries()
            .iter()
            .filter_map(|e| self.class_entry(e))
            .collect()
    }

    /// The class entries a JVM of `release` would load: each class's
    /// highest version at or below `release`, else its base (in name
    /// order).
    pub fn classes_for_release(&self, release: u32) -> Vec<ClassEntry> {
        let mut best: BTreeMap<String, ClassEntry> = BTreeMap::new();
        let multi = self.is_multi_release();
        for c in self.classes() {
            let ok = match c.release {
                None => true,
                Some(r) => multi && r <= release,
            };
            if !ok {
                continue;
            }
            let better = best
                .get(&c.class_name)
                .is_none_or(|b| b.release.unwrap_or(0) <= c.release.unwrap_or(0));
            if better {
                best.insert(c.class_name.clone(), c);
            }
        }
        best.into_values().collect()
    }

    /// The signature files and blocks under `META-INF`.
    pub fn signature_files(&self) -> Vec<SignatureFile> {
        self.archive
            .entries()
            .iter()
            .filter_map(|e| {
                let name = e.name();
                let file = name.strip_prefix("META-INF/")?;
                if file.contains('/') {
                    return None;
                }
                let upper = file.to_ascii_uppercase();
                let kind = if upper.ends_with(".SF") {
                    SignatureFileKind::SignatureFile
                } else if upper.ends_with(".RSA")
                    || upper.ends_with(".DSA")
                    || upper.ends_with(".EC")
                {
                    SignatureFileKind::SignatureBlock
                } else if upper.starts_with("SIG-") {
                    SignatureFileKind::Other
                } else {
                    return None;
                };
                Some(SignatureFile {
                    index: e.index,
                    path: name.into_owned(),
                    kind,
                })
            })
            .collect()
    }

    /// Whether it has a signature file and block.
    pub fn is_signed(&self) -> bool {
        let s = self.signature_files();
        s.iter().any(|f| f.kind == SignatureFileKind::SignatureFile)
            && s.iter()
                .any(|f| f.kind == SignatureFileKind::SignatureBlock)
    }

    /// The signature file `path` (`*.SF`), parsed: a manifest of the
    /// manifest's digests.
    pub fn signature_file(&self, path: &str) -> Option<Result<Manifest>> {
        let d = self.archive.read_by_name(path)?;
        Some(d.and_then(|d| Manifest::parse(&d)))
    }

    /// `f` of each class entry's data, on every core (the largest first):
    /// for reading or parsing every class.
    pub fn par_map_classes<R: Send>(
        &self,
        f: impl Fn(&ClassEntry, Result<Cow<'d, [u8]>>) -> R + Sync,
    ) -> Vec<R>
    where
        Self: Sync,
    {
        let classes = self.classes();
        let entries = self.archive.entries();
        turbonop_par::map_costed(
            classes.len(),
            |i| classes[i].uncompressed_size + 64,
            |i| {
                let c = &classes[i];
                f(c, self.archive.read(&entries[c.index]))
            },
        )
    }

    /// Every class parsed on every core: each entry with its class's name
    /// (as the class file says it), version, and any error.
    pub fn read_classes(&self) -> Vec<(ClassEntry, Result<ClassSummary>)>
    where
        Self: Sync,
    {
        self.par_map_classes(|c, data| {
            let r = data.and_then(|d| ClassFile::parse(&d).and_then(|cf| ClassSummary::of(&cf)));
            (c.clone(), r)
        })
    }

    /// The classes' sizes by package (the base classes and every
    /// version's), in package order.
    pub fn package_sizes(&self) -> Vec<PackageSize> {
        let mut by: BTreeMap<String, PackageSize> = BTreeMap::new();
        for c in self.classes() {
            let p = by
                .entry(c.package().to_string())
                .or_insert_with(|| PackageSize {
                    package: c.package().to_string(),
                    ..PackageSize::default()
                });
            p.classes += 1;
            p.compressed += c.compressed_size;
            p.uncompressed += c.uncompressed_size;
            p.overhead += c.overhead;
        }
        by.into_values().collect()
    }
}

/// What a class file holds, in brief (owned: for many classes at once).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassSummary {
    /// Its name (internal form).
    pub name: String,
    /// Its major version.
    pub major_version: u16,
    /// Its minor version.
    pub minor_version: u16,
    /// Its fields.
    pub fields: usize,
    /// Its methods, each with its name, descriptor, and size in the class
    /// file.
    pub methods: Vec<(String, String, u64)>,
    /// Its constant pool's size in bytes.
    pub constant_pool_size: u64,
    /// The attributes that didn't read.
    pub malformed_attributes: usize,
}

impl ClassSummary {
    /// The summary of `cf`.
    pub fn of(cf: &ClassFile<'_>) -> Result<Self> {
        let cp = &cf.constant_pool;
        let methods = cf
            .methods
            .iter()
            .map(|m| {
                Ok((
                    m.name(cp)?.into_owned(),
                    m.descriptor(cp)?.into_owned(),
                    m.size,
                ))
            })
            .collect::<Result<_>>()?;
        let l = cf.layout();
        Ok(Self {
            name: cf.name()?.into_owned(),
            major_version: cf.major_version,
            minor_version: cf.minor_version,
            fields: cf.fields.len(),
            methods,
            constant_pool_size: l.constant_pool.end - l.constant_pool.start,
            malformed_attributes: cf.malformed_attributes().len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_paths() {
        assert_eq!(
            versioned("META-INF/versions/17/a/B.class"),
            Some((17, "a/B.class"))
        );
        assert_eq!(versioned("META-INF/versions/x/a/B.class"), None);
        assert_eq!(versioned("META-INF/versions/9"), None);
        assert_eq!(versioned("a/B.class"), None);
    }
}
