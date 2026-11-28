//! A small DEX writer, for what no compiler writes: every opcode and
//! format, hidden-API flags, every debug opcode and encoded value kind,
//! containers of several DEX files. What it writes passes ART's DEX file
//! verifier (dexdump runs it on every file it reads): ids sorted and
//! unique, items in sections of their own kind, aligned, and the map,
//! checksum, and signature right.

#![allow(dead_code, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::collections::BTreeMap;

use turbonop_dex::header::adler32;
use turbonop_dex::mutf8;
use turbonop_dex::reader::{encode_sleb128, encode_uleb128};
use turbonop_dex::value::{EncodedAnnotation, EncodedValue};

/// A method's code.
#[derive(Clone, Debug, Default)]
pub struct Code {
    pub registers: u16,
    pub ins: u16,
    pub outs: u16,
    pub insns: Vec<u16>,
    /// Each try block: start, count, and which handler (an index in
    /// `handlers`).
    pub tries: Vec<(u32, u16, usize)>,
    /// Each handler: typed catches (type index, address), and catch-all.
    pub handlers: Vec<(Vec<(u32, u32)>, Option<u32>)>,
    /// The debug information item's bytes, as they are.
    pub debug: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Method {
    pub method_idx: u32,
    pub access: u32,
    pub code: Option<Code>,
}

/// An annotation and its visibility.
pub type Annotation = (u8, EncodedAnnotation);

#[derive(Clone, Debug, Default)]
pub struct Annotations {
    pub class: Vec<Annotation>,
    /// Field index, annotations.
    pub fields: Vec<(u32, Vec<Annotation>)>,
    /// Method index, annotations.
    pub methods: Vec<(u32, Vec<Annotation>)>,
    /// Method index, each parameter's annotations (`None`: no set).
    pub parameters: Vec<(u32, Vec<Option<Vec<Annotation>>>)>,
}

#[derive(Clone, Debug)]
pub struct Class {
    pub class_idx: u32,
    pub access: u32,
    pub superclass: Option<u32>,
    pub interfaces: Vec<u16>,
    pub source_file: Option<u32>,
    /// Field index, access flags.
    pub static_fields: Vec<(u32, u32)>,
    pub instance_fields: Vec<(u32, u32)>,
    pub direct_methods: Vec<Method>,
    pub virtual_methods: Vec<Method>,
    pub static_values: Vec<EncodedValue>,
    pub annotations: Option<Annotations>,
    /// Hidden-API flags, one for each field and method.
    pub hiddenapi: Option<Vec<u32>>,
}

/// The ids, declared first (any order), then sorted as the format wants
/// them: [`Ids::freeze`] and the lookups after it give their indices.
#[derive(Clone, Debug, Default)]
pub struct Ids {
    strings: Vec<Vec<u16>>,
    types: Vec<String>,
    protos: Vec<(String, Vec<String>)>,
    fields: Vec<(String, String, String)>,
    methods: Vec<(String, String, String, Vec<String>)>,
    frozen: bool,
}

fn shorty_char(d: &str) -> char {
    match d.as_bytes()[0] {
        b'[' | b'L' => 'L',
        c => c as char,
    }
}

impl Ids {
    pub fn string(&mut self, s: &str) {
        let u: Vec<u16> = s.encode_utf16().collect();
        if !self.strings.contains(&u) {
            self.strings.push(u);
        }
    }

    /// A string of UTF-16 units (unpaired surrogates and all).
    pub fn string_utf16(&mut self, u: &[u16]) {
        if !self.strings.iter().any(|s| s == u) {
            self.strings.push(u.to_vec());
        }
    }

    pub fn ty(&mut self, d: &str) {
        self.string(d);
        if !self.types.iter().any(|t| t == d) {
            self.types.push(d.to_string());
        }
    }

    pub fn proto(&mut self, ret: &str, params: &[&str]) {
        self.ty(ret);
        for p in params {
            self.ty(p);
        }
        let shorty: String = std::iter::once(ret)
            .chain(params.iter().copied())
            .map(shorty_char)
            .collect();
        self.string(&shorty);
        let p = (ret.to_string(), params.iter().map(|s| (*s).to_string()).collect());
        if !self.protos.contains(&p) {
            self.protos.push(p);
        }
    }

    pub fn field(&mut self, class: &str, name: &str, ty: &str) {
        self.ty(class);
        self.ty(ty);
        self.string(name);
        let f = (class.to_string(), name.to_string(), ty.to_string());
        if !self.fields.contains(&f) {
            self.fields.push(f);
        }
    }

    pub fn method(&mut self, class: &str, name: &str, ret: &str, params: &[&str]) {
        self.ty(class);
        self.string(name);
        self.proto(ret, params);
        let m = (
            class.to_string(),
            name.to_string(),
            ret.to_string(),
            params.iter().map(|s| (*s).to_string()).collect(),
        );
        if !self.methods.contains(&m) {
            self.methods.push(m);
        }
    }

    /// Sorts the ids as the format wants them.
    pub fn freeze(&mut self) {
        // Strings by their UTF-16 code points (as MUTF-8's own order is).
        self.strings.sort();
        let strings = self.strings.clone();
        let sidx = |s: &str| {
            let u: Vec<u16> = s.encode_utf16().collect();
            strings.binary_search(&u).unwrap()
        };
        self.types.sort_by_key(|t| sidx(t));
        let types = self.types.clone();
        let tidx = |t: &str| types.iter().position(|x| x == t).unwrap();
        self.protos.sort_by(|a, b| {
            (tidx(&a.0), a.1.iter().map(|p| tidx(p)).collect::<Vec<_>>())
                .cmp(&(tidx(&b.0), b.1.iter().map(|p| tidx(p)).collect::<Vec<_>>()))
        });
        self.fields
            .sort_by_key(|f| (tidx(&f.0), sidx(&f.1), tidx(&f.2)));
        let protos = self.protos.clone();
        let pidx = |r: &str, p: &[String]| {
            protos
                .iter()
                .position(|x| x.0 == r && x.1 == p)
                .unwrap()
        };
        self.methods
            .sort_by_key(|m| (tidx(&m.0), sidx(&m.1), pidx(&m.2, &m.3)));
        self.frozen = true;
    }

    pub fn s(&self, s: &str) -> u32 {
        assert!(self.frozen);
        let u: Vec<u16> = s.encode_utf16().collect();
        self.strings.binary_search(&u).unwrap() as u32
    }

    pub fn s16(&self, u: &[u16]) -> u32 {
        self.strings.binary_search(&u.to_vec()).unwrap() as u32
    }

    pub fn t(&self, d: &str) -> u32 {
        assert!(self.frozen);
        self.types.iter().position(|x| x == d).unwrap() as u32
    }

    pub fn p(&self, ret: &str, params: &[&str]) -> u32 {
        self.protos
            .iter()
            .position(|x| x.0 == ret && x.1.iter().map(String::as_str).eq(params.iter().copied()))
            .unwrap() as u32
    }

    pub fn f(&self, class: &str, name: &str) -> u32 {
        self.fields
            .iter()
            .position(|x| x.0 == class && x.1 == name)
            .unwrap() as u32
    }

    pub fn m(&self, class: &str, name: &str) -> u32 {
        self.methods
            .iter()
            .position(|x| x.0 == class && x.1 == name)
            .unwrap() as u32
    }
}

/// What goes in a DEX file besides the ids.
#[derive(Clone, Debug, Default)]
pub struct Contents {
    pub version: u32,
    pub classes: Vec<Class>,
    /// Kind, field or method index.
    pub method_handles: Vec<(u16, u16)>,
    pub call_sites: Vec<Vec<EncodedValue>>,
}

/// The smallest encoding of `v` (as d8 writes them).
pub fn encode_value(v: &EncodedValue, out: &mut Vec<u8>) {
    fn signed(ty: u8, v: i64, out: &mut Vec<u8>) {
        let mut n = 1;
        while n < 8 {
            let shift = 64 - 8 * n;
            if (v << shift) >> shift == v {
                break;
            }
            n += 1;
        }
        out.push(ty | ((n as u8 - 1) << 5));
        out.extend_from_slice(&v.to_le_bytes()[..n]);
    }
    fn unsigned(ty: u8, v: u64, out: &mut Vec<u8>) {
        let mut n = 1;
        while n < 8 && v >> (8 * n) != 0 {
            n += 1;
        }
        out.push(ty | ((n as u8 - 1) << 5));
        out.extend_from_slice(&v.to_le_bytes()[..n]);
    }
    /// Floating point: the high-order bytes, trailing zero bytes dropped.
    fn right(ty: u8, bytes: &[u8], out: &mut Vec<u8>) {
        let mut start = 0;
        while start + 1 < bytes.len() && bytes[start] == 0 {
            start += 1;
        }
        let kept = &bytes[start..];
        out.push(ty | ((kept.len() as u8 - 1) << 5));
        out.extend_from_slice(kept);
    }
    use turbonop_dex::value::kind;
    match v {
        EncodedValue::Byte(x) => {
            out.push(kind::BYTE);
            out.push(*x as u8);
        }
        EncodedValue::Short(x) => signed(kind::SHORT, i64::from(*x), out),
        EncodedValue::Char(x) => unsigned(kind::CHAR, u64::from(*x), out),
        EncodedValue::Int(x) => signed(kind::INT, i64::from(*x), out),
        EncodedValue::Long(x) => signed(kind::LONG, *x, out),
        EncodedValue::Float(x) => right(kind::FLOAT, &x.to_bits().to_le_bytes(), out),
        EncodedValue::Double(x) => right(kind::DOUBLE, &x.to_bits().to_le_bytes(), out),
        EncodedValue::MethodType(i) => unsigned(kind::METHOD_TYPE, u64::from(*i), out),
        EncodedValue::MethodHandle(i) => unsigned(kind::METHOD_HANDLE, u64::from(*i), out),
        EncodedValue::String(i) => unsigned(kind::STRING, u64::from(*i), out),
        EncodedValue::Type(i) => unsigned(kind::TYPE, u64::from(*i), out),
        EncodedValue::Field(i) => unsigned(kind::FIELD, u64::from(*i), out),
        EncodedValue::Method(i) => unsigned(kind::METHOD, u64::from(*i), out),
        EncodedValue::Enum(i) => unsigned(kind::ENUM, u64::from(*i), out),
        EncodedValue::Array(items) => {
            out.push(kind::ARRAY);
            encode_array(items, out);
        }
        EncodedValue::Annotation(a) => {
            out.push(kind::ANNOTATION);
            encode_annotation(a, out);
        }
        EncodedValue::Null => out.push(kind::NULL),
        EncodedValue::Boolean(b) => out.push(kind::BOOLEAN | (u8::from(*b) << 5)),
    }
}

pub fn encode_array(items: &[EncodedValue], out: &mut Vec<u8>) {
    encode_uleb128(items.len() as u32, out);
    for v in items {
        encode_value(v, out);
    }
}

pub fn encode_annotation(a: &EncodedAnnotation, out: &mut Vec<u8>) {
    encode_uleb128(a.type_idx, out);
    encode_uleb128(a.elements.len() as u32, out);
    for (name, v) in &a.elements {
        encode_uleb128(*name, out);
        encode_value(v, out);
    }
}

/// The file being laid out: its bytes, and each section's offset and
/// count for the map.
struct Out {
    b: Vec<u8>,
    base: usize,
    map: Vec<(u16, u32, u32)>,
}

impl Out {
    fn align(&mut self, n: usize) {
        while (self.b.len() - self.base) % n != 0 {
            self.b.push(0);
        }
    }
    fn pos(&self) -> u32 {
        self.b.len() as u32
    }
    fn u16(&mut self, v: u16) {
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    fn put32(&mut self, at: u32, v: u32) {
        self.b[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn section(&mut self, kind: u16, count: usize, start: u32) {
        if count != 0 {
            self.map.push((kind, count as u32, start));
        }
    }
}

/// The DEX file of `ids` and `c` (at `base` of `prefix`, for a container's
/// files after the first: offsets are then from the container's start).
pub fn build(ids: &Ids, c: &Contents) -> Vec<u8> {
    build_at(ids, c, Vec::new())
}

pub fn build_at(ids: &Ids, c: &Contents, prefix: Vec<u8>) -> Vec<u8> {
    let base = prefix.len();
    let mut o = Out {
        b: prefix,
        base,
        map: Vec::new(),
    };
    let v41 = c.version >= 41;
    let header_size = if v41 { 0x78 } else { 0x70 };
    o.b.resize(base + header_size, 0);
    o.map.push((0x0000, 1, base as u32));
    let at = |o: &Out| o.pos();

    // The ids, their offsets filled in later.
    let string_ids = at(&o);
    o.b.resize(o.b.len() + 4 * ids.strings.len(), 0);
    o.section(0x0001, ids.strings.len(), string_ids);
    let type_ids = at(&o);
    for t in &ids.types {
        let s = ids.s(t);
        o.u32(s);
    }
    o.section(0x0002, ids.types.len(), type_ids);
    let proto_ids = at(&o);
    let mut proto_params_at = Vec::new();
    for (ret, params) in &ids.protos {
        let shorty: String = std::iter::once(ret.as_str())
            .chain(params.iter().map(String::as_str))
            .map(shorty_char)
            .collect();
        o.u32(ids.s(&shorty));
        o.u32(ids.t(ret));
        proto_params_at.push(o.pos());
        o.u32(0);
    }
    o.section(0x0003, ids.protos.len(), proto_ids);
    let field_ids = at(&o);
    for (class, name, ty) in &ids.fields {
        o.u16(ids.t(class) as u16);
        o.u16(ids.t(ty) as u16);
        o.u32(ids.s(name));
    }
    o.section(0x0004, ids.fields.len(), field_ids);
    let method_ids = at(&o);
    for (class, name, ret, params) in &ids.methods {
        o.u16(ids.t(class) as u16);
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        o.u16(ids.p(ret, &p) as u16);
        o.u32(ids.s(name));
    }
    o.section(0x0005, ids.methods.len(), method_ids);
    let class_defs = at(&o);
    o.b.resize(o.b.len() + 32 * c.classes.len(), 0);
    o.section(0x0006, c.classes.len(), class_defs);
    let call_site_ids = at(&o);
    o.b.resize(o.b.len() + 4 * c.call_sites.len(), 0);
    o.section(0x0007, c.call_sites.len(), call_site_ids);
    let method_handles = at(&o);
    for &(kind, idx) in &c.method_handles {
        o.u16(kind);
        o.u16(0);
        o.u16(idx);
        o.u16(0);
    }
    o.section(0x0008, c.method_handles.len(), method_handles);
    let data_off = at(&o);

    // Code items (and their debug information after them).
    let mut code_at: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    let mut debug_fixups = Vec::new();
    let code_start = {
        o.align(4);
        at(&o)
    };
    let mut ncode = 0;
    for (ci, class) in c.classes.iter().enumerate() {
        for (mi, m) in class
            .direct_methods
            .iter()
            .chain(&class.virtual_methods)
            .enumerate()
        {
            let Some(code) = &m.code else { continue };
            o.align(4);
            code_at.insert((ci, mi), at(&o));
            ncode += 1;
            o.u16(code.registers);
            o.u16(code.ins);
            o.u16(code.outs);
            o.u16(code.tries.len() as u16);
            debug_fixups.push((at(&o), code.debug.clone()));
            o.u32(0);
            o.u32(code.insns.len() as u32);
            for &u in &code.insns {
                o.u16(u);
            }
            if !code.tries.is_empty() {
                if code.insns.len() % 2 == 1 {
                    o.u16(0);
                }
                // The handler list, to know each handler's offset.
                let mut list = Vec::new();
                encode_uleb128(code.handlers.len() as u32, &mut list);
                let mut offs = Vec::new();
                for (typed, all) in &code.handlers {
                    offs.push(list.len() as u16);
                    let n = typed.len() as i32;
                    encode_sleb128(if all.is_some() { -n } else { n }, &mut list);
                    for &(t, a) in typed {
                        encode_uleb128(t, &mut list);
                        encode_uleb128(a, &mut list);
                    }
                    if let Some(a) = all {
                        encode_uleb128(*a, &mut list);
                    }
                }
                for &(start, count, h) in &code.tries {
                    o.u32(start);
                    o.u16(count);
                    o.u16(offs[h]);
                }
                o.b.extend_from_slice(&list);
            }
        }
    }
    o.section(0x2001, ncode, code_start);
    let debug_start = at(&o);
    let mut ndebug = 0;
    for (fix, debug) in debug_fixups {
        if let Some(d) = debug {
            let p = at(&o);
            o.put32(fix, p);
            o.b.extend_from_slice(&d);
            ndebug += 1;
        }
    }
    o.section(0x2003, ndebug, debug_start);

    // Type lists: protos' parameters, classes' interfaces.
    o.align(4);
    let lists_start = at(&o);
    let mut lists: Vec<(Vec<u16>, u32)> = Vec::new();
    let mut list = |o: &mut Out, l: Vec<u16>| -> u32 {
        if l.is_empty() {
            return 0;
        }
        if let Some((_, off)) = lists.iter().find(|(x, _)| *x == l) {
            return *off;
        }
        o.align(4);
        let p = o.pos();
        o.u32(l.len() as u32);
        for &t in &l {
            o.u16(t);
        }
        lists.push((l, p));
        p
    };
    for (i, (_, params)) in ids.protos.iter().enumerate() {
        let l: Vec<u16> = params.iter().map(|p| ids.t(p) as u16).collect();
        let off = list(&mut o, l);
        let fix = proto_params_at[i];
        o.put32(fix, off);
    }
    let mut interfaces_at = Vec::new();
    for class in &c.classes {
        interfaces_at.push(list(&mut o, class.interfaces.clone()));
    }
    let nlists = lists.len();
    o.section(0x1001, nlists, lists_start);

    // String data.
    let strings_start = at(&o);
    for (i, s) in ids.strings.iter().enumerate() {
        let p = at(&o);
        o.put32(string_ids + 4 * i as u32, p);
        encode_uleb128(s.len() as u32, &mut o.b);
        for &u in s {
            mutf8::encode_unit(u, &mut o.b);
        }
        o.b.push(0);
    }
    o.section(0x2002, ids.strings.len(), strings_start);

    // Class data.
    let class_data_start = at(&o);
    let mut class_data_at = Vec::new();
    let mut nclass_data = 0;
    for (ci, class) in c.classes.iter().enumerate() {
        let empty = class.static_fields.is_empty()
            && class.instance_fields.is_empty()
            && class.direct_methods.is_empty()
            && class.virtual_methods.is_empty();
        if empty {
            class_data_at.push(0);
            continue;
        }
        nclass_data += 1;
        class_data_at.push(at(&o));
        let b = &mut o.b;
        for n in [
            class.static_fields.len(),
            class.instance_fields.len(),
            class.direct_methods.len(),
            class.virtual_methods.len(),
        ] {
            encode_uleb128(n as u32, b);
        }
        for list in [&class.static_fields, &class.instance_fields] {
            let mut prev = 0;
            for &(f, a) in list.iter() {
                encode_uleb128(f - prev, b);
                encode_uleb128(a, b);
                prev = f;
            }
        }
        let mut mi = 0;
        for list in [&class.direct_methods, &class.virtual_methods] {
            let mut prev = 0;
            for m in list.iter() {
                encode_uleb128(m.method_idx - prev, b);
                encode_uleb128(m.access, b);
                encode_uleb128(code_at.get(&(ci, mi)).copied().unwrap_or(0), b);
                prev = m.method_idx;
                mi += 1;
            }
        }
    }
    o.section(0x2000, nclass_data, class_data_start);

    // Encoded arrays: static values, call sites.
    let arrays_start = at(&o);
    let mut narrays = 0;
    let mut static_values_at = Vec::new();
    for class in &c.classes {
        if class.static_values.is_empty() {
            static_values_at.push(0);
            continue;
        }
        static_values_at.push(at(&o));
        encode_array(&class.static_values, &mut o.b);
        narrays += 1;
    }
    for (i, site) in c.call_sites.iter().enumerate() {
        let p = at(&o);
        o.put32(call_site_ids + 4 * i as u32, p);
        encode_array(site, &mut o.b);
        narrays += 1;
    }
    o.section(0x2005, narrays, arrays_start);

    // Annotations: items, sets (sorted by type), ref lists, directories.
    let items_start = at(&o);
    let mut nitems = 0;
    // (Each class's sets, each a list of item offsets in type order.)
    let mut class_sets: Vec<Option<(Vec<u32>, Vec<Vec<u32>>, Vec<Vec<u32>>, Vec<Vec<Option<Vec<u32>>>>)>> =
        Vec::new();
    let mut item = |o: &mut Out, a: &Annotation| -> u32 {
        let p = o.pos();
        o.b.push(a.0);
        let mut ann = a.1.clone();
        ann.elements.sort_by_key(|e| e.0);
        encode_annotation(&ann, &mut o.b);
        nitems += 1;
        p
    };
    let sorted = |set: &[Annotation]| -> Vec<Annotation> {
        let mut s = set.to_vec();
        s.sort_by_key(|a| a.1.type_idx);
        s
    };
    for class in &c.classes {
        let Some(a) = &class.annotations else {
            class_sets.push(None);
            continue;
        };
        // (The directory's lists in index order, as the verifier wants.)
        let mut a = a.clone();
        a.fields.sort_by_key(|x| x.0);
        a.methods.sort_by_key(|x| x.0);
        a.parameters.sort_by_key(|x| x.0);
        let a = &a;
        let class_set: Vec<u32> = sorted(&a.class).iter().map(|x| item(&mut o, x)).collect();
        let fields: Vec<Vec<u32>> = a
            .fields
            .iter()
            .map(|(_, s)| sorted(s).iter().map(|x| item(&mut o, x)).collect())
            .collect();
        let methods: Vec<Vec<u32>> = a
            .methods
            .iter()
            .map(|(_, s)| sorted(s).iter().map(|x| item(&mut o, x)).collect())
            .collect();
        let params: Vec<Vec<Option<Vec<u32>>>> = a
            .parameters
            .iter()
            .map(|(_, ps)| {
                ps.iter()
                    .map(|s| s.as_ref().map(|s| sorted(s).iter().map(|x| item(&mut o, x)).collect()))
                    .collect()
            })
            .collect();
        class_sets.push(Some((class_set, fields, methods, params)));
    }
    o.section(0x2004, nitems, items_start);
    o.align(4);
    let sets_start = at(&o);
    let mut nsets = 0;
    let mut set = |o: &mut Out, items: &[u32]| -> u32 {
        o.align(4);
        let p = o.pos();
        o.u32(items.len() as u32);
        for &i in items {
            o.u32(i);
        }
        nsets += 1;
        p
    };
    let mut set_offsets = Vec::new();
    for cs in &class_sets {
        let Some((class_set, fields, methods, params)) = cs else {
            set_offsets.push(None);
            continue;
        };
        let class_off = if class_set.is_empty() { 0 } else { set(&mut o, class_set) };
        let f: Vec<u32> = fields.iter().map(|s| set(&mut o, s)).collect();
        let m: Vec<u32> = methods.iter().map(|s| set(&mut o, s)).collect();
        let p: Vec<Vec<u32>> = params
            .iter()
            .map(|ps| ps.iter().map(|s| s.as_ref().map_or(0, |s| set(&mut o, s))).collect())
            .collect();
        set_offsets.push(Some((class_off, f, m, p)));
    }
    o.section(0x1003, nsets, sets_start);
    o.align(4);
    let refs_start = at(&o);
    let mut nrefs = 0;
    let mut ref_offsets = Vec::new();
    for so in &set_offsets {
        let Some((_, _, _, p)) = so else {
            ref_offsets.push(Vec::new());
            continue;
        };
        let mut offs = Vec::new();
        for list in p {
            o.align(4);
            offs.push(o.pos());
            o.u32(list.len() as u32);
            for &s in list {
                o.u32(s);
            }
            nrefs += 1;
        }
        ref_offsets.push(offs);
    }
    o.section(0x1002, nrefs, refs_start);
    o.align(4);
    let dirs_start = at(&o);
    let mut ndirs = 0;
    let mut dir_at = Vec::new();
    for (ci, class) in c.classes.iter().enumerate() {
        let (Some(a), Some((class_off, f, m, _))) = (&class.annotations, &set_offsets[ci]) else {
            dir_at.push(0);
            continue;
        };
        let mut a = a.clone();
        a.fields.sort_by_key(|x| x.0);
        a.methods.sort_by_key(|x| x.0);
        a.parameters.sort_by_key(|x| x.0);
        o.align(4);
        dir_at.push(o.pos());
        ndirs += 1;
        o.u32(*class_off);
        o.u32(a.fields.len() as u32);
        o.u32(a.methods.len() as u32);
        o.u32(a.parameters.len() as u32);
        for (i, (idx, _)) in a.fields.iter().enumerate() {
            o.u32(*idx);
            o.u32(f[i]);
        }
        for (i, (idx, _)) in a.methods.iter().enumerate() {
            o.u32(*idx);
            o.u32(m[i]);
        }
        for (i, (idx, _)) in a.parameters.iter().enumerate() {
            o.u32(*idx);
            o.u32(ref_offsets[ci][i]);
        }
    }
    o.section(0x2006, ndirs, dirs_start);

    // Hidden-API flags.
    if c.classes.iter().any(|cl| cl.hiddenapi.is_some()) {
        o.align(4);
        let start = o.pos();
        let mut sec = vec![0u8; 4 + 4 * c.classes.len()];
        for (i, class) in c.classes.iter().enumerate() {
            if let Some(flags) = &class.hiddenapi {
                let off = sec.len() as u32;
                sec[4 + 4 * i..8 + 4 * i].copy_from_slice(&off.to_le_bytes());
                for &f in flags {
                    encode_uleb128(f, &mut sec);
                }
            }
        }
        let n = sec.len() as u32;
        sec[..4].copy_from_slice(&n.to_le_bytes());
        o.b.extend_from_slice(&sec);
        o.section(0xF000, 1, start);
    }

    // The class definitions.
    for (i, class) in c.classes.iter().enumerate() {
        let at = class_defs as usize + 32 * i;
        let fields = [
            class.class_idx,
            class.access,
            class.superclass.unwrap_or(0xFFFF_FFFF),
            interfaces_at[i],
            class.source_file.unwrap_or(0xFFFF_FFFF),
            dir_at[i],
            class_data_at[i],
            static_values_at[i],
        ];
        for (k, v) in fields.iter().enumerate() {
            o.b[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    // The map.
    o.align(4);
    let map_off = o.pos();
    o.map.push((0x1000, 1, map_off));
    o.map.sort_by_key(|m| m.2);
    let map = o.map.clone();
    o.u32(map.len() as u32);
    for (kind, count, off) in map {
        o.u16(kind);
        o.u16(0);
        o.u32(count);
        o.u32(off);
    }
    let end = o.pos();

    // The header.
    let h = base;
    let b = &mut o.b;
    b[h..h + 8].copy_from_slice(format!("dex\n{:03}\0", c.version).as_bytes());
    let put = |b: &mut Vec<u8>, at: usize, v: u32| b[h + at..h + at + 4].copy_from_slice(&v.to_le_bytes());
    put(b, 32, end - base as u32);
    put(b, 36, header_size as u32);
    put(b, 40, 0x1234_5678);
    put(b, 52, map_off);
    let counts = [
        (ids.strings.len(), string_ids),
        (ids.types.len(), type_ids),
        (ids.protos.len(), proto_ids),
        (ids.fields.len(), field_ids),
        (ids.methods.len(), method_ids),
        (c.classes.len(), class_defs),
    ];
    for (k, (n, off)) in counts.iter().enumerate() {
        put(b, 56 + 8 * k, *n as u32);
        put(b, 60 + 8 * k, if *n == 0 { 0 } else { *off });
    }
    if v41 {
        put(b, 104, 0);
        put(b, 108, 0);
        put(b, 112, end);
        put(b, 116, base as u32);
    } else {
        put(b, 104, end - data_off);
        put(b, 108, data_off);
    }
    sign(b, h, end as usize);
    o.b
}

/// Writes the signature and checksum of the DEX file from `h` to `end`.
pub fn sign(b: &mut [u8], h: usize, end: usize) {
    use sha1::Digest;
    let s = sha1::Sha1::digest(&b[h + 32..end]);
    b[h + 12..h + 32].copy_from_slice(&s);
    let c = adler32(&b[h + 12..end]);
    b[h + 8..h + 12].copy_from_slice(&c.to_le_bytes());
}

/// The container of `files` (each its ids and contents, version 041):
/// one after another, every header's `container_size` the whole.
pub fn container(files: &[(&Ids, &Contents)]) -> Vec<u8> {
    let mut b = Vec::new();
    let mut headers = Vec::new();
    for (ids, c) in files {
        headers.push(b.len());
        b = build_at(ids, c, b);
    }
    let total = b.len() as u32;
    for &h in &headers {
        b[h + 112..h + 116].copy_from_slice(&total.to_le_bytes());
        let size = u32::from_le_bytes(b[h + 32..h + 36].try_into().unwrap()) as usize;
        sign(&mut b, h, h + size);
    }
    b
}
