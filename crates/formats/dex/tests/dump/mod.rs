//! A file dumped as `dexdump -a -d -f -h` dumps it (its plain layout),
//! from turbonop-dex: the file header, each class's header, annotations,
//! fields (static values included) and methods (code, disassembly,
//! catches, positions, locals), then the method handles and call sites.

#![allow(
    dead_code,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation
)]

use std::io::Write as _;

use turbonop_dex::annotations::{self, AnnotationsDirectory};
use turbonop_dex::debug::DebugInfoItem;
use turbonop_dex::names::{AccessFor, access_flags_text};
use turbonop_dex::text::{self, Style};
use turbonop_dex::value::EncodedValue;
use turbonop_dex::{ClassDef, CodeItem, Dex, EncodedField, EncodedMethod, MethodHandleKind, insn};

const S: Style = Style::Dexdump;

/// A string's bytes, or what printf writes for a null pointer.
fn string(dex: &Dex<'_>, idx: u32) -> Vec<u8> {
    dex.string_bytes(idx)
        .map_or_else(|_| b"(null)".to_vec(), <[u8]>::to_vec)
}

fn descriptor(dex: &Dex<'_>, idx: u32) -> Vec<u8> {
    dex.type_descriptor(idx)
        .map_or_else(|_| b"(null)".to_vec(), <[u8]>::to_vec)
}

/// dexdump's `descriptorToDot`: `Ljava/lang/String;` as
/// `java.lang.String`, `[I` as `int[]`.
fn descriptor_to_dot(d: &[u8]) -> Vec<u8> {
    let mut len = d.len();
    let mut off = 0;
    while len > 1 && d[off] == b'[' {
        off += 1;
        len -= 1;
    }
    let depth = off;
    let mut out: Vec<u8>;
    if len == 1 {
        out = match d[off] {
            b'B' => "byte",
            b'C' => "char",
            b'D' => "double",
            b'F' => "float",
            b'I' => "int",
            b'J' => "long",
            b'S' => "short",
            b'V' => "void",
            b'Z' => "boolean",
            _ => "UNKNOWN",
        }
        .as_bytes()
        .to_vec();
    } else {
        if len >= 2 && d[off] == b'L' && d[off + len - 1] == b';' {
            len -= 2;
            off += 1;
        }
        out = d[off..off + len]
            .iter()
            .map(|&c| if c == b'/' { b'.' } else { c })
            .collect();
    }
    for _ in 0..depth {
        out.extend_from_slice(b"[]");
    }
    out
}

fn file_header(dex: &Dex<'_>, out: &mut Vec<u8>) {
    let h = dex.header();
    out.extend_from_slice(b"DEX file header:\n");
    out.extend_from_slice(b"magic               : '");
    for &c in &h.magic {
        match c {
            b'\n' => out.extend_from_slice(b"\\n"),
            0 => out.extend_from_slice(b"\\0"),
            c if c.is_ascii_graphic() || c == b' ' => out.push(c),
            c => {
                let _ = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.extend_from_slice(b"'\n");
    let _ = writeln!(out, "checksum            : {:08x}", h.checksum);
    let s = h.signature;
    let _ = writeln!(
        out,
        "signature           : {:02x}{:02x}...{:02x}{:02x}",
        s[0], s[1], s[18], s[19]
    );
    if let Some(c) = h.container_size {
        let _ = writeln!(out, "container_size      : {}", c as i32);
    }
    let _ = writeln!(out, "file_size           : {}", h.file_size as i32);
    let _ = writeln!(out, "header_size         : {}", h.header_size as i32);
    if let Some(o) = h.header_offset {
        let _ = writeln!(out, "header_off          : {} (0x{o:06x})", o as i32);
    }
    let _ = writeln!(out, "link_size           : {}", h.link_size as i32);
    let pair = |out: &mut Vec<u8>, name: &str, size: u32, off: u32| {
        let _ = writeln!(
            out,
            "{name}_size{:pad$}: {}",
            "",
            size as i32,
            pad = 15 - name.len()
        );
        let _ = writeln!(
            out,
            "{name}_off{:pad$}: {} (0x{off:06x})",
            "",
            off as i32,
            pad = 16 - name.len()
        );
    };
    let _ = writeln!(
        out,
        "link_off            : {} (0x{:06x})",
        h.link_off as i32, h.link_off
    );
    pair(out, "string_ids", h.string_ids_size, h.string_ids_off);
    pair(out, "type_ids", h.type_ids_size, h.type_ids_off);
    pair(out, "proto_ids", h.proto_ids_size, h.proto_ids_off);
    pair(out, "field_ids", h.field_ids_size, h.field_ids_off);
    pair(out, "method_ids", h.method_ids_size, h.method_ids_off);
    pair(out, "class_defs", h.class_defs_size, h.class_defs_off);
    pair(out, "data", h.data_size, h.data_off);
    out.push(b'\n');
}

fn class_header(dex: &Dex<'_>, i: u32, c: &ClassDef, out: &mut Vec<u8>) {
    let cd = dex.class_data(c).ok().flatten().unwrap_or_default();
    let _ = writeln!(out, "Class #{i} header:");
    let _ = writeln!(out, "class_idx           : {}", c.class_idx & 0xFFFF);
    let _ = writeln!(
        out,
        "access_flags        : {} (0x{:04x})",
        c.access_flags as i32, c.access_flags
    );
    let _ = writeln!(out, "superclass_idx      : {}", c.superclass_idx & 0xFFFF);
    let _ = writeln!(
        out,
        "interfaces_off      : {} (0x{:06x})",
        c.interfaces_off as i32, c.interfaces_off
    );
    let _ = writeln!(out, "source_file_idx     : {}", c.source_file_idx as i32);
    let _ = writeln!(
        out,
        "annotations_off     : {} (0x{:06x})",
        c.annotations_off as i32, c.annotations_off
    );
    let _ = writeln!(
        out,
        "class_data_off      : {} (0x{:06x})",
        c.class_data_off as i32, c.class_data_off
    );
    let _ = writeln!(out, "static_fields_size  : {}", cd.static_fields.len());
    let _ = writeln!(out, "instance_fields_size: {}", cd.instance_fields.len());
    let _ = writeln!(out, "direct_methods_size : {}", cd.direct_methods.len());
    let _ = writeln!(out, "virtual_methods_size: {}", cd.virtual_methods.len());
    out.push(b'\n');
}

fn annotation_set(dex: &Dex<'_>, off: u32, out: &mut Vec<u8>) {
    let set = if off == 0 {
        Vec::new()
    } else {
        annotations::annotation_set(dex.data(), off).unwrap_or_default()
    };
    if set.is_empty() {
        out.extend_from_slice(b"  empty-annotation-set\n");
        return;
    }
    for a in set {
        let Ok(item) = annotations::annotation_item(dex.data(), a) else {
            continue;
        };
        out.extend_from_slice(b"  ");
        out.extend_from_slice(annotations::visibility_name(item.visibility).as_bytes());
        out.push(b' ');
        text::encoded_value(dex, &EncodedValue::Annotation(item.annotation), S, out);
        out.push(b'\n');
    }
}

fn class_annotations(dex: &Dex<'_>, i: u32, c: &ClassDef, out: &mut Vec<u8>) {
    if c.annotations_off == 0 {
        return;
    }
    let Ok(dir) = AnnotationsDirectory::parse(dex.data(), c.annotations_off) else {
        return;
    };
    let _ = writeln!(out, "Class #{i} annotations:");
    if dir.class_annotations_off != 0 {
        out.extend_from_slice(b"Annotations on class\n");
        annotation_set(dex, dir.class_annotations_off, out);
    }
    for &(f, off) in &dir.fields {
        let name = dex
            .field(f)
            .map(|f| string(dex, f.name_idx))
            .unwrap_or_default();
        let _ = write!(out, "Annotations on field #{f} '");
        out.extend_from_slice(&name);
        out.extend_from_slice(b"'\n");
        annotation_set(dex, off, out);
    }
    for &(m, off) in &dir.methods {
        let name = dex
            .method(m)
            .map(|m| string(dex, m.name_idx))
            .unwrap_or_default();
        let _ = write!(out, "Annotations on method #{m} '");
        out.extend_from_slice(&name);
        out.extend_from_slice(b"'\n");
        annotation_set(dex, off, out);
    }
    for &(m, off) in &dir.parameters {
        let name = dex
            .method(m)
            .map(|m| string(dex, m.name_idx))
            .unwrap_or_default();
        let _ = write!(out, "Annotations on method #{m} '");
        out.extend_from_slice(&name);
        out.extend_from_slice(b"' parameters\n");
        if off != 0 {
            for (j, s) in annotations::annotation_set_ref_list(dex.data(), off)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let _ = writeln!(out, "#{j}");
                annotation_set(dex, s, out);
            }
        }
    }
    out.push(b'\n');
}

/// dexdump's names for the flags: as ART's, but values 8 to 14 as
/// `UNSUPPORTED`, and bits above the domains not at all.
fn hiddenapi_line(flags: Option<u32>, out: &mut Vec<u8>) {
    if let Some(f) = flags.filter(|&f| f != 0) {
        let value = match f & 0xF {
            8..=14 => 1,
            v => v,
        };
        let names = turbonop_dex::hiddenapi::flag_names(value | (f & 0x30)).to_uppercase();
        let _ = writeln!(out, "      hiddenapi     : 0x{f:04x} ({names})");
    }
}

fn field(
    dex: &Dex<'_>,
    i: usize,
    f: &EncodedField,
    value: Option<&EncodedValue>,
    hidden: Option<u32>,
    out: &mut Vec<u8>,
) {
    let id = dex.field(f.field_idx).ok();
    let _ = write!(out, "    #{i}              : (in ");
    out.extend_from_slice(
        &id.map(|id| descriptor(dex, u32::from(id.class_idx)))
            .unwrap_or_default(),
    );
    out.extend_from_slice(b")\n      name          : '");
    out.extend_from_slice(&id.map(|id| string(dex, id.name_idx)).unwrap_or_default());
    out.extend_from_slice(b"'\n      type          : '");
    out.extend_from_slice(
        &id.map(|id| descriptor(dex, u32::from(id.type_idx)))
            .unwrap_or_default(),
    );
    let _ = writeln!(
        out,
        "'\n      access        : 0x{:04x} ({})",
        f.access_flags,
        access_flags_text(f.access_flags, AccessFor::Field)
    );
    hiddenapi_line(hidden, out);
    if let Some(v) = value {
        out.extend_from_slice(b"      value         : ");
        text::encoded_value(dex, v, S, out);
        out.push(b'\n');
    }
}

fn code(dex: &Dex<'_>, m: &EncodedMethod, item: &CodeItem<'_>, out: &mut Vec<u8>) {
    let _ = writeln!(out, "      registers     : {}", item.registers_size);
    let _ = writeln!(out, "      ins           : {}", item.ins_size);
    let _ = writeln!(out, "      outs          : {}", item.outs_size);
    let _ = writeln!(
        out,
        "      insns size    : {} 16-bit code units",
        item.insns_size()
    );
    // The disassembly.
    let id = dex.method(m.method_idx).ok();
    let base = item.offset;
    let _ = write!(
        out,
        "{base:06x}:                                        |[{base:06x}] "
    );
    if let Some(id) = id {
        out.extend_from_slice(&descriptor_to_dot(&descriptor(
            dex,
            u32::from(id.class_idx),
        )));
        out.push(b'.');
        out.extend_from_slice(&string(dex, id.name_idx));
        out.push(b':');
        text::proto_signature(dex, u32::from(id.proto_idx), S, out);
    }
    out.push(b'\n');
    for insn in insn::decode(item.insns) {
        let Ok(insn) = insn else {
            break;
        };
        let _ = write!(out, "{:06x}:", base + 0x10 + insn.address * 2);
        for i in 0..8 {
            if i < insn.width {
                if i == 7 {
                    out.extend_from_slice(b" ... ");
                } else {
                    let at = (insn.address + i) as usize * 2;
                    let _ = write!(out, " {:02x}{:02x}", item.insns[at], item.insns[at + 1]);
                }
            } else {
                out.extend_from_slice(b"     ");
            }
        }
        let _ = write!(out, "|{:04x}: ", insn.address);
        out.extend_from_slice(&text::instruction(dex, &insn, S));
        out.push(b'\n');
    }
    // Catches.
    if item.tries.is_empty() {
        out.extend_from_slice(b"      catches       : (none)\n");
    } else {
        let _ = writeln!(out, "      catches       : {}", item.tries.len());
        for t in &item.tries {
            let _ = writeln!(
                out,
                "        0x{:04x} - 0x{:04x}",
                t.start_addr,
                t.start_addr.wrapping_add(u32::from(t.insn_count))
            );
            if let Some(h) = item.handler(t.handler_off) {
                for &(ty, addr) in &h.handlers {
                    out.extend_from_slice(b"          ");
                    out.extend_from_slice(&descriptor(dex, ty));
                    let _ = writeln!(out, " -> 0x{addr:04x}");
                }
                if let Some(addr) = h.catch_all {
                    let _ = writeln!(out, "          <any> -> 0x{addr:04x}");
                }
            }
        }
    }
    // Positions and locals.
    out.extend_from_slice(b"      positions     :\n");
    let debug = (item.debug_info_off != 0)
        .then(|| DebugInfoItem::parse(dex.data(), item.debug_info_off).ok())
        .flatten();
    if let Some(d) = &debug {
        for p in d.positions() {
            let _ = writeln!(out, "        0x{:04x} line={}", p.address, p.line as i32);
        }
    }
    out.extend_from_slice(b"      locals        :\n");
    if let Some(d) = &debug {
        let is_static = m.access_flags & turbonop_dex::names::acc::STATIC != 0;
        let (locals, _) = d.locals(
            dex,
            m.method_idx,
            is_static,
            item.registers_size,
            item.ins_size,
            item.insns_size(),
        );
        for l in locals {
            let _ = write!(
                out,
                "        0x{:04x} - 0x{:04x} reg={} ",
                l.start, l.end, l.reg
            );
            if l.is_this {
                out.extend_from_slice(b"this");
            } else {
                match l.name_idx {
                    Some(n) => out.extend_from_slice(&string(dex, n)),
                    None => out.extend_from_slice(b"(null)"),
                }
            }
            out.push(b' ');
            match l.type_idx {
                Some(t) => out.extend_from_slice(&descriptor(dex, t)),
                None => out.extend_from_slice(b"(null)"),
            }
            if let Some(s) = l.signature_idx {
                out.push(b' ');
                out.extend_from_slice(&string(dex, s));
            }
            out.push(b'\n');
        }
    }
}

fn method(dex: &Dex<'_>, i: usize, m: &EncodedMethod, hidden: Option<u32>, out: &mut Vec<u8>) {
    let id = dex.method(m.method_idx).ok();
    let _ = write!(out, "    #{i}              : (in ");
    out.extend_from_slice(
        &id.map(|id| descriptor(dex, u32::from(id.class_idx)))
            .unwrap_or_default(),
    );
    out.extend_from_slice(b")\n      name          : '");
    out.extend_from_slice(&id.map(|id| string(dex, id.name_idx)).unwrap_or_default());
    out.extend_from_slice(b"'\n      type          : '");
    if let Some(id) = id {
        text::proto_signature(dex, u32::from(id.proto_idx), S, out);
    }
    let _ = writeln!(
        out,
        "'\n      access        : 0x{:04x} ({})",
        m.access_flags,
        access_flags_text(m.access_flags, AccessFor::Method)
    );
    let _ = writeln!(out, "      method_idx    : {}", m.method_idx);
    hiddenapi_line(hidden, out);
    match (m.code_off != 0).then(|| dex.code_item(m.code_off)) {
        None => out.extend_from_slice(b"      code          : (none)\n"),
        Some(Ok(item)) => {
            out.extend_from_slice(b"      code          -\n");
            code(dex, m, &item, out);
        }
        Some(Err(_)) => out.extend_from_slice(b"      code          -\n"),
    }
    out.push(b'\n');
}

fn class(dex: &Dex<'_>, i: u32, c: &ClassDef, out: &mut Vec<u8>) {
    class_header(dex, i, c, out);
    class_annotations(dex, i, c, out);
    let _ = writeln!(out, "Class #{i}            -");
    out.extend_from_slice(b"  Class descriptor  : '");
    out.extend_from_slice(&descriptor(dex, c.class_idx));
    let _ = writeln!(
        out,
        "'\n  Access flags      : 0x{:04x} ({})",
        c.access_flags,
        access_flags_text(c.access_flags, AccessFor::Class)
    );
    if c.superclass_idx & 0xFFFF != 0xFFFF {
        out.extend_from_slice(b"  Superclass        : '");
        out.extend_from_slice(&descriptor(dex, c.superclass_idx & 0xFFFF));
        out.extend_from_slice(b"'\n");
    }
    out.extend_from_slice(b"  Interfaces        -\n");
    for (j, t) in dex.interfaces(c).unwrap_or_default().iter().enumerate() {
        let _ = write!(out, "    #{j}              : '");
        out.extend_from_slice(&descriptor(dex, u32::from(t)));
        out.extend_from_slice(b"'\n");
    }
    let cd = dex.class_data(c).ok().flatten().unwrap_or_default();
    let values = dex.static_values(c).unwrap_or_default();
    let hidden = dex
        .hiddenapi()
        .ok()
        .flatten()
        .and_then(|h| h.class_flags(i, &cd).ok().flatten());
    let flag = |k: usize| hidden.as_ref().and_then(|h| h.get(k).copied());
    let mut k = 0;
    out.extend_from_slice(b"  Static fields     -\n");
    for (j, f) in cd.static_fields.iter().enumerate() {
        field(dex, j, f, values.get(j), flag(k), out);
        k += 1;
    }
    out.extend_from_slice(b"  Instance fields   -\n");
    for (j, f) in cd.instance_fields.iter().enumerate() {
        field(dex, j, f, None, flag(k), out);
        k += 1;
    }
    out.extend_from_slice(b"  Direct methods    -\n");
    for (j, m) in cd.direct_methods.iter().enumerate() {
        method(dex, j, m, flag(k), out);
        k += 1;
    }
    out.extend_from_slice(b"  Virtual methods   -\n");
    for (j, m) in cd.virtual_methods.iter().enumerate() {
        method(dex, j, m, flag(k), out);
        k += 1;
    }
    let _ = write!(out, "  source_file_idx   : {} (", c.source_file_idx as i32);
    if c.source_file_idx == turbonop_dex::NO_INDEX {
        out.extend_from_slice(b"unknown");
    } else {
        out.extend_from_slice(&string(dex, c.source_file_idx));
    }
    out.extend_from_slice(b")\n\n");
}

fn method_handle(dex: &Dex<'_>, i: u32, out: &mut Vec<u8>) {
    let Ok(mh) = dex.method_handle(i) else {
        return;
    };
    let (ty, class, member, mut member_type);
    match MethodHandleKind::from_u16(mh.kind) {
        Some(kind) => {
            ty = kind.name().as_bytes().to_vec();
            let idx = u32::from(mh.field_or_method_idx);
            let is_instance = !matches!(
                kind,
                MethodHandleKind::StaticPut
                    | MethodHandleKind::StaticGet
                    | MethodHandleKind::InvokeStatic
            );
            if kind.is_field() {
                let f = dex.field(idx).unwrap();
                class = descriptor(dex, u32::from(f.class_idx));
                member = string(dex, f.name_idx);
                member_type = descriptor(dex, u32::from(f.type_idx));
            } else {
                let m = dex.method(idx).unwrap();
                class = descriptor(dex, u32::from(m.class_idx));
                member = string(dex, m.name_idx);
                member_type = Vec::new();
                text::proto_signature(dex, u32::from(m.proto_idx), S, &mut member_type);
            }
            if is_instance {
                let mut t = b"(".to_vec();
                t.extend_from_slice(&class);
                t.extend_from_slice(member_type.get(1..).unwrap_or_default());
                member_type = t;
            }
        }
        None => {
            ty = b"?".to_vec();
            class = b"?".to_vec();
            member = b"?".to_vec();
            member_type = b"?".to_vec();
        }
    }
    let _ = writeln!(out, "Method handle #{i}:");
    out.extend_from_slice(b"  type        : ");
    out.extend_from_slice(&ty);
    out.extend_from_slice(b"\n  target      : ");
    out.extend_from_slice(&class);
    out.push(b' ');
    out.extend_from_slice(&member);
    out.extend_from_slice(b"\n  target_type : ");
    out.extend_from_slice(&member_type);
    out.push(b'\n');
}

fn call_site(dex: &Dex<'_>, i: u32, out: &mut Vec<u8>) {
    let Ok(values) = dex.call_site(i) else {
        return;
    };
    if values.len() < 3 {
        return;
    }
    let idx = |v: &EncodedValue| match v {
        EncodedValue::MethodHandle(x)
        | EncodedValue::String(x)
        | EncodedValue::MethodType(x)
        | EncodedValue::Type(x) => *x,
        _ => 0,
    };
    let _ = writeln!(
        out,
        "Call site #{i}: // offset {}",
        dex.call_site_offset(i).unwrap()
    );
    let _ = writeln!(
        out,
        "  link_argument[0] : {} (MethodHandle)",
        idx(&values[0])
    );
    out.extend_from_slice(b"  link_argument[1] : ");
    out.extend_from_slice(&string(dex, idx(&values[1])));
    out.extend_from_slice(b" (String)\n  link_argument[2] : ");
    text::proto_signature(dex, idx(&values[2]), S, out);
    out.extend_from_slice(b" (MethodType)\n");
    for (j, v) in values.iter().enumerate().skip(3) {
        let (value, ty): (Vec<u8>, &str) = match v {
            EncodedValue::Byte(x) => (format!("{}", i32::from(*x) as u32).into_bytes(), "byte"),
            EncodedValue::Short(x) => (format!("{x}").into_bytes(), "short"),
            EncodedValue::Char(x) => (format!("{x}").into_bytes(), "char"),
            EncodedValue::Int(x) => (format!("{x}").into_bytes(), "int"),
            EncodedValue::Long(x) => (format!("{x}").into_bytes(), "long"),
            EncodedValue::Float(x) => (text::format_g(f64::from(*x)).into_bytes(), "float"),
            EncodedValue::Double(x) => (text::format_g(*x).into_bytes(), "double"),
            EncodedValue::MethodType(p) => {
                let mut s = Vec::new();
                text::proto_signature(dex, *p, S, &mut s);
                (s, "MethodType")
            }
            EncodedValue::MethodHandle(h) => (format!("{h}").into_bytes(), "MethodHandle"),
            EncodedValue::String(s) => {
                let mut e = Vec::new();
                text::escape(&string(dex, *s), &mut e);
                (e, "String")
            }
            EncodedValue::Type(t) => (descriptor(dex, *t), "Class"),
            EncodedValue::Null => (b"null".to_vec(), "Null"),
            EncodedValue::Boolean(b) => (
                if *b {
                    b"true".to_vec()
                } else {
                    b"false".to_vec()
                },
                "boolean",
            ),
            _ => (b"?".to_vec(), "?"),
        };
        let _ = write!(out, "  link_argument[{j}] : ");
        out.extend_from_slice(&value);
        let _ = writeln!(out, " ({ty})");
    }
}

/// `data` (named `name`) as `dexdump -a -d -f -h` dumps it.
pub fn dump(name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = writeln!(out, "Processing '{name}'...");
    let all = Dex::parse_all(data).unwrap();
    let n = all.len();
    for (k, dex) in all.iter().enumerate() {
        let _ = write!(out, "Opened '{name}");
        if n > 1 {
            if k == 0 {
                out.extend_from_slice(b":classes.dex");
            } else {
                let _ = write!(out, ":classes{}.dex", k + 1);
            }
        }
        let _ = writeln!(out, "', DEX version '{}'", dex.header().version_str());
        file_header(dex, &mut out);
        for i in 0..dex.class_def_count() {
            let c = dex.class_def(i).unwrap();
            class(dex, i, &c, &mut out);
        }
        for i in 0..dex.method_handle_count() {
            method_handle(dex, i, &mut out);
        }
        for i in 0..dex.call_site_count() {
            call_site(dex, i, &mut out);
        }
    }
    out
}

/// Where `ours` first differs from `theirs`: the line, both versions, and
/// some context.
pub fn first_difference(ours: &[u8], theirs: &[u8]) -> Option<String> {
    let a: Vec<&[u8]> = ours.split(|&b| b == b'\n').collect();
    let b: Vec<&[u8]> = theirs.split(|&b| b == b'\n').collect();
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x != y {
            let show = |l: Option<&&[u8]>| {
                l.map_or("<end>".to_string(), |l| {
                    String::from_utf8_lossy(l).into_owned()
                })
            };
            let context: Vec<String> = b[i.saturating_sub(5)..i.min(b.len())]
                .iter()
                .map(|l| String::from_utf8_lossy(l).into_owned())
                .collect();
            return Some(format!(
                "line {}:\n ours:   {}\n theirs: {}\n after:\n{}",
                i + 1,
                show(x),
                show(y),
                context.join("\n")
            ));
        }
    }
    None
}
