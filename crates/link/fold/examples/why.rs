//! Why units don't fold: `why [--locals] [--name SUBSTRING] <object>...`
//! prints each refused unit's leader, reason, and detail. With `--locals`,
//! as `--locals=copy` finds them; with `--name`, every unit whose first
//! external name contains SUBSTRING, with its structural code and what it
//! reaches.
use turbonop_global_alloc as _;

use turbonop_fold::rewrite::locals::Node;
use turbonop_fold::rewrite::units::{self, Mode};

fn main() {
    let mut mode = Mode::default();
    let mut name: Option<String> = None;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--locals" => mode.copy_locals = true,
            "--name" => name = args.next(),
            _ => paths.push(a),
        }
    }
    for path in paths {
        let data = std::fs::read(&path).unwrap();
        let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(&data) else {
            continue;
        };
        let facts = match units::find_with(&o, mode) {
            Ok(f) => f,
            Err(e) => {
                println!("{path}: passed through: {e:?}");
                continue;
            }
        };
        for u in &facts.units {
            let leader = &o.sections[u.leader.index()];
            let first = u
                .externals
                .first()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            if let Some(want) = &name {
                if !first.contains(want.as_str()) {
                    continue;
                }
                println!(
                    "{path}\t{first}\t{:032x}\t{:?}\t{}",
                    u.structural(),
                    u.unfoldable,
                    u.detail.as_deref().unwrap_or("")
                );
                for node in u.closure().iter().skip(1) {
                    match *node {
                        Node::Piece(p) => {
                            let piece = &facts.pieces[p as usize];
                            let sec = &o.sections[piece.section.index()];
                            println!(
                                "\tpiece {} {}..{} {:02x?} {:?}",
                                String::from_utf8_lossy(sec.name.as_bytes()),
                                piece.start,
                                piece.end,
                                &sec.bytes()[piece.start as usize..piece.end as usize],
                                piece.blocked
                            );
                        }
                        Node::Unit(v) => {
                            let v = &facts.units[v as usize];
                            println!(
                                "\tlocal unit {} {:032x} {:?}",
                                String::from_utf8_lossy(
                                    o.sections[v.leader.index()].name.as_bytes()
                                ),
                                v.structural(),
                                v.blocked()
                            );
                        }
                    }
                }
                continue;
            }
            if let Some(why) = u.unfoldable {
                println!(
                    "{:?}\t{}\t{}\t{}",
                    why,
                    String::from_utf8_lossy(leader.name.as_bytes()),
                    first,
                    u.detail.as_deref().unwrap_or("")
                );
            }
        }
    }
}
