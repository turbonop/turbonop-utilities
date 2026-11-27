//! Methods' parameters, from the demangled names of their public symbols
//! (`public: virtual long __cdecl CC2::XObjHandler10::CodeGen(int,unsigned
//! short * *,struct ILinkDataRO *,struct ILinkDataRO * *)`): what the probe
//! needs to show each argument (a string, a buffer, an interface it should
//! wrap, a number).

/// How an argument is shown, and what the probe does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Param {
    /// A number (an integer, a `bool`, an enumeration).
    Scalar,
    /// A `GUID const &` (an interface's identifier).
    Guid,
    /// A wide string (`unsigned short const *`, `wchar_t const *`).
    WStr,
    /// `unsigned short *`: a wide string given, or a number returned.
    WStrOrOut,
    /// `unsigned short * *`: an array of wide strings (after an `int`
    /// count: an argument vector), or a string returned.
    WStrs,
    /// A narrow string (`char const *`).
    Str,
    /// An interface given (`struct ILinkDataRO *`).
    Iface(String),
    /// An interface returned (`struct IObjFile2 * *`).
    OutIface(String),
    /// A scalar returned through a pointer (`unsigned long *`), of this
    /// many bytes.
    OutScalar(u8),
    /// Bytes (`unsigned char *`), given or filled.
    Bytes,
    /// Anything else, shown as its bits.
    Raw,
}

/// A method: its name and parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    /// Its name (`CodeGen`), or the candidates' (`Init|RequestPchSymbol`)
    /// when the slot's function was folded with others.
    pub name: String,
    /// Its parameters (none known for a folded slot's).
    pub params: Vec<Param>,
    /// Whether its parameters are known (one candidate).
    pub known: bool,
}

/// The method a demangled name is (with its class's `::` prefix and
/// parameter list): `Some(("CodeGen", params))`.
pub fn parse(demangled: &str) -> Option<(String, Vec<Param>)> {
    // The parameter list: the last top-level parenthesized group (a
    // `const` may follow it).
    let close = demangled.rfind(')')?;
    let mut depth = 0i32;
    let mut open = None;
    for (i, c) in demangled[..=close].char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let open = open?;
    let head = demangled[..open].trim_end();
    // An adjustor thunk: `...::Release`adjustor{8}' `.
    let head = head.split('`').next().unwrap_or(head);
    let name = head.rsplit("::").next()?.trim().to_string();
    let list = demangled[open + 1..close].trim();
    let params = if list == "void" || list.is_empty() {
        Vec::new()
    } else {
        split(list)
            .into_iter()
            .map(|p| classify(p.trim()))
            .collect()
    };
    Some((name, params))
}

/// A parameter list's parameters (commas inside template arguments and
/// function types are theirs).
fn split(list: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in list.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&list[start..]);
    out
}

/// What one parameter's type is.
fn classify(ty: &str) -> Param {
    let constant = ty.contains("const");
    let words: Vec<&str> = ty
        .split_whitespace()
        .filter(|w| {
            !matches!(
                *w,
                "struct" | "class" | "enum" | "union" | "const" | "__ptr64"
            )
        })
        .collect();
    let stars = words.iter().filter(|w| **w == "*").count();
    let reference = words.contains(&"&");
    let base: Vec<&str> = words
        .iter()
        .copied()
        .filter(|w| *w != "*" && *w != "&")
        .collect();
    let base = base.join(" ");
    let is_wide = matches!(base.as_str(), "unsigned short" | "wchar_t");
    let is_interface = base.starts_with('I')
        && base.len() > 1
        && base[1..2].chars().all(|c| c.is_ascii_uppercase())
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let is_enum = ty.trim_start().starts_with("enum ");
    match (stars, reference) {
        (0, true) if base == "_GUID" => Param::Guid,
        (0, false) => Param::Scalar,
        (1, false) if is_wide && constant => Param::WStr,
        (1, false) if is_wide => Param::WStrOrOut,
        (1, false) if base == "char" => Param::Str,
        (1, false) if base == "unsigned char" => Param::Bytes,
        (1, false) if is_interface || base == "IUnknown" => Param::Iface(base),
        (1, false) if is_enum => Param::OutScalar(4),
        (1, false) if is_scalar(&base) => Param::OutScalar(width(&base)),
        (2, false) if is_wide => Param::WStrs,
        (2, false) if is_interface || base == "IUnknown" => Param::OutIface(base),
        _ => Param::Raw,
    }
}

/// An integer type's size in bytes.
fn width(base: &str) -> u8 {
    match base {
        "unsigned __int64" | "__int64" => 8,
        "short" | "unsigned short" => 2,
        "char" | "bool" => 1,
        _ => 4,
    }
}

/// Whether a type (without its pointers) is an integer.
fn is_scalar(base: &str) -> bool {
    matches!(
        base,
        "int"
            | "long"
            | "short"
            | "char"
            | "bool"
            | "unsigned int"
            | "unsigned long"
            | "unsigned short"
            | "unsigned __int64"
            | "__int64"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_generation_s_parameters() {
        let (name, params) = parse(
            "public: virtual long __cdecl CC2::XObjHandler10::CodeGen(int,unsigned short * *,struct ILinkDataRO *,struct ILinkDataRO * *)",
        )
        .unwrap();
        assert_eq!(name, "CodeGen");
        assert_eq!(
            params,
            [
                Param::Scalar,
                Param::WStrs,
                Param::Iface("ILinkDataRO".into()),
                Param::OutIface("ILinkDataRO".into()),
            ]
        );
    }

    #[test]
    fn strings_guids_enums_and_outs() {
        let (_, p) = parse(
            "public: virtual long __cdecl CC2::XObjHandler10::IObjFileForFile(unsigned short *,unsigned short,unsigned long,unsigned long,unsigned __int64,enum __MIDL___MIDL_itf_objint_0000_0000_0002,struct IObjFile2 * *)",
        )
        .unwrap();
        assert_eq!(
            p,
            [
                Param::WStrOrOut,
                Param::Scalar,
                Param::Scalar,
                Param::Scalar,
                Param::Scalar,
                Param::Scalar,
                Param::OutIface("IObjFile2".into()),
            ]
        );
        let (name, p) = parse(
            "public: virtual long __cdecl CLinkNotify::QueryInterface(struct _GUID const &,void * *)",
        )
        .unwrap();
        assert_eq!(name, "QueryInterface");
        assert_eq!(p, [Param::Guid, Param::Raw]);
        let (_, p) = parse(
            "public: virtual long __cdecl CPublicSym::XPublicSym5::SymType(enum __MIDL___MIDL_itf_objint_0000_0000_0001 *)",
        )
        .unwrap();
        assert_eq!(p, [Param::OutScalar(4)]);
        let (_, p) = parse("public: virtual long __cdecl CC2::XObjHandler10::SetPdbDll(unsigned short const *,unsigned short * *)").unwrap();
        assert_eq!(p, [Param::WStr, Param::WStrs]);
        let (_, p) =
            parse("public: virtual long __cdecl CC2::XArm64XObj::MarkDefinedECSym(char const *)")
                .unwrap();
        assert_eq!(p, [Param::Str]);
    }

    #[test]
    fn no_parameters_and_thunks() {
        assert_eq!(
            parse("public: virtual unsigned long __cdecl CC2::XObjHandler10::AddRef(void)"),
            Some(("AddRef".into(), vec![]))
        );
        let (name, p) = parse(
            "[thunk]:public: virtual unsigned long __cdecl CImportObject::Release`adjustor{8}' (void)",
        )
        .unwrap();
        assert_eq!((name.as_str(), p.len()), ("Release", 0));
        let (name, p) =
            parse("public: virtual long __cdecl CCOFFSym::XCOFFSymRW::SetISection(int)").unwrap();
        assert_eq!((name.as_str(), p), ("SetISection", vec![Param::Scalar]));
    }

    #[test]
    fn templates_commas_are_theirs() {
        assert_eq!(split("int,std::map<int,int>,void (*)(int,int)").len(), 3);
    }
}
