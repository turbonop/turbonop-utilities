//! JAR manifests (`META-INF/MANIFEST.MF`, and signature files, which have
//! the same form): the main section and per-entry sections, each a list of
//! `Name: value` attributes, values continued on lines that start with a
//! space.

use turbonop_core::{Error, ErrorKind, Result};

/// A section: its attributes, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    /// Where the section starts in the manifest.
    pub offset: usize,
    /// Its attributes: name and value (continuations joined).
    pub attributes: Vec<(String, String)>,
}

impl Section {
    /// The value of attribute `name` (names match regardless of case).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A per-entry section's `Name`.
    pub fn name(&self) -> Option<&str> {
        self.get("Name")
    }
}

/// A manifest: its main section, then the per-entry sections.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The main section.
    pub main: Section,
    /// The per-entry sections, each with a `Name`.
    pub entries: Vec<Section>,
}

/// The lines of `data` (ending CR LF, LF, or CR), each with its offset.
fn lines(data: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\r' => {
                out.push((start, &data[start..i]));
                i += if data.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                out.push((start, &data[start..i]));
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < data.len() {
        out.push((start, &data[start..]));
    }
    out
}

impl Manifest {
    /// Parses a manifest (or a signature file).
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut sections: Vec<Section> = Vec::new();
        let mut current: Option<Section> = None;
        // The main section exists even when it's empty (a manifest that
        // starts with a blank line).
        let mut seen_main = false;
        for (at, line) in lines(data) {
            if line.is_empty() {
                if let Some(s) = current.take() {
                    sections.push(s);
                } else if !seen_main {
                    sections.push(Section {
                        offset: at,
                        attributes: Vec::new(),
                    });
                }
                seen_main = true;
                continue;
            }
            let s = current.get_or_insert_with(|| Section {
                offset: at,
                attributes: Vec::new(),
            });
            seen_main = true;
            if let Some(rest) = line.strip_prefix(b" ") {
                let Some((_, v)) = s.attributes.last_mut() else {
                    return Err(Error::new(
                        ErrorKind::Malformed,
                        at as u64,
                        "continuation line with no attribute before it",
                    ));
                };
                v.push_str(&String::from_utf8_lossy(rest));
                continue;
            }
            let colon = line.iter().position(|&b| b == b':').ok_or(Error::new(
                ErrorKind::Malformed,
                at as u64,
                "manifest line without a colon",
            ))?;
            let name = &line[..colon];
            if name.is_empty()
                || !name
                    .iter()
                    .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    at as u64,
                    "manifest attribute name",
                ));
            }
            let value = line[colon + 1..]
                .strip_prefix(b" ")
                .unwrap_or(&line[colon + 1..]);
            s.attributes.push((
                String::from_utf8_lossy(name).into_owned(),
                String::from_utf8_lossy(value).into_owned(),
            ));
        }
        if let Some(s) = current.take() {
            sections.push(s);
        }
        let mut it = sections.into_iter();
        let main = it.next().unwrap_or_default();
        let entries: Vec<Section> = it.filter(|s| !s.attributes.is_empty()).collect();
        if let Some(s) = entries.iter().find(|s| s.name().is_none()) {
            return Err(Error::new(
                ErrorKind::Malformed,
                s.offset as u64,
                "manifest section without a Name",
            ));
        }
        Ok(Self { main, entries })
    }

    /// Main attribute `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.main.get(name)
    }

    /// `Main-Class`.
    pub fn main_class(&self) -> Option<&str> {
        self.get("Main-Class")
    }

    /// `Manifest-Version`.
    pub fn version(&self) -> Option<&str> {
        self.get("Manifest-Version")
    }

    /// `Created-By`.
    pub fn created_by(&self) -> Option<&str> {
        self.get("Created-By")
    }

    /// `Class-Path`'s entries.
    pub fn class_path(&self) -> Vec<&str> {
        self.get("Class-Path")
            .map(|v| v.split_ascii_whitespace().collect())
            .unwrap_or_default()
    }

    /// Whether `Multi-Release` is `true` (any case).
    pub fn is_multi_release(&self) -> bool {
        self.get("Multi-Release")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
    }

    /// The section for entry `name`.
    pub fn entry(&self, name: &str) -> Option<&Section> {
        self.entries.iter().find(|s| s.name() == Some(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_continuations() {
        let text = b"Manifest-Version: 1.0\r\nMain-Class: a.B\r\nX-Long: one two \r\n three\r\n  four\r\nMulti-Release: TRUE\r\nClass-Path: lib/a.jar  lib/b.jar\r\n\r\nName: a/B.class\r\nSHA-256-Digest: abc=\r\n\r\nName: a/very/long/name/that/wra\r\n ps.class\r\nX:\r\n\r\n";
        let m = Manifest::parse(text).unwrap();
        assert_eq!(m.version(), Some("1.0"));
        assert_eq!(m.main_class(), Some("a.B"));
        assert_eq!(m.get("x-long"), Some("one two three four"));
        assert!(m.is_multi_release());
        assert_eq!(m.class_path(), ["lib/a.jar", "lib/b.jar"]);
        assert_eq!(m.entries.len(), 2);
        assert_eq!(
            m.entry("a/B.class").unwrap().get("SHA-256-Digest"),
            Some("abc=")
        );
        let e = m.entry("a/very/long/name/that/wraps.class").unwrap();
        assert_eq!(e.get("X"), Some(""));
        assert_eq!(e.offset, 172);
        assert_eq!(m.created_by(), None);
    }

    #[test]
    fn line_ends_and_edges() {
        // LF and CR alone; no final line end.
        let m = Manifest::parse(b"A: 1\nB: 2\rC: 3").unwrap();
        assert_eq!(m.main.attributes.len(), 3);
        // Empty, and an empty main section.
        assert_eq!(Manifest::parse(b"").unwrap(), Manifest::default());
        let m = Manifest::parse(b"\nName: x\nY: 1\n").unwrap();
        assert!(m.main.attributes.is_empty());
        assert_eq!(m.entries.len(), 1);
        // Extra blank lines between sections.
        let m = Manifest::parse(b"A: 1\n\n\n\nName: x\n").unwrap();
        assert_eq!(m.entries.len(), 1);
        assert!(!m.is_multi_release());
        for bad in [
            &b" cont\n"[..],
            b"no colon\n",
            b"bad name!: x\n",
            b": x\n",
            b"A: 1\n\nB: 2\n",
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad:?}");
        }
    }
}
