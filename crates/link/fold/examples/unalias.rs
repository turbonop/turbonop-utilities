//! Turns an object's alias weak externals into plain undefined externals,
//! for experiments with how linkers treat them: `unalias <in> <out>
//! [--keep-alias]`. With `--keep-alias`, each also gets a second record of
//! the same name: the alias, which relocations don't name.
use turbonop_coff::{Aux, File, Symbol, storage_class, weak_search};
use turbonop_global_alloc as _;

fn main() {
    let mut args = std::env::args().skip(1);
    let (input, output) = (args.next().unwrap(), args.next().unwrap());
    let keep = args.next().as_deref() == Some("--keep-alias");
    let data = std::fs::read(&input).unwrap();
    let File::Object(o) = File::parse(&data).unwrap() else {
        panic!("not a COFF object")
    };
    let mut o = o.into_owned();
    let mut aliases: Vec<Symbol<'_>> = Vec::new();
    for s in &mut o.symbols {
        if let Aux::WeakExternal(w) = s.aux
            && w.search == weak_search::ALIAS
        {
            aliases.push(s.clone());
            s.storage_class = storage_class::EXTERNAL;
            s.aux = Aux::None;
        }
    }
    let n = aliases.len();
    if keep {
        for a in aliases {
            o.push_symbol(a);
        }
    }
    std::fs::write(&output, o.write().unwrap()).unwrap();
    println!(
        "{n} aliases made undefined{}",
        if keep { ", kept beside" } else { "" }
    );
}
