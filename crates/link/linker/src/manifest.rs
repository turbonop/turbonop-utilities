//! The application manifest, as link.exe writes it: `/MANIFEST` writes it
//! beside the image (`out.exe.manifest`, or `/MANIFESTFILE`'s path);
//! `/MANIFEST:EMBED[,ID=n]` makes it a resource of the image (type 24,
//! `RT_MANIFEST`; ID 1 for an EXE, 2 for a DLL, or `n`; language 1033).
//! Without `/MANIFEST`, a manifest is written beside the image if anything
//! asks for a dependency (`/MANIFESTDEPENDENCY`, an object's directive
//! among them); `/MANIFEST:NO`, never.
//!
//! The XML is link.exe's, byte for byte: its header, the UAC section (the
//! default level `asInvoker` and `uiAccess` false, `/MANIFESTUAC`'s
//! fragment in their place, or none with `/MANIFESTUAC:NO`), then each
//! dependency (each once, the command line's first), CRLF line ends.
//! With `/MANIFESTINPUT`s, link.exe has `mt.exe` merge them with its own
//! manifest and embeds what `mt.exe` writes; turbolink runs `mt.exe` the
//! same way (`/MANIFESTINPUT` needs `/MANIFEST:EMBED`, LNK1220).

use std::path::{Path, PathBuf};

use crate::config::{Config, Manifest};
use crate::resolve::LinkError;
use crate::resources::{Id, Resource};

/// The manifest link.exe writes for `c`.
pub fn xml(c: &Config) -> Vec<u8> {
    let mut x = String::new();
    x.push_str("<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\r\n");
    x.push_str("<assembly xmlns='urn:schemas-microsoft-com:asm.v1' manifestVersion='1.0'>\r\n");
    let level = match &c.manifest_uac {
        None => Some("level='asInvoker' uiAccess='false'"),
        Some(None) => None,
        Some(Some(f)) if f.is_empty() => Some("level='asInvoker' uiAccess='false'"),
        Some(Some(f)) => Some(f.as_str()),
    };
    if let Some(level) = level {
        x.push_str("  <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">\r\n");
        x.push_str("    <security>\r\n");
        x.push_str("      <requestedPrivileges>\r\n");
        x.push_str(&format!("        <requestedExecutionLevel {level} />\r\n"));
        x.push_str("      </requestedPrivileges>\r\n");
        x.push_str("    </security>\r\n");
        x.push_str("  </trustInfo>\r\n");
    }
    let mut seen: Vec<&str> = Vec::new();
    for d in &c.manifest_dependencies {
        if seen.contains(&d.as_str()) {
            continue;
        }
        seen.push(d);
        x.push_str("  <dependency>\r\n");
        x.push_str("    <dependentAssembly>\r\n");
        x.push_str(&format!("      <assemblyIdentity {d} />\r\n"));
        x.push_str("    </dependentAssembly>\r\n");
        x.push_str("  </dependency>\r\n");
    }
    x.push_str("</assembly>\r\n");
    x.into_bytes()
}

/// What the link does with its manifest, as link.exe decides.
pub fn choice(c: &Config) -> Manifest {
    if !c.manifest_given && !c.manifest_dependencies.is_empty() {
        return Manifest::SideBySide;
    }
    c.manifest
}

/// Writes the manifest beside the image `out`, or returns it as the
/// resource to embed, as `c` asks.
pub fn make(c: &Config, out: &Path) -> Result<Option<Resource>, LinkError> {
    let error = |code: u32, message: String| LinkError {
        code,
        message,
        warning: false,
    };
    let choice = choice(c);
    if !c.manifest_inputs.is_empty() && !matches!(choice, Manifest::Embed { .. }) {
        return Err(error(
            1220,
            "'MANIFESTINPUT' requires '/MANIFEST:EMBED' specification".into(),
        ));
    }
    match choice {
        Manifest::No => Ok(None),
        Manifest::SideBySide => {
            let path = c.manifest_file.clone().unwrap_or_else(|| beside(out));
            std::fs::write(&path, xml(c))
                .map_err(|e| error(1104, format!("cannot open file '{}': {e}", path.display())))?;
            Ok(None)
        }
        Manifest::Embed { id } => {
            let mut data = xml(c);
            if !c.manifest_inputs.is_empty() {
                data = merge(&c.manifest_inputs, &data).map_err(|m| error(1327, m))?;
            }
            Ok(Some(Resource {
                ty: Id::Number(24),
                name: Id::Number(id.unwrap_or(if c.dll { 2 } else { 1 })),
                language: 0x409,
                data: Box::leak(data.into_boxed_slice()),
                // (The linker's own: from no .res file.)
                source: u32::MAX,
                converted: 0,
            }))
        }
    }
}

/// `out.exe.manifest` for `out.exe`.
fn beside(out: &Path) -> PathBuf {
    let mut name = out.as_os_str().to_owned();
    name.push(".manifest");
    PathBuf::from(name)
}

/// What `mt.exe` makes of the `inputs` and `ours` merged, run as link.exe
/// runs it: `mt.exe /nologo /out:merged /manifest inputs... ours`.
fn merge(inputs: &[PathBuf], ours: &[u8]) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir();
    let tag = format!(
        "turbolink-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let ours_path = dir.join(format!("{tag}-ours.manifest"));
    let merged = dir.join(format!("{tag}-merged.manifest"));
    std::fs::write(&ours_path, ours)
        .map_err(|e| format!("cannot write '{}': {e}", ours_path.display()))?;
    let run = std::process::Command::new("mt.exe")
        .arg("/nologo")
        .arg(format!("/out:{}", merged.display()))
        .arg("/manifest")
        .args(inputs)
        .arg(&ours_path)
        .output();
    let _ = std::fs::remove_file(&ours_path);
    let result = match run {
        Ok(o) if o.status.success() => std::fs::read(&merged).map_err(|e| e.to_string()),
        Ok(o) => Err(format!(
            "failure during running mt.exe: {}",
            String::from_utf8_lossy(&o.stdout).trim()
        )),
        Err(e) => Err(format!("failure during running mt.exe: {e}")),
    };
    let _ = std::fs::remove_file(&merged);
    result
}
