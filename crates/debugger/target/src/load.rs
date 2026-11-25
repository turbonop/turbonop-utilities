//! A target's modules' files: each module's image (found by the identity
//! the target recorded, on this machine or from the symbol servers) and
//! its debug information, all found at once, and the address space over
//! the target's memory and the images.
//!
//! Every module's image is looked for at once (see
//! `turbonop_program::locate::find_images`: local checks on every core,
//! then batches of fetches), then every image's debug information is
//! opened on every core.

use std::path::{Path, PathBuf};

use turbonop_image::Image;
use turbonop_mmap::Mmap;
use turbonop_program::Opened;
use turbonop_program::locate::{FoundImage, ImageKey, NotFound, find_images, find_images_in};

use crate::model::{ImageId, Module, Target};
use crate::{AddressSpace, LoadedImage};

/// Where to look for modules' files, besides where the target says they
/// were loaded from and the symbol directories.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// Directories that may hold modules' files by name (the dump's own,
    /// a build's output).
    pub dirs: Vec<PathBuf>,
    /// Directories that stand for the target's root, its modules' paths
    /// under them (a Linux system's files from Windows: `\\wsl$\Ubuntu`; a
    /// device's copied system).
    pub sysroots: Vec<PathBuf>,
    /// Whether to look where the target says each module was loaded from
    /// (on this machine: right for a dump written here).
    pub skip_recorded_paths: bool,
    /// The symbol directories and servers to use, if not the
    /// environment's (`_NT_SYMBOL_PATH`, `TURBONOP_SYMBOL_SERVER`, ...).
    pub symbols: Option<(Vec<PathBuf>, turbonop_program::server::Servers)>,
}

/// A module's files.
pub struct ModuleFiles {
    /// Its image, or where it was looked for (or, if the target recorded
    /// no identity for it, why it couldn't be).
    pub image: Result<FoundImage, NotFound>,
    /// Its image and debug information, opened, if its image was found:
    /// or why its debug information wasn't.
    pub program: Option<Result<Opened, turbonop_program::Error>>,
}

/// A target's modules' files, in the target's module order.
pub struct Loaded {
    pub modules: Vec<ModuleFiles>,
}

/// The key the target's identity of a module gives its image.
pub fn image_key(module: &Module) -> Option<ImageKey> {
    let name = module.file_name().to_string();
    Some(match module.image_id.as_ref()? {
        ImageId::Pe {
            time_date_stamp,
            size_of_image,
        } => ImageKey::Pe {
            name,
            time_date_stamp: *time_date_stamp,
            size_of_image: *size_of_image,
        },
        ImageId::BuildId(b) => ImageKey::BuildId {
            name,
            build_id: b.clone(),
        },
        ImageId::Uuid(u) => ImageKey::Uuid { name, uuid: *u },
    })
}

/// The places to look for a module first.
fn hinted(module: &Module, hints: &Hints) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !hints.skip_recorded_paths {
        out.push(PathBuf::from(&module.path));
    }
    // (Its path as the target had it, under each root.)
    let relative = module.path.trim_start_matches(['/', '\\']);
    let relative = relative
        .split_once(":\\")
        .map_or(relative, |(_, rest)| rest);
    for root in &hints.sysroots {
        out.push(root.join(relative.replace('\\', "/")));
    }
    for d in &hints.dirs {
        out.push(d.join(module.file_name()));
    }
    out
}

/// Finds every module's files.
pub fn load(target: &dyn Target, hints: &Hints) -> Loaded {
    let modules = target.modules();
    let keys: Vec<Option<ImageKey>> = modules.iter().map(image_key).collect();
    let wanted: Vec<(ImageKey, Vec<PathBuf>)> = modules
        .iter()
        .zip(&keys)
        .filter_map(|(m, k)| Some((k.clone()?, hinted(m, hints))))
        .collect();
    let found = match &hints.symbols {
        Some((dirs, servers)) => find_images_in(&wanted, dirs, servers),
        None => find_images(&wanted),
    };
    let mut found = found.into_iter();
    let images: Vec<Result<FoundImage, NotFound>> = keys
        .iter()
        .map(|k| match k {
            Some(_) => found.next().unwrap_or_else(|| Err(NotFound::default())),
            None => Err(NotFound {
                tried: vec!["(the target recorded no identity for it)".into()],
            }),
        })
        .collect();
    // Each image's debug information, on every core.
    let programs = turbonop_par::map(&images, 1, |image| {
        let found = image.as_ref().ok()?;
        let data = if found.slice == (0..found.data.len()) {
            Mmap::open(&found.path).unwrap_or_else(|_| Mmap::from(found.bytes().to_vec()))
        } else {
            Mmap::from(found.bytes().to_vec())
        };
        Some(Opened::open_mapped(&found.path, data, None))
    });
    Loaded {
        modules: images
            .into_iter()
            .zip(programs)
            .map(|(image, program)| ModuleFiles { image, program })
            .collect(),
    }
}

impl Loaded {
    /// The target's address space: its memory, with each image found
    /// standing in for what it didn't capture (built on every core: each
    /// image's loader's words are read).
    pub fn address_space<'a>(&'a self, target: &'a dyn Target) -> AddressSpace<'a> {
        let pairs: Vec<(&FoundImage, u64)> = self
            .modules
            .iter()
            .zip(target.modules())
            .filter_map(|(f, m)| Some((f.image.as_ref().ok()?, m.base)))
            .collect();
        let images = turbonop_par::map(&pairs, 1, |(found, base)| {
            let image = Image::parse(found.bytes()).ok()?;
            LoadedImage::new(&image, *base)
        });
        AddressSpace::new(target.memory(), images.into_iter().flatten().collect())
    }

    /// How many modules' images, and debug information, were found.
    pub fn counts(&self) -> (usize, usize) {
        let images = self.modules.iter().filter(|m| m.image.is_ok()).count();
        let debug = self
            .modules
            .iter()
            .filter(|m| matches!(m.program, Some(Ok(_))))
            .count();
        (images, debug)
    }
}

/// Whether `path` is a directory a dump's modules may be beside.
pub fn dump_dir(dump: &Path) -> Option<PathBuf> {
    dump.parent().map(Path::to_path_buf)
}
