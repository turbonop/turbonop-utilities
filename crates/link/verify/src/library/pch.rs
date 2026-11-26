//! turbolib's precompiled-types members (`--precomp=library`) against their
//! users, as link.exe pairs them.
//!
//! link.exe pairs a user's `LF_PRECOMP` (a path and a signature) with a PCH
//! object whose signature equals that signature and whose `S_OBJNAME` path,
//! or the name it is linked under (its file or member name), equals that
//! path, ignoring case; if none does, it warns LNK4206 and links the user
//! without its types (measured with link.exe 14.52). turbolib writes the
//! PCH member's `S_OBJNAME` as the name its users give, so both hold; this
//! checks that the `S_OBJNAME` one does, which survives re-archiving. lld-link pairs by signature (and by file name when no PCH has
//! the signature) and checks the type count. So each user of a turbolib
//! PCH member must name a member of the library whose `S_OBJNAME` path and
//! signature match, whose `LF_ENDPRECOMP` signature matches too, and whose
//! records before `LF_ENDPRECOMP` number what the user's `LF_PRECOMP` says.

use turbonop_coff::{File, Object};

/// The file names turbolib gives its precompiled-types members.
const PREFIX: &[u8] = b"turbolib-types-";

const CV_SIGNATURE_C13: u32 = 4;
const DEBUG_S_SYMBOLS: u32 = 0xF1;
const S_OBJNAME: u16 = 0x1101;
const LF_PRECOMP: u16 = 0x1509;
const LF_ENDPRECOMP: u16 = 0x0014;

/// A user's `LF_PRECOMP`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    pub path: Vec<u8>,
    pub signature: u32,
    pub count: u32,
}

/// A PCH object's identity, as link.exe sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pch {
    /// `S_OBJNAME`'s path and signature.
    pub path: Vec<u8>,
    pub signature: u32,
    /// `LF_ENDPRECOMP`'s signature, and how many records come before it.
    pub end_signature: u32,
    pub count: u32,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// The NUL-terminated string at `o`.
fn string_at(b: &[u8], o: usize) -> Option<Vec<u8>> {
    let s = b.get(o..)?;
    Some(s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec())
}

fn object(data: &[u8]) -> Option<Object<'_>> {
    match File::parse(data).ok()? {
        File::Object(o) => Some(o),
        File::Opaque { .. } => None,
    }
}

/// The first non-COMDAT section named `name`.
fn section<'o>(o: &'o Object<'_>, name: &[u8]) -> Option<&'o [u8]> {
    o.sections
        .iter()
        .find(|s| s.name.as_bytes() == name && s.characteristics & 0x1000 == 0)
        .map(turbonop_coff::Section::bytes)
}

/// The member's `LF_PRECOMP`, if its `.debug$T` begins with one.
pub fn use_of(data: &[u8]) -> Option<Use> {
    let o = object(data)?;
    let t = section(&o, b".debug$T")?;
    if u32_at(t, 0)? != CV_SIGNATURE_C13 || u16_at(t, 6)? != LF_PRECOMP {
        return None;
    }
    // length, kind, start index, count, signature, path.
    Some(Use {
        count: u32_at(t, 12)?,
        signature: u32_at(t, 16)?,
        path: string_at(t, 20)?,
    })
}

/// The member's identity as a PCH object, if it has a `.debug$P` ending in
/// `LF_ENDPRECOMP` and an `S_OBJNAME`.
pub fn pch_of(data: &[u8]) -> Option<Pch> {
    let o = object(data)?;
    let p = section(&o, b".debug$P")?;
    if u32_at(p, 0)? != CV_SIGNATURE_C13 {
        return None;
    }
    let (mut at, mut count, mut end) = (4usize, 0u32, None);
    while at + 4 <= p.len() {
        let len = usize::from(u16_at(p, at)?);
        if u16_at(p, at + 2)? == LF_ENDPRECOMP {
            end = Some(u32_at(p, at + 4)?);
            break;
        }
        count += 1;
        at += 2 + len;
    }
    let end_signature = end?;
    let s = section(&o, b".debug$S")?;
    let mut at = 4usize;
    while at + 8 <= s.len() {
        let (kind, len) = (u32_at(s, at)?, u32_at(s, at + 4)? as usize);
        let body = s.get(at + 8..at + 8 + len)?;
        if kind == DEBUG_S_SYMBOLS {
            let mut r = 0usize;
            while r + 4 <= body.len() {
                let rlen = usize::from(u16_at(body, r)?);
                if u16_at(body, r + 2)? == S_OBJNAME {
                    return Some(Pch {
                        signature: u32_at(body, r + 4)?,
                        path: string_at(body, r + 8)?,
                        end_signature,
                        count,
                    });
                }
                r += 2 + rlen;
            }
        }
        at += (8 + len).next_multiple_of(4);
    }
    None
}

/// Whether `path`'s file name is one turbolib gives a PCH member.
pub fn names_turbolib_pch(path: &[u8]) -> bool {
    let file = path
        .rsplit(|&c| c == b'\\' || c == b'/')
        .next()
        .unwrap_or(path);
    file.starts_with(PREFIX)
}

/// Whether link.exe takes the PCH object `p`, linked as `member`, for the
/// path `u` names: its `S_OBJNAME` path or the name it is linked under
/// equals it, ignoring case.
pub fn names(u: &Use, p: &Pch, member: &[u8]) -> bool {
    u.path.eq_ignore_ascii_case(&p.path) || u.path.eq_ignore_ascii_case(member)
}

/// Why `u` and `p` don't pair as link.exe pairs them, if they don't (`p`'s
/// path aside: see [`names`]).
pub fn mismatch(u: &Use, p: &Pch) -> Option<&'static str> {
    if u.signature != p.signature || u.signature != p.end_signature {
        Some("signature")
    } else if u.count != p.count {
        Some("type count")
    } else {
        None
    }
}
