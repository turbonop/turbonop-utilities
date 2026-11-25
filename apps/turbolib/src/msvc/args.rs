//! Command-line words: response files and the `_LIB_` environment variable,
//! split into words as `lib.exe` and `llvm-lib` split them.
//!
//! The two tools differ (measured with `lib.exe` 14.52 and read in LLVM's
//! `LibDriver.cpp`):
//!
//! - `lib.exe` expands `@file` only on the command line itself: an `@word`
//!   inside a response file is an input named `@word`. In a response file,
//!   words are separated by spaces, tabs, and line breaks, and a double
//!   quote anywhere in a word toggles quoting and is dropped (`q"uo"te` is
//!   `quote`); a backslash is an ordinary character. `lib.exe` then reads
//!   the `_LIB_` environment variable, split the same way, after the
//!   command line, so its options win. (`LIB`, `LINK`, and `_LINK_` are not
//!   read for options.)
//! - `llvm-lib` expands `@file` anywhere, including inside response files,
//!   and splits a response file as the Windows C runtime splits a command
//!   line (backslashes escape quotes). It reads no environment variable for
//!   options.

use std::io;

/// Which tool's rules to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Microsoft's `lib.exe` (and `link /lib`).
    Lib,
    /// LLVM's `llvm-lib` (and `lld-link /lib`).
    LlvmLib,
}

/// A response file that couldn't be read.
#[derive(Debug)]
pub struct RspError {
    pub path: String,
    pub error: io::Error,
}

/// Expands the response files in `args` (the arguments after the program's
/// name) and, for `lib.exe`, appends `_LIB_`'s words. `read` reads a file;
/// `env` is `_LIB_`'s value, if set.
pub fn expand(
    flavor: Flavor,
    args: &[String],
    env: Option<&str>,
    read: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
) -> Result<Vec<String>, RspError> {
    let mut out = Vec::new();
    match flavor {
        Flavor::Lib => {
            for a in args {
                match a.strip_prefix('@') {
                    Some(path) => out.extend(split_lib(&read_rsp(path, read)?)),
                    None => out.push(a.clone()),
                }
            }
            if let Some(env) = env {
                out.extend(split_lib(env));
            }
        }
        Flavor::LlvmLib => {
            let mut stack = Vec::new();
            expand_llvm(args, &mut out, read, &mut stack)?;
        }
    }
    Ok(out)
}

/// `llvm-lib`'s expansion, as LLVM's `ExpansionContext::expandResponseFiles`
/// does it: a response file that doesn't exist is left as the word `@file`
/// (which then names a missing input), and one that is already being
/// expanded is an error. Nested names are relative to the current
/// directory, not to the file naming them.
fn expand_llvm(
    args: &[String],
    out: &mut Vec<String>,
    read: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
    stack: &mut Vec<String>,
) -> Result<(), RspError> {
    for a in args {
        let Some(path) = a.strip_prefix('@') else {
            out.push(a.clone());
            continue;
        };
        let key = path.replace('/', "\\").to_lowercase();
        if stack.contains(&key) {
            return Err(RspError {
                path: path.to_string(),
                error: io::Error::other(format!("recursive expansion of: '{path}'")),
            });
        }
        let bytes = match read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                out.push(a.clone());
                continue;
            }
            Err(error) => {
                return Err(RspError {
                    path: path.to_string(),
                    error,
                });
            }
        };
        let words = split_windows(&decode(&bytes));
        stack.push(key);
        expand_llvm(&words, out, read, stack)?;
        stack.pop();
    }
    Ok(())
}

fn read_rsp(
    path: &str,
    read: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
) -> Result<String, RspError> {
    let bytes = read(path).map_err(|error| RspError {
        path: path.to_string(),
        error,
    })?;
    Ok(decode(&bytes))
}

/// A response file's text: UTF-16 (little- or big-endian) after its byte
/// order mark, otherwise UTF-8 (with or without a mark). Bytes that aren't
/// valid UTF-8 are replaced, as no tool's paths can hold them.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], le: bool| {
        let units = bytes.as_chunks::<2>().0.iter().map(|&c| {
            if le {
                u16::from_le_bytes(c)
            } else {
                u16::from_be_bytes(c)
            }
        });
        char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0B' | '\x0C')
}

/// Splits text into words as `lib.exe` splits a response file.
pub fn split_lib(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
            in_word = true;
        } else if is_space(c) && !quoted {
            if in_word {
                words.push(std::mem::take(&mut word));
                in_word = false;
            }
        } else {
            word.push(c);
            in_word = true;
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Splits text into words as LLVM's `cl::TokenizeWindowsCommandLine` does:
/// the Windows C runtime's rules, with line breaks as spaces. Outside
/// quotes, `"` starts quoting. Inside, `""` is a literal quote and `"`
/// ends quoting. A run of n backslashes followed by `"` is n/2 backslashes,
/// and then, if n is odd, a literal quote; backslashes not followed by a
/// quote are themselves.
pub fn split_windows(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let run = chars[i..].iter().take_while(|&&c| c == '\\').count();
            i += run;
            in_word = true;
            if chars.get(i) == Some(&'"') {
                word.extend(std::iter::repeat_n('\\', run / 2));
                if run % 2 == 1 {
                    word.push('"');
                    i += 1;
                }
            } else {
                word.extend(std::iter::repeat_n('\\', run));
            }
            continue;
        }
        if c == '"' {
            in_word = true;
            if quoted && chars.get(i + 1) == Some(&'"') {
                word.push('"');
                i += 2;
                continue;
            }
            quoted = !quoted;
        } else if is_space(c) && !quoted {
            if in_word {
                words.push(std::mem::take(&mut word));
                in_word = false;
            }
        } else {
            word.push(c);
            in_word = true;
        }
        i += 1;
    }
    if in_word {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests;
