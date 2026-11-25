//! Frames' symbols: each module's image (parsed once) and debug
//! information (parsed once), and the function and inlined calls at an
//! address.

use std::sync::{Mutex, OnceLock};

use turbonop_debuginfo::DebugInfo;
use turbonop_image::Image;
use turbonop_symbolize::FunctionTable;
use turbonop_target::AddressSpace;
use turbonop_target::load::Loaded;

/// A frame's function, and the calls inlined at its address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// The function (the outermost: not an inlined copy).
    pub function: String,
    /// How far into the function the address is.
    pub offset: u64,
    /// The calls inlined at the address, innermost first: each function,
    /// and where it is (the innermost's, the address's own source line;
    /// each other's, where the next was inlined into it).
    pub inlines: Vec<Inline>,
    /// The address's source file and line (in the innermost function).
    pub file: Option<String>,
    pub line: u32,
}

/// An inlined call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub function: String,
    pub file: Option<String>,
    pub line: u32,
}

/// One module's files, parsed.
struct Module<'a> {
    image: Option<Image<'a>>,
    /// What a load address less this is the image's address.
    bias: u64,
    debug: Option<DebugInfo<'a>>,
    /// The image's own functions (exports, symbols), where there's no
    /// debug information.
    functions: OnceLock<Option<FunctionTable>>,
    /// (A locator per module, behind a lock: walks on every core share
    /// them.)
    lock: Mutex<()>,
    /// 32-bit x86's frame records, from its PDB.
    x86: Option<crate::x86::X86Frames>,
}

/// Every module's files, parsed, for walking and symbolizing.
pub struct Modules<'a> {
    modules: Vec<Module<'a>>,
}

impl<'a> Modules<'a> {
    /// Parses each module's image and debug information, on every core.
    pub fn new(loaded: &'a Loaded, space: &AddressSpace<'a>, bases: &[u64]) -> Modules<'a> {
        let pairs: Vec<_> = loaded.modules.iter().zip(bases).collect();
        let modules = turbonop_par::map(&pairs, 1, |(files, base)| {
            let image = files
                .image
                .as_ref()
                .ok()
                .and_then(|f| Image::parse(f.bytes()).ok());
            let bias = space
                .image_at(**base)
                .map_or(**base, turbonop_target::LoadedImage::bias);
            let debug = match &files.program {
                Some(Ok(opened)) => opened.debug_info().ok(),
                _ => None,
            };
            // (Each module's functions, sorted, built when first asked for:
            // most modules have no frame to symbolize.)
            let functions = OnceLock::new();
            let x86 = match (&image, &files.program) {
                (Some(i), Some(Ok(opened))) if i.arch() == turbonop_core::Arch::X86 => {
                    crate::x86::X86Frames::load(opened.debug_path())
                }
                _ => None,
            };
            Module {
                image,
                bias,
                debug,
                functions,
                lock: Mutex::new(()),
                x86,
            }
        });
        Modules { modules }
    }

    /// Module `m`'s image and its bias, if its image was found.
    pub fn image(&self, m: usize) -> Option<(&Image<'a>, u64)> {
        let module = self.modules.get(m)?;
        Some((module.image.as_ref()?, module.bias))
    }

    /// Module `m`'s x86 frame records, and its bias.
    pub fn x86(&self, m: usize) -> Option<(&crate::x86::X86Frames, u64)> {
        let module = self.modules.get(m)?;
        Some((module.x86.as_ref()?, module.bias))
    }

    /// Module `m`'s debug information and its bias (what a load address
    /// less this is the debug information's address), if it was found.
    pub fn debug(&self, m: usize) -> Option<(&DebugInfo<'a>, u64)> {
        let module = self.modules.get(m)?;
        Some((module.debug.as_ref()?, module.bias))
    }

    /// The symbol at load address `address` in module `m`.
    pub fn symbol(&self, m: usize, address: u64) -> Option<Symbol> {
        let module = self.modules.get(m)?;
        let at = address.wrapping_sub(module.bias);
        let functions = module
            .functions
            .get_or_init(|| match (&module.debug, &module.image) {
                (Some(d), i) => FunctionTable::from_debug(d, i.as_ref())
                    .ok()
                    .or_else(|| i.as_ref().map(FunctionTable::from_image)),
                (None, Some(i)) => Some(FunctionTable::from_image(i)),
                (None, None) => None,
            });
        let (start, name) = functions.as_ref()?.function(at)?;
        let mut symbol = Symbol {
            function: name.to_string(),
            offset: at.wrapping_sub(start),
            inlines: Vec::new(),
            file: None,
            line: 0,
        };
        if let Some(debug) = &module.debug {
            let _guard = module
                .lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut frames = debug.locator().frames(at);
            if let Some(innermost) = frames.first() {
                symbol.file.clone_from(&innermost.file);
                symbol.line = innermost.line;
            }
            // (The last is the function itself.)
            if frames.len() > 1 {
                frames.pop();
                symbol.inlines = frames
                    .into_iter()
                    .map(|f| Inline {
                        function: f.function,
                        file: f.file,
                        line: f.line,
                    })
                    .collect();
            }
        }
        Some(symbol)
    }
}
