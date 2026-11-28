//! Every reader over a file: what any of them can be asked, with the
//! errors they give.

#![allow(dead_code)]

use turbonop_dex::annotations::{self, AnnotationsDirectory};
use turbonop_dex::debug::DebugInfoItem;
use turbonop_dex::text::{self, Style};
use turbonop_dex::{Dex, MethodHandleKind, insn, names, size};

/// The errors the readers gave.
#[derive(Debug, Default)]
pub struct Errors(pub Vec<String>);

impl Errors {
    fn note<T>(&mut self, what: &str, r: turbonop_core::Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.0.push(format!("{what}: {e}"));
                None
            }
        }
    }
}

/// Every reader over every DEX file of `data`.
pub fn file(data: &[u8], e: &mut Errors) -> usize {
    let Some(all) = e.note("parse_all", Dex::parse_all(data)) else {
        // (The first file, at least, if it's there.)
        if let Ok(d) = Dex::parse(data) {
            dex(&d, e);
        }
        return 0;
    };
    for d in &all {
        dex(d, e);
    }
    all.len()
}

/// Every reader over `d`.
pub fn dex(d: &Dex<'_>, e: &mut Errors) {
    let v = d.verify();
    if !v.checksum_ok {
        e.0.push("checksum".into());
    }
    if !v.signature_ok {
        e.0.push("signature".into());
    }
    e.note("map", d.map().map(<[_]>::len));
    for i in 0..d.string_count() {
        e.note("string", d.string(i));
    }
    for i in 0..d.type_count() {
        e.note("type", d.type_name(i));
    }
    for i in 0..d.proto_count() {
        e.note("proto", d.proto_parameters(i));
        let mut s = Vec::new();
        text::proto_signature(d, i, Style::Unicode, &mut s);
    }
    for i in 0..d.field_count() {
        let mut s = Vec::new();
        text::field_ref(d, i, Style::Dexdump, &mut s);
    }
    for i in 0..d.method_count() {
        let mut s = Vec::new();
        text::method_ref(d, i, Style::Unicode, &mut s);
    }
    for i in 0..d.method_handle_count() {
        if let Some(h) = e.note("method handle", d.method_handle(i)) {
            let _ = MethodHandleKind::from_u16(h.kind).map(MethodHandleKind::name);
        }
    }
    for i in 0..d.call_site_count() {
        if let Some(values) = e.note("call site", d.call_site(i)) {
            for v in &values {
                let mut s = Vec::new();
                text::encoded_value(d, v, Style::Unicode, &mut s);
            }
        }
    }
    let hidden = e.note("hiddenapi", d.hiddenapi()).flatten();
    for i in 0..d.class_def_count() {
        let Some(c) = e.note("class def", d.class_def(i)) else {
            continue;
        };
        let _ = names::access_flags_text(c.access_flags, names::AccessFor::Class);
        e.note("interfaces", d.interfaces(&c));
        if let Some(values) = e.note("static values", d.static_values(&c)) {
            for v in &values {
                let mut s = Vec::new();
                text::encoded_value(d, v, Style::Dexdump, &mut s);
            }
        }
        if c.annotations_off != 0
            && let Some(dir) = e.note(
                "annotations",
                AnnotationsDirectory::parse(d.data(), c.annotations_off),
            )
        {
            let mut sets: Vec<u32> = vec![dir.class_annotations_off];
            sets.extend(dir.fields.iter().chain(&dir.methods).map(|x| x.1));
            for &(_, off) in &dir.parameters {
                if let Some(list) = e.note(
                    "ref list",
                    annotations::annotation_set_ref_list(d.data(), off),
                ) {
                    sets.extend(list);
                }
            }
            for s in sets.into_iter().filter(|&s| s != 0) {
                if let Some(items) = e.note(
                    "annotation set",
                    annotations::annotations_of_set(d.data(), s),
                ) {
                    for a in items {
                        let mut t = Vec::new();
                        text::encoded_value(
                            d,
                            &turbonop_dex::EncodedValue::Annotation(a.annotation),
                            Style::Unicode,
                            &mut t,
                        );
                    }
                }
            }
        }
        let Some(cd) = e.note("class data", d.class_data(&c)).flatten() else {
            continue;
        };
        if let Some(h) = &hidden {
            e.note("hiddenapi flags", h.class_flags(i, &cd));
        }
        for m in cd.methods() {
            let _ = names::access_flags_text(m.access_flags, names::AccessFor::Method);
            if m.code_off == 0 {
                continue;
            }
            let Some(code) = e.note("code", d.code_item(m.code_off)) else {
                continue;
            };
            for t in &code.tries {
                let _ = code.handler(t.handler_off);
            }
            for i in insn::decode(code.insns) {
                let Some(i) = e.note("instruction", i) else {
                    break;
                };
                let _ = text::instruction(d, &i, Style::Dexdump);
                let _ = text::instruction_string(d, &i);
                let _ = (i.index(), i.target(), i.arg_registers());
                if matches!(i.opcode, 0x26 | 0x2B | 0x2C) {
                    let _ = insn::payload_of(code.insns, &i);
                }
                if i.kind != insn::Kind::Op {
                    e.note("payload", insn::payload(code.insns, &i));
                }
            }
            if code.debug_info_off != 0
                && let Some(debug) = e.note(
                    "debug info",
                    DebugInfoItem::parse(d.data(), code.debug_info_off),
                )
            {
                let _ = debug.positions();
                let is_static = m.access_flags & names::acc::STATIC != 0;
                let (_, r) = debug.locals(
                    d,
                    m.method_idx,
                    is_static,
                    code.registers_size,
                    code.ins_size,
                    code.insns_size(),
                );
                e.note("locals", r);
            }
        }
    }
    for s in size::class_sizes(d) {
        e.note("size", s);
    }
}
