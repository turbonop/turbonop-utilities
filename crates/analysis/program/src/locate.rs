//! Finding a module's image file from what a crash dump recorded of it
//! (its [`ImageKey`]): on this machine where it was loaded from or in the
//! symbol directories, else from the symbol servers. A file is used only
//! if it is the image the key names (its time stamp and size, build ID,
//! or UUID; in a universal file, the slice with the UUID).
//!
//! [`find_images`] finds many at once, as a dump needs: the local
//! candidates checked on every core, then what's left fetched in batches,
//! every module's first server at once, then the next for those still
//! missing.

use std::ops::Range;
use std::path::{Path, PathBuf};

use turbonop_image::Image;
use turbonop_mmap::Mmap;

pub use crate::server::ImageKey;
use crate::server::{self, Request, Servers};

/// An image found.
#[derive(Debug)]
pub struct FoundImage {
    /// Where: a file's path, or a server's URL (kept in memory).
    pub path: PathBuf,
    /// Its file's bytes.
    pub data: Mmap,
    /// The image's bytes within them (all of them, but for a universal
    /// file's slice).
    pub slice: Range<usize>,
}

impl FoundImage {
    /// The image's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.data[self.slice.clone()]
    }
}

/// Where an image wasn't found, and why, for each place tried.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotFound {
    /// Each place tried, and why it wasn't there ("(not found)", "(not
    /// this image)", "(not there)").
    pub tried: Vec<String>,
}

impl ImageKey {
    /// The part of `data` that is the image this names, if it's there (a
    /// universal file's slice; else all of it).
    pub fn matches(&self, data: &[u8]) -> Option<Range<usize>> {
        if let ImageKey::Uuid { uuid, .. } = self
            && let Some(archs) = turbonop_macho::fat_archs(data)
        {
            return archs.iter().find_map(|a| {
                let start = usize::try_from(a.offset).ok()?;
                let end = start.checked_add(usize::try_from(a.size).ok()?)?;
                let slice = data.get(start..end)?;
                let m = turbonop_macho::MachO::parse(slice).ok()?;
                (m.uuid() == Some(*uuid)).then_some(start..end)
            });
        }
        let image = Image::parse(data).ok()?;
        let all = 0..data.len();
        match self {
            ImageKey::Pe {
                time_date_stamp,
                size_of_image,
                ..
            } => {
                let pe = image.pe()?;
                let stamp = pe.file_header().time_date_stamp.get();
                let size = pe.optional_header()?.size_of_image();
                (stamp == *time_date_stamp && size == *size_of_image).then_some(all)
            }
            ImageKey::BuildId { build_id, .. } => {
                let id = image.elf()?.build_id()?;
                // (Older Breakpad recorded a build ID's first 16 bytes.)
                let same = id == build_id.as_slice()
                    || (build_id.len() == 16 && id.len() > 16 && id.starts_with(build_id));
                same.then_some(all)
            }
            ImageKey::Uuid { uuid, .. } => (image.macho()?.uuid() == Some(*uuid)).then_some(all),
        }
    }
}

/// The places on this machine to look for an image: `hints` first (where
/// the dump says it was loaded from, beside the dump), then each symbol
/// directory laid out as a symbol store (`name/<index>/name`) and flat.
pub fn local_candidates(key: &ImageKey, hints: &[PathBuf], dirs: &[PathBuf]) -> Vec<PathBuf> {
    let name = key.name();
    let mut out: Vec<PathBuf> = hints.to_vec();
    if !name.is_empty() && !name.contains("..") {
        let index = key.index();
        for d in dirs {
            out.push(d.join(name).join(&index).join(name));
            let lower = name.to_ascii_lowercase();
            if lower != name {
                out.push(d.join(&lower).join(&index).join(&lower));
            }
            out.push(d.join(name));
        }
    }
    out.dedup();
    out
}

/// The symbol directories, and the servers' cache.
fn symbol_dirs(servers: &Servers) -> Vec<PathBuf> {
    let mut dirs = turbonop_debuginfo::symbol_dirs();
    if let Some(c) = &servers.cache
        && !dirs.contains(c)
    {
        dirs.push(c.clone());
    }
    dirs
}

/// Looks for an image on this machine only.
pub fn find_local(key: &ImageKey, hints: &[PathBuf]) -> Result<FoundImage, NotFound> {
    let servers = server::servers();
    find_local_in(key, hints, &symbol_dirs(&servers))
}

fn find_local_in(
    key: &ImageKey,
    hints: &[PathBuf],
    dirs: &[PathBuf],
) -> Result<FoundImage, NotFound> {
    let mut tried = Vec::new();
    for p in local_candidates(key, hints, dirs) {
        let Ok(data) = Mmap::open_named(&p) else {
            tried.push(format!("{} (not found)", p.display()));
            continue;
        };
        match key.matches(&data) {
            Some(slice) => {
                return Ok(FoundImage {
                    path: p,
                    data,
                    slice,
                });
            }
            None => tried.push(format!("{} (not this image)", p.display())),
        }
    }
    Err(NotFound { tried })
}

/// Finds each image (with its hints: places to look first), on this
/// machine, then from the symbol servers.
pub fn find_images(wanted: &[(ImageKey, Vec<PathBuf>)]) -> Vec<Result<FoundImage, NotFound>> {
    let servers = server::servers();
    let dirs = symbol_dirs(&servers);
    // On this machine, on every core.
    let mut out: Vec<Result<FoundImage, NotFound>> =
        turbonop_par::map(wanted, 1, |(key, hints)| find_local_in(key, hints, &dirs));
    // Then the servers: round after round, each still missing asking its
    // next location, all of a round at once.
    let locations: Vec<Vec<(String, Option<PathBuf>)>> = wanted
        .iter()
        .map(|(key, _)| server::image_locations(&servers, key))
        .collect();
    let mut round = 0;
    loop {
        let asking: Vec<usize> = (0..wanted.len())
            .filter(|&i| out[i].is_err() && round < locations[i].len())
            .collect();
        if asking.is_empty() {
            break;
        }
        let requests: Vec<Request> = asking
            .iter()
            .map(|&i| {
                let (url, cached) = &locations[i][round];
                Request {
                    url: url.clone(),
                    cached: cached.clone(),
                }
            })
            .collect();
        let got = server::fetch_batch(&requests);
        // (Checked on every core: each check parses an image.)
        let pairs: Vec<(usize, &Option<Mmap>)> = asking.iter().copied().zip(&got).collect();
        let slices = turbonop_par::map(&pairs, 1, |(k, data)| {
            data.as_ref().map(|d| wanted[*k].0.matches(d))
        });
        for ((k, data), slice) in asking.iter().zip(got).zip(slices) {
            let (url, cached) = &locations[*k][round];
            let Err(nf) = &mut out[*k] else { continue };
            match (data, slice) {
                (Some(data), Some(Some(slice))) => {
                    let path = cached.clone().unwrap_or_else(|| PathBuf::from(url));
                    out[*k] = Ok(FoundImage { path, data, slice });
                }
                (Some(_), _) => nf.tried.push(format!("{url} (not this image)")),
                (None, _) => nf.tried.push(format!("{url} (not there)")),
            }
        }
        round += 1;
    }
    out
}

/// Whether `path` names a file on this machine (not a server's URL).
pub fn is_local(path: &Path) -> bool {
    !path.to_string_lossy().contains("://")
}
