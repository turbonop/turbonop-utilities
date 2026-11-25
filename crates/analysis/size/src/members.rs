//! The code and data each type owns: its member functions (and the lambdas
//! and local classes inside them), its static data members, its vtables
//! and RTTI. What a class costs the binary besides the bytes of an
//! instance.
//!
//! A symbol belongs to the innermost scope of its qualified name that is a
//! type the debug information defines: `ns::C::f(int)` and
//! `ns::C::f::<lambda_1>::operator()` are `ns::C`'s, `ns::C::\`vftable'`
//! too, and `ns::f()` no type's. Names are compared as [`normalize`]
//! spells them, since a type's name and its members' symbols may space
//! template arguments differently.

use turbonop_hash::Map as HashMap;

use crate::Size;
use crate::symbols::{Symbol, SymbolKind};

/// What one type owns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owned {
    /// Its code: member functions (and their separated blocks, thunks).
    pub code: Size,
    /// Its data: static members, vtables, RTTI, and other data it names.
    pub data: Size,
    /// How many functions (a function split into blocks once; names
    /// folded into another's code counted, with no bytes).
    pub functions: usize,
    /// Its symbols, as indexes into the symbols given.
    pub symbols: Vec<u32>,
}

/// Whether symbols of `kind` are code (else data, or neither).
fn is_code(kind: SymbolKind) -> Option<bool> {
    match kind {
        SymbolKind::Function
        | SymbolKind::PrimaryBlock
        | SymbolKind::SeparatedBlock
        | SymbolKind::Thunk => Some(true),
        SymbolKind::Data | SymbolKind::Public => Some(false),
        _ => None,
    }
}

/// `name` spelled one way: no spaces beside `<`, `>`, `,`, `*`, or `&`,
/// and no `class`, `struct`, `union`, or `enum` before a template argument
/// (`std::vector<class A, std::allocator<A> >` is
/// `std::vector<A,std::allocator<A>>`).
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name.trim();
    let mut at_argument = true;
    while !rest.is_empty() {
        if at_argument {
            for keyword in ["class ", "struct ", "union ", "enum "] {
                if let Some(r) = rest.strip_prefix(keyword) {
                    rest = r.trim_start();
                }
            }
        }
        let Some(c) = rest.chars().next() else { break };
        rest = &rest[c.len_utf8()..];
        if c == ' ' {
            let next = rest.trim_start().chars().next();
            let before = out.chars().last();
            let tight = |ch: Option<char>| matches!(ch, Some('<' | '>' | ',' | '*' | '&'));
            if tight(before) || tight(next) || next.is_none() {
                continue;
            }
        }
        at_argument = matches!(c, '<' | ',');
        out.push(c);
    }
    out
}

/// The scopes `name` is qualified by, innermost first: `a::b<c::d>::f(x)`
/// gives `a::b<c::d>` and `a`. Parameters, an operator's name, and
/// anything before the qualified name (a return type, `static`) aren't
/// scopes; `::` inside template arguments, parentheses, braces, and MSVC's
/// quoted names (`` `anonymous namespace' ``) don't separate them.
pub fn scopes(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut depth = 0i32;
    let mut quoted = 0i32;
    // Where the qualified name starts (past a depth-0 space), and each
    // depth-0 `::`.
    let mut start = 0;
    let mut seps: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if quoted > 0 {
            match b {
                b'`' => quoted += 1,
                b'\'' => quoted -= 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        match b {
            b'`' => quoted += 1,
            b'<' | b'(' | b'[' | b'{' if depth == 0 && b != b'<' => {
                // Parameters, or a vtable's `{for ...}`: the name ends.
                if b == b'(' || b == b'{' {
                    break;
                }
                depth += 1;
            }
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => depth = (depth - 1).max(0),
            b' ' if depth == 0 => {
                start = i + 1;
                seps.retain(|&s| s >= start);
            }
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                seps.push(i);
                i += 2;
                // An operator's name (`operator<`, `operator()`) isn't
                // parsed: the qualified name ends there.
                if name[i..].starts_with("operator") {
                    break;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    seps.iter()
        .rev()
        .filter(|&&s| s > start)
        .map(|&s| &name[start..s])
        .collect()
}

/// The type (one of `types`, by its index) that owns the symbol `name`:
/// the innermost of its scopes that is one. A function's static (`x (in
/// static bool C::f())`) is the function's type's.
fn owner(name: &str, types: &HashMap<String, u32>) -> Option<u32> {
    let qualified = match name.split_once(" (in ") {
        Some((_, f)) if name.ends_with(')') => {
            // The function's name, then a scope of its own.
            let f = f.strip_suffix(')').unwrap_or(f);
            return std::iter::once(crate::findings::strip_parameters(f))
                .chain(scopes(f))
                .map(|s| s.rsplit_once(' ').map_or(s, |(_, n)| n))
                .find_map(|s| types.get(&normalize(s)).copied());
        }
        _ => name,
    };
    scopes(qualified)
        .into_iter()
        .find_map(|s| types.get(&normalize(s)).copied())
}

/// What each of `types` (qualified names) owns of `symbols`, by its index
/// in `types`; types that own nothing are absent.
pub fn by_type(symbols: &[Symbol], types: &[&str]) -> HashMap<u32, Owned> {
    // (A name given twice is the first's.)
    let mut index: HashMap<String, u32> = HashMap::default();
    for (i, t) in types.iter().enumerate() {
        if let Ok(i) = u32::try_from(i) {
            index.entry(normalize(t)).or_insert(i);
        }
    }
    // Each symbol's owner, on every core.
    let owners: Vec<Option<u32>> = turbonop_par::map(symbols, 4096, |s| {
        is_code(s.kind)?;
        owner(&s.name, &index)
    });
    let mut out: HashMap<u32, Owned> = HashMap::default();
    for (i, (s, o)) in symbols.iter().zip(&owners).enumerate() {
        let (Some(t), Some(code)) = (o, is_code(s.kind)) else {
            continue;
        };
        let owned = out.entry(*t).or_default();
        if code {
            owned.code += s.size;
            if s.kind != SymbolKind::SeparatedBlock {
                owned.functions += 1;
            }
        } else {
            owned.data += s.size;
        }
        owned.symbols.push(u32::try_from(i).unwrap_or(u32::MAX));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_spelled_one_way() {
        assert_eq!(
            normalize("std::vector<class A, std::allocator<A> >"),
            "std::vector<A,std::allocator<A>>"
        );
        assert_eq!(normalize("ns::C<struct B *>"), "ns::C<B*>");
        assert_eq!(normalize("unsigned int"), "unsigned int");
    }

    #[test]
    fn scopes_of_names() {
        assert_eq!(scopes("a::b<c::d>::f(x::y)"), ["a::b<c::d>", "a"]);
        assert_eq!(scopes("f()"), Vec::<&str>::new());
        assert_eq!(scopes("C::operator()(int)"), ["C"]);
        assert_eq!(scopes("C::operator<(C const&)"), ["C"]);
        assert_eq!(scopes("C::operator new(unsigned __int64)"), ["C"]);
        assert_eq!(scopes("C::`vftable'"), ["C"]);
        assert_eq!(scopes("C::`vftable'{for `I::J'}"), ["C"]);
        assert_eq!(
            scopes("`anonymous namespace'::C::f()"),
            ["`anonymous namespace'::C", "`anonymous namespace'"]
        );
        assert_eq!(
            scopes("C::f::<lambda_1>::operator()() const"),
            ["C::f::<lambda_1>", "C::f", "C"]
        );
        // What comes before the qualified name isn't a scope.
        assert_eq!(scopes("static bool C::f()"), ["C"]);
    }

    fn symbol(name: &str, kind: SymbolKind, size: u64) -> Symbol {
        Symbol {
            addr: 0,
            size: Size::new(size, size),
            kind,
            folded: false,
            name: name.into(),
            compiland: None,
            file_static: false,
        }
    }

    #[test]
    fn symbols_by_their_types() {
        let symbols = [
            symbol("ns::C::f(int)", SymbolKind::Function, 100),
            symbol(
                "ns::C::f::<lambda_1>::operator()() const",
                SymbolKind::Function,
                20,
            ),
            symbol("ns::C::`vftable'", SymbolKind::Data, 16),
            symbol("ns::C::count", SymbolKind::Data, 4),
            symbol("x (in static bool ns::C::g())", SymbolKind::Data, 8),
            symbol("ns::C::g()", SymbolKind::PrimaryBlock, 30),
            symbol("ns::C::g()", SymbolKind::SeparatedBlock, 10),
            symbol("ns::f()", SymbolKind::Function, 50),
            symbol("ns::V<int, ns::C>::push(int)", SymbolKind::Function, 40),
            symbol("[pdata] ns::C::f(int)", SymbolKind::Pdata, 12),
        ];
        let types = ["ns::C", "ns::V<int,ns::C>", "ns::Unused"];
        let owned = by_type(&symbols, &types);
        let c = &owned[&0];
        assert_eq!(c.code, Size::new(160, 160));
        assert_eq!(c.data, Size::new(28, 28));
        assert_eq!(c.functions, 3);
        assert_eq!(c.symbols, [0, 1, 2, 3, 4, 5, 6]);
        let v = &owned[&1];
        assert_eq!((v.code.file, v.functions), (40, 1));
        assert!(!owned.contains_key(&2));
    }
}
