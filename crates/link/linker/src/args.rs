//! The command line, as `link.exe` reads it: words split by the Windows
//! rules, `@file` response files expanded in place, the `LINK`
//! environment variable's words before the command line's and `_LINK_`'s
//! after, and each word an option (`/name[:value]` or `-name[:value]`,
//! the name in any case) or an input file. Objects' `.drectve` sections
//! hold options in the same form ([`directives`]).

use std::fmt;

/// One word of the command line: an option or an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// `/name[:value]` (or `-name[:value]`).
    Option {
        /// The name, in lower case (`out`, `defaultlib`, `opt`).
        name: String,
        /// What follows the first `:`, as written (`/OPT:REF,ICF` has
        /// `REF,ICF`); `None` without a `:`.
        value: Option<String>,
    },
    /// An input: an object, a library, a `.res`, a `.def` (a word that
    /// isn't an option).
    Input(String),
}

impl Arg {
    /// The word, classified: an option if it starts with `/` or `-`.
    pub fn parse(word: &str) -> Arg {
        let Some(rest) = word.strip_prefix(['/', '-']) else {
            return Arg::Input(word.to_owned());
        };
        match rest.split_once(':') {
            Some((name, value)) => Arg::Option {
                name: name.to_ascii_lowercase(),
                value: Some(value.to_owned()),
            },
            None => Arg::Option {
                name: rest.to_ascii_lowercase(),
                value: None,
            },
        }
    }
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arg::Option { name, value: None } => write!(f, "/{}", name.to_ascii_uppercase()),
            Arg::Option {
                name,
                value: Some(v),
            } => write!(f, "/{}:{v}", name.to_ascii_uppercase()),
            Arg::Input(path) => f.write_str(path),
        }
    }
}

/// Splits `text` into words by the rules of the Microsoft C runtime (as
/// `link.exe`'s own command line and its response files are split):
/// words are separated by white space outside double quotes; quotes
/// group and are removed; `""` inside quotes is a quote; backslashes are
/// literal, except before a quote, where each pair is one backslash and
/// an odd one makes the quote literal.
pub fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // Whether a word has begun (so that `""` is an empty word).
    let mut started = false;
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut n = 1;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    n += 1;
                }
                started = true;
                if chars.peek() == Some(&'"') {
                    word.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        chars.next();
                        word.push('"');
                    }
                } else {
                    word.extend(std::iter::repeat_n('\\', n));
                }
            }
            '"' => {
                started = true;
                if quoted && chars.peek() == Some(&'"') {
                    chars.next();
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
            }
            ' ' | '\t' | '\r' | '\n' if !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// The text of a response file: UTF-16 (little- or big-endian) or UTF-8
/// by its byte-order mark; without one, UTF-8 (bytes that aren't are
/// replaced, as an ANSI file's non-ASCII bytes would otherwise be lost).
pub fn decode_text(bytes: &[u8]) -> String {
    fn utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&p| word(p)).collect();
        String::from_utf16_lossy(&units)
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        String::from_utf8_lossy(rest).into_owned()
    }
}

/// Response files can name response files, this deep.
const MAX_DEPTH: usize = 32;

/// A command line that couldn't be read.
#[derive(Debug)]
pub struct ArgsError {
    /// What went wrong, as `link.exe`'s message would say it.
    pub message: String,
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ArgsError {}

/// The words of `words` with each `@file` replaced by the words of the
/// file `read` returns for it (and so on in those).
pub fn expand_response_files(
    words: impl IntoIterator<Item = String>,
    read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<String>, ArgsError> {
    fn go(
        words: impl IntoIterator<Item = String>,
        read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
        depth: usize,
        out: &mut Vec<String>,
    ) -> Result<(), ArgsError> {
        for word in words {
            let Some(path) = word.strip_prefix('@') else {
                out.push(word);
                continue;
            };
            if depth >= MAX_DEPTH {
                return Err(ArgsError {
                    message: format!("response files nested too deeply at '@{path}'"),
                });
            }
            let bytes = read(path).map_err(|e| ArgsError {
                message: format!("cannot open response file '{path}': {e}"),
            })?;
            go(split_words(&decode_text(&bytes)), read, depth + 1, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(words, read, 0, &mut out)?;
    Ok(out)
}

/// The whole command line, as `link.exe` reads it: `link_env`'s words
/// (the `LINK` variable), then `argv`'s (without the program's name),
/// then `link_env_after`'s (`_LINK_`); response files expanded
/// throughout.
pub fn command_line(
    argv: impl IntoIterator<Item = String>,
    link_env: Option<&str>,
    link_env_after: Option<&str>,
    read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<Arg>, ArgsError> {
    let mut words = Vec::new();
    words.extend(link_env.map(split_words).unwrap_or_default());
    words.extend(argv);
    words.extend(link_env_after.map(split_words).unwrap_or_default());
    Ok(expand_response_files(words, read)?
        .iter()
        .map(|w| Arg::parse(w))
        .collect())
}

/// The options of a `.drectve` section's text (an object's directives to
/// the linker: `/DEFAULTLIB:"LIBCMT" /EXPORT:f`). Directives are options
/// only; a word that isn't one is returned as an [`Arg::Input`], for the
/// caller to warn of (as `link.exe` does, LNK4229).
pub fn directives(text: &[u8]) -> Vec<Arg> {
    // A directive section is ANSI or UTF-8 text, sometimes with a UTF-8
    // byte-order mark; NULs pad it, and separate words as spaces do (as
    // link.exe reads them: `-export:f\0-export:g`).
    let text = decode_text(text).replace('\0', " ");
    split_words(&text)
        .iter()
        .filter(|w| !w.is_empty())
        .map(|w| Arg::parse(w))
        .collect()
}

#[cfg(test)]
mod tests;
