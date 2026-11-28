//! Lists the functions in a bitcode file (or the bitcode an object or
//! archive embeds): each one's linkage, visibility, instructions, basic
//! blocks, the size of its block, what it calls, and its source.
//!
//! ```text
//! cargo run --release -p turbonop-bitcode --example functions -- <file>
//! ```

use turbonop_global_alloc as _;

use turbonop_bitcode::embedded::find;
use turbonop_bitcode::{Bitcode, Callee, Visibility};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: functions <file>");
        std::process::exit(2);
    };
    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1)
    });
    for e in find(&data) {
        let bc = match Bitcode::parse(e.data) {
            Ok(bc) => bc,
            Err(err) => {
                eprintln!("{path} at {:#x}: {err}", e.offset);
                continue;
            }
        };
        for m in &bc.modules {
            println!(
                "{}{} ({})",
                e.member
                    .as_deref()
                    .map_or_else(String::new, |n| format!("{}: ", String::from_utf8_lossy(n))),
                String::from_utf8_lossy(&m.source_filename),
                String::from_utf8_lossy(&m.triple)
            );
            for (f, b) in m.defined() {
                let linkage = match f.linkage.name() {
                    "" => "external",
                    l => l,
                };
                let visibility = match f.visibility {
                    Visibility::Default => "",
                    Visibility::Hidden => " hidden",
                    Visibility::Protected => " protected",
                };
                let callees: Vec<String> = b
                    .calls
                    .iter()
                    .map(|c| match c.callee {
                        Callee::Function(i) => m.ir_name(m.functions[i].value).unwrap_or_default(),
                        Callee::Alias(i) => m.ir_name(m.aliases[i].value).unwrap_or_default(),
                        Callee::Global(i) => m.ir_name(m.globals[i].value).unwrap_or_default(),
                        Callee::InlineAsm => "asm".to_string(),
                        Callee::Indirect => "indirect".to_string(),
                    })
                    .collect();
                let source = b.subprogram(m).map_or_else(String::new, |sp| {
                    let file = sp.file.map_or_else(String::new, |f| {
                        String::from_utf8_lossy(f.filename).into_owned()
                    });
                    format!("  {file}:{}", sp.line)
                });
                println!(
                    "  {} {linkage}{visibility}: {} instructions, {} blocks, {} bits{source}",
                    m.ir_name(f.value).unwrap_or_default(),
                    b.instructions.len(),
                    b.blocks,
                    b.bits,
                );
                if !callees.is_empty() {
                    println!("    calls {}", callees.join(", "));
                }
            }
        }
    }
}
