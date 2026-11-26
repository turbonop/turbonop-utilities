//! Why units don't fold: `why <object>...` prints each refused unit's
//! leader, reason, and detail.
use turbonop_global_alloc as _;

fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(&data) else {
            continue;
        };
        let facts = match turbonop_fold::rewrite::units::find(&o) {
            Ok(f) => f,
            Err(e) => {
                println!("{path}: passed through: {e:?}");
                continue;
            }
        };
        for u in &facts.units {
            if let Some(why) = u.unfoldable {
                let leader = &o.sections[u.leader.index()];
                let name = u
                    .externals
                    .first()
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default();
                println!(
                    "{:?}\t{}\t{}\t{}",
                    why,
                    String::from_utf8_lossy(leader.name.as_bytes()),
                    name,
                    u.detail.as_deref().unwrap_or("")
                );
            }
        }
    }
}
