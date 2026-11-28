//! Instructions, references, and values rendered as `dexdump` writes
//! them: `invoke-virtual {v0, v4}, Ljava/lang/StringBuilder;.append:(Ljava/lang/String;)Ljava/lang/StringBuilder; // method@0021`.
//!
//! Text is built as bytes: dexdump writes strings' MUTF-8 bytes as they
//! are ([`Style::Dexdump`]), which needn't be UTF-8 (a NUL is `C0 80`, a
//! supplementary character a pair of surrogates); [`Style::Unicode`]
//! decodes them instead, for text that is UTF-8.

use std::io::Write as _;

use crate::insn::{Format, IndexType, Instruction, Kind};
use crate::value::EncodedValue;
use crate::{Dex, mutf8};

/// How strings are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// As dexdump writes them: their MUTF-8 bytes.
    Dexdump,
    /// Decoded (unpaired surrogates as U+FFFD): UTF-8 text.
    Unicode,
}

/// C's `printf("%g", v)`: six significant digits, the shorter of fixed
/// and exponent notation, without trailing zeros; `inf`, `nan`.
pub fn format_g(v: f64) -> String {
    const P: i32 = 6;
    if v.is_nan() {
        // (Without a sign, as the SDK's dexdump.exe writes it; glibc's
        // printf writes a negative NaN `-nan`.)
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    // The exponent after rounding to P digits.
    let e = format!("{:.*e}", (P - 1) as usize, v);
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if (-4..P).contains(&x) {
        #[allow(clippy::cast_sign_loss)]
        strip(&format!("{:.*}", (P - 1 - x) as usize, v))
    } else {
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), x.unsigned_abs())
    }
}

/// `bytes` escaped as dexdump escapes strings: `\\`, `\"`, `\b`, `\t`,
/// `\n`, `\f`, `\r`, the other control characters and DEL as octal
/// (`\001`), every other byte as it is.
pub fn escape(bytes: &[u8], out: &mut Vec<u8>) {
    for &c in bytes {
        match c {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            0x08 => out.extend_from_slice(b"\\b"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            0x0C => out.extend_from_slice(b"\\f"),
            b'\r' => out.extend_from_slice(b"\\r"),
            c if c < 0x20 || c == 0x7F => {
                let _ = write!(out, "\\{c:03o}");
            }
            c => out.push(c),
        }
    }
}

/// A string's bytes in `style`.
fn styled(bytes: &[u8], style: Style, out: &mut Vec<u8>) {
    match style {
        Style::Dexdump => out.extend_from_slice(bytes),
        Style::Unicode => out.extend_from_slice(mutf8::decode_lossy(bytes).as_bytes()),
    }
}

/// String `idx` in `style` (unescaped); `false` if it isn't in the file.
pub fn string(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) -> bool {
    match dex.string_bytes(idx) {
        Ok(b) => {
            styled(b, style, out);
            true
        }
        Err(_) => false,
    }
}

/// Type `idx`'s descriptor; `false` if it isn't in the file.
pub fn descriptor(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) -> bool {
    match dex.type_descriptor(idx) {
        Ok(b) => {
            styled(b, style, out);
            true
        }
        Err(_) => false,
    }
}

fn descriptor_or(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) {
    if !descriptor(dex, idx, style, out) {
        out.extend_from_slice(b"<type?>");
    }
}

/// Prototype `idx` as a method descriptor: `(ILjava/lang/String;)J`;
/// `false` if it isn't in the file (then what of it there is).
pub fn proto_signature(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) -> bool {
    let Ok(p) = dex.proto(idx) else {
        return false;
    };
    out.push(b'(');
    let mut ok = true;
    match dex.type_list(p.parameters_off) {
        Ok(params) => {
            for t in params.iter() {
                ok &= descriptor(dex, u32::from(t), style, out);
            }
        }
        Err(_) => ok = false,
    }
    out.push(b')');
    ok & descriptor(dex, p.return_type_idx, style, out)
}

/// Method reference `idx` as dexdump writes it: `Lcls;.name:(I)V`.
pub fn method_ref(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) -> bool {
    let Ok(m) = dex.method(idx) else {
        return false;
    };
    descriptor_or(dex, u32::from(m.class_idx), style, out);
    out.push(b'.');
    if !string(dex, m.name_idx, style, out) {
        out.extend_from_slice(b"<name?>");
    }
    out.push(b':');
    proto_signature(dex, u32::from(m.proto_idx), style, out);
    true
}

/// Field reference `idx` as dexdump writes it: `Lcls;.name:I`.
pub fn field_ref(dex: &Dex<'_>, idx: u32, style: Style, out: &mut Vec<u8>) -> bool {
    let Ok(f) = dex.field(idx) else {
        return false;
    };
    descriptor_or(dex, u32::from(f.class_idx), style, out);
    out.push(b'.');
    if !string(dex, f.name_idx, style, out) {
        out.extend_from_slice(b"<name?>");
    }
    out.push(b':');
    descriptor_or(dex, u32::from(f.type_idx), style, out);
    true
}

/// An instruction's index, resolved, as dexdump writes it: `"text" //
/// string@0004`, `Ljava/lang/String; // type@0013`, `method_handle@0003`.
pub fn index_text(dex: &Dex<'_>, insn: &Instruction, style: Style, out: &mut Vec<u8>) {
    let info = insn.info();
    let (index, width) = match info.format {
        Format::F31c => (insn.b32(), 8),
        Format::F22c | Format::F22cs => (insn.c, 4),
        _ => (insn.b32(), 4),
    };
    match info.index {
        IndexType::None => out.extend_from_slice(b"<no-index>"),
        IndexType::Type => {
            if !descriptor(dex, index, style, out) {
                out.extend_from_slice(b"<type?>");
            }
            let _ = write!(out, " // type@{index:0width$x}");
        }
        IndexType::String => {
            match dex.string_bytes(index) {
                Ok(b) => {
                    out.push(b'"');
                    let mut s = Vec::new();
                    styled(b, style, &mut s);
                    escape(&s, out);
                    out.push(b'"');
                }
                Err(_) => out.extend_from_slice(b"<string?>"),
            }
            let _ = write!(out, " // string@{index:0width$x}");
        }
        IndexType::Method => {
            if !method_ref(dex, index, style, out) {
                out.extend_from_slice(b"<method?>");
            }
            let _ = write!(out, " // method@{index:0width$x}");
        }
        IndexType::Field => {
            if !field_ref(dex, index, style, out) {
                out.extend_from_slice(b"<field?>");
            }
            let _ = write!(out, " // field@{index:0width$x}");
        }
        IndexType::MethodAndProto => {
            if !method_ref(dex, index, style, out) {
                out.extend_from_slice(b"<method?>");
            }
            out.extend_from_slice(b", ");
            let mut p = Vec::new();
            if proto_signature(dex, insn.h, style, &mut p) {
                out.extend_from_slice(&p);
            } else {
                out.extend_from_slice(b"<proto?>");
            }
            let h = insn.h;
            let _ = write!(out, " // method@{index:0width$x}, proto@{h:0width$x}");
        }
        IndexType::CallSite => {
            let _ = write!(out, "call_site@{index:0width$x}");
        }
        IndexType::MethodHandle => {
            let _ = write!(out, "method_handle@{index:0width$x}");
        }
        IndexType::Proto => {
            let mut p = Vec::new();
            if proto_signature(dex, index, style, &mut p) {
                out.extend_from_slice(&p);
            } else {
                out.extend_from_slice(b"<?>");
            }
            let _ = write!(out, " // proto@{index:0width$x}");
        }
        IndexType::VtableOffset => {
            let _ = write!(out, "[{index:0width$x}] // vtable #{index:0width$x}");
        }
        IndexType::FieldOffset => {
            let _ = write!(out, "[obj+{index:0width$x}]");
        }
        IndexType::InlineMethod => {
            let _ = write!(out, "[{index:0width$x}] // inline #{index:0width$x}");
        }
        IndexType::VerificationError => {
            let _ = write!(out, "<unknown-index> // ref@{index:0width$x}");
        }
    }
}

/// A branch's target and offset: `0018 // +000d`.
fn branch(out: &mut Vec<u8>, address: u32, off: i32) {
    let target = address.wrapping_add_signed(off);
    let sign = if off < 0 { '-' } else { '+' };
    let _ = write!(out, "{target:04x} // {sign}{:04x}", off.unsigned_abs());
}

/// An instruction as dexdump writes it, after the address and code units:
/// its name and operands, indices resolved (`if-nez v1, 0008 // +0004`;
/// a payload, `packed-switch-data (8 units)`; padding, `nop // spacer`).
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn instruction(dex: &Dex<'_>, insn: &Instruction, style: Style) -> Vec<u8> {
    let mut out = Vec::new();
    let info = insn.info();
    match insn.kind {
        Kind::PackedSwitchPayload => {
            let _ = write!(out, "packed-switch-data ({} units)", insn.width);
            return out;
        }
        Kind::SparseSwitchPayload => {
            let _ = write!(out, "sparse-switch-data ({} units)", insn.width);
            return out;
        }
        Kind::FillArrayDataPayload => {
            let _ = write!(out, "array-data ({} units)", insn.width);
            return out;
        }
        Kind::Op if insn.opcode == 0 => {
            out.extend_from_slice(b"nop // spacer");
            return out;
        }
        Kind::Op => out.extend_from_slice(info.name.as_bytes()),
    }
    let mut index = Vec::new();
    if info.index != IndexType::None {
        index_text(dex, insn, style, &mut index);
    }
    let (a, b, c) = (insn.a, insn.b32(), insn.c);
    match info.format {
        Format::F10x => {}
        Format::F12x | Format::F22x | Format::F32x => {
            let _ = write!(out, " v{a}, v{b}");
        }
        Format::F11n => {
            let _ = write!(out, " v{a}, #int {} // #{:x}", b as i32, b as u8);
        }
        Format::F11x => {
            let _ = write!(out, " v{a}");
        }
        Format::F10t | Format::F20t => {
            out.push(b' ');
            branch(&mut out, insn.address, a as i32);
        }
        Format::F21t => {
            let _ = write!(out, " v{a}, ");
            branch(&mut out, insn.address, b as i32);
        }
        Format::F21s => {
            let _ = write!(out, " v{a}, #int {} // #{:x}", b as i32, b as u16);
        }
        Format::F21h => {
            if insn.opcode == 0x15 {
                let _ = write!(out, " v{a}, #int {} // #{:x}", (b << 16) as i32, b as u16);
            } else {
                let _ = write!(
                    out,
                    " v{a}, #long {} // #{:x}",
                    (u64::from(b) << 48) as i64,
                    b as u16
                );
            }
        }
        Format::F21c | Format::F31c | Format::F20bc => {
            let _ = write!(out, " v{a}, ");
            out.extend_from_slice(&index);
        }
        Format::F23x => {
            let _ = write!(out, " v{a}, v{b}, v{c}");
        }
        Format::F22b => {
            let _ = write!(out, " v{a}, v{b}, #int {} // #{:02x}", c as i32, c as u8);
        }
        Format::F22t => {
            let _ = write!(out, " v{a}, v{b}, ");
            branch(&mut out, insn.address, c as i32);
        }
        Format::F22s => {
            let _ = write!(out, " v{a}, v{b}, #int {} // #{:04x}", c as i32, c as u16);
        }
        Format::F22c | Format::F22cs => {
            let _ = write!(out, " v{a}, v{b}, ");
            out.extend_from_slice(&index);
        }
        Format::F30t => {
            let _ = write!(out, " #{a:08x}");
        }
        Format::F31i => {
            let f = f32::from_bits(b);
            let _ = write!(out, " v{a}, #float {} // #{b:08x}", format_g(f64::from(f)));
        }
        Format::F31t => {
            let _ = write!(
                out,
                " v{a}, {:08x} // +{b:08x}",
                insn.address.wrapping_add(b)
            );
        }
        Format::F35c | Format::F35ms | Format::F35mi | Format::F45cc => {
            out.extend_from_slice(b" {");
            for (i, r) in insn.args.iter().take(a as usize).enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                let _ = write!(out, "v{r}");
            }
            out.extend_from_slice(b"}, ");
            out.extend_from_slice(&index);
        }
        Format::F3rc | Format::F3rms | Format::F3rmi | Format::F4rcc => {
            out.extend_from_slice(b" {");
            for i in 0..a {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                let _ = write!(out, "v{}", c.wrapping_add(i));
            }
            out.extend_from_slice(b"}, ");
            out.extend_from_slice(&index);
        }
        Format::F51l => {
            let d = f64::from_bits(insn.b);
            let _ = write!(out, " v{a}, #double {} // #{:016x}", format_g(d), insn.b);
        }
    }
    out
}

/// [`instruction`] as UTF-8 text ([`Style::Unicode`]).
pub fn instruction_string(dex: &Dex<'_>, insn: &Instruction) -> String {
    String::from_utf8_lossy(&instruction(dex, insn, Style::Unicode)).into_owned()
}

/// A value as dexdump writes annotation elements and static values:
/// numbers in decimal (`%g` for floating point), strings quoted and
/// escaped, types as descriptors, fields, enum constants and methods by
/// name, arrays as `{ a b }`, annotations as `Ltype; name=value ...`.
pub fn encoded_value(dex: &Dex<'_>, v: &EncodedValue, style: Style, out: &mut Vec<u8>) {
    match v {
        EncodedValue::Byte(x) => {
            let _ = write!(out, "{x}");
        }
        EncodedValue::Short(x) => {
            let _ = write!(out, "{x}");
        }
        EncodedValue::Char(x) => {
            let _ = write!(out, "{x}");
        }
        EncodedValue::Int(x) => {
            let _ = write!(out, "{x}");
        }
        EncodedValue::Long(x) => {
            let _ = write!(out, "{x}");
        }
        EncodedValue::Float(x) => out.extend_from_slice(format_g(f64::from(*x)).as_bytes()),
        EncodedValue::Double(x) => out.extend_from_slice(format_g(*x).as_bytes()),
        EncodedValue::String(i) => {
            out.push(b'"');
            let mut s = Vec::new();
            if !string(dex, *i, style, &mut s) {
                s.extend_from_slice(b"<string?>");
            }
            escape(&s, out);
            out.push(b'"');
        }
        EncodedValue::Type(i) => descriptor_or(dex, *i, style, out),
        EncodedValue::Field(i) | EncodedValue::Enum(i) => match dex.field(*i) {
            Ok(f) if string(dex, f.name_idx, style, out) => {}
            _ => out.extend_from_slice(b"<field?>"),
        },
        EncodedValue::Method(i) => match dex.method(*i) {
            Ok(m) if string(dex, m.name_idx, style, out) => {}
            _ => out.extend_from_slice(b"<method?>"),
        },
        EncodedValue::Array(items) => {
            out.push(b'{');
            for x in items {
                out.push(b' ');
                encoded_value(dex, x, style, out);
            }
            out.extend_from_slice(b" }");
        }
        EncodedValue::Annotation(a) => {
            descriptor_or(dex, a.type_idx, style, out);
            for (name, x) in &a.elements {
                out.push(b' ');
                if !string(dex, *name, style, out) {
                    out.extend_from_slice(b"<name?>");
                }
                out.push(b'=');
                encoded_value(dex, x, style, out);
            }
        }
        EncodedValue::Null => out.extend_from_slice(b"null"),
        EncodedValue::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        EncodedValue::MethodType(p) => {
            if !proto_signature(dex, *p, style, out) {
                out.extend_from_slice(b"<proto?>");
            }
        }
        EncodedValue::MethodHandle(h) => {
            let _ = write!(out, "method_handle@{h}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printf_g() {
        for (v, s) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (123_456.789, "123457"),
            (1_234_567.0, "1.23457e+06"),
            (1e10, "1e+10"),
            (-2.75e100, "-2.75e+100"),
            (1e-300, "1e-300"),
            (0.0001, "0.0001"),
            (0.000_012_345, "1.2345e-05"),
            (2.414_36e-314, "2.41436e-314"),
            (100_000.0, "100000"),
            (999_999.5, "1e+06"),
            (f64::NAN, "nan"),
            (f64::NEG_INFINITY, "-inf"),
            (std::f64::consts::PI, "3.14159"),
        ] {
            assert_eq!(format_g(v), s, "{v}");
        }
        assert_eq!(
            format_g(f64::from(f32::from_bits(0x1234_5678))),
            "5.69046e-28"
        );
    }

    #[test]
    fn escapes() {
        let mut out = Vec::new();
        escape(b"a\"b\\c\n\r\t\x08\x0c\x01\x07\x0b\x7f'\xc0\x80", &mut out);
        assert_eq!(
            out,
            b"a\\\"b\\\\c\\n\\r\\t\\b\\f\\001\\007\\013\\177'\xc0\x80"
        );
    }
}
