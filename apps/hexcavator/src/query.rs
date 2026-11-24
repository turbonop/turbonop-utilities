//! `hexcavator query`: an image's symbols, filtered, grouped, and sorted (see
//! `turbonop_query` for the language).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use turbonop_query::{Output, Value};
use turbonop_size::table::{COLUMNS, SymbolTable};
use turbonop_size::{Model, symbols};

pub const USAGE: &str = "\
usage: hexcavator query [options] <image> <query>

An image's symbols (from its PDB or DWARF), as a table to filter, group,
and sort:

  hexcavator query app.exe \"kind=function file>4k sort by file desc limit 20\"
  hexcavator query app.exe \"group by library\"
  hexcavator query app.exe \"name~'^std::' group by compiland limit 10\"
  hexcavator query app.exe \"section=.rdata static group by name sort by count desc\"

The query is a filter (column op value, joined by and, or, not, and
parentheses; ops = != < <= > >= ~ !~, ~ a regular expression; numbers
take k, M, G), then any of: group by <columns> (with count and summed
sizes), sort by <column> [desc], select <columns>, limit <n>.

options:
  -debug <path> its debug information: a PDB, or a file with its DWARF
                (or -debug; default: its own, or the one it names)
  -csv, -json   output format (default: a table)
  -columns      list the columns and what they hold";

pub fn run(args: &[String]) -> ExitCode {
    let mut pdb = None;
    let mut format = "table";
    let mut list_columns = false;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-pdb" | "/pdb" | "-debug" | "/debug" => match it.next() {
                Some(p) => pdb = Some(PathBuf::from(p)),
                None => return usage("-pdb needs a path"),
            },
            "-csv" | "/csv" => format = "csv",
            "-json" | "/json" => format = "json",
            "-columns" | "/columns" => list_columns = true,
            _ => rest.push(a.as_str()),
        }
    }
    if list_columns {
        let mut out = String::new();
        for c in COLUMNS {
            let _ = writeln!(out, "  {:<10} {}", c.name, c.doc);
        }
        return crate::out::print(out.as_bytes());
    }
    let [image, query] = rest[..] else {
        return usage(if rest.is_empty() {
            "no image or query given"
        } else if rest.len() == 1 {
            "no query given (quote it: \"kind=function ...\")"
        } else {
            "too many arguments (quote the query: \"kind=function ...\")"
        });
    };
    let parsed = match turbonop_query::parse(query, &COLUMNS) {
        Ok(q) => q,
        Err(e) => {
            ereport!("hexcavator query: {e}\n  {query}\n  {}^", " ".repeat(e.at));
            return ExitCode::from(2);
        }
    };
    let (model, syms, modules, lines) = match load(Path::new(image), pdb.as_deref()) {
        Ok(x) => x,
        Err(e) => {
            ereport!("hexcavator query: {e}");
            return ExitCode::FAILURE;
        }
    };
    let table = SymbolTable::new(&model, &syms, &modules).with_lines(&lines);
    let out = match turbonop_query::run(&parsed, &table) {
        Ok(o) => o,
        Err(e) => {
            ereport!("hexcavator query: {}", e.message);
            return ExitCode::from(2);
        }
    };
    let text = match format {
        "csv" => csv(&out),
        "json" => json_text(&out),
        _ => table_text(&out),
    };
    let code = crate::out::print(text.as_bytes());
    crate::out::dispose((out, table));
    crate::out::dispose((model, syms, modules, lines));
    code
}

fn usage(message: &str) -> ExitCode {
    ereport!("hexcavator query: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

/// What [`load`] reads: the model, symbols, compilands, and source lines.
type Loaded = (
    std::sync::Arc<Model>,
    std::sync::Arc<Vec<symbols::Symbol>>,
    Vec<turbonop_debuginfo::Compiland>,
    turbonop_debuginfo::SourceLines,
);

/// The model and symbols of the image at `path`.
pub(crate) fn load(path: &Path, pdb: Option<&Path>) -> Result<Loaded, String> {
    let opened = turbonop_program::Opened::open(path, pdb)?;
    let program = opened.program()?;
    let show = opened.debug_path().display();
    // (The compilands and lines are read while the symbols are built.)
    let (modules, lines, built) = std::thread::scope(|scope| {
        let modules = scope.spawn(|| program.debug.compilands());
        let lines = scope.spawn(|| program.debug.source_lines());
        let built = program.symbols().map(|_| ());
        (
            modules.join().expect("compilands"),
            lines.join().expect("source lines"),
            built,
        )
    });
    built?;
    let modules = modules.map_err(|e| format!("{show}: {e}"))?;
    let lines = lines.map_err(|e| format!("{show}: {e}"))?;
    let (model, syms) = program.into_built()?;
    crate::out::dispose(opened);
    Ok((model, syms, modules, lines))
}

/// As an aligned table: numbers right, the rest left, then how many rows
/// matched.
pub(crate) fn table_text(out: &Output) -> String {
    let cells: Vec<Vec<String>> = out
        .rows
        .iter()
        .map(|r| r.iter().map(|v| v.text().into_owned()).collect())
        .collect();
    let width = |i: usize| {
        cells
            .iter()
            .map(|r| r[i].chars().count())
            .chain([out.columns[i].len()])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..out.columns.len()).map(width).collect();
    let numeric: Vec<bool> = (0..out.columns.len())
        .map(|i| {
            out.rows
                .first()
                .is_some_and(|r| matches!(r[i], Value::Int(_)))
        })
        .collect();
    let mut text = String::new();
    let line = |text: &mut String, row: &[String]| {
        let last = row.len() - 1;
        for (i, cell) in row.iter().enumerate() {
            let pad = widths[i];
            if numeric[i] {
                let _ = write!(text, "{cell:>pad$}");
            } else if i == last {
                text.push_str(cell);
            } else {
                let _ = write!(text, "{cell:<pad$}");
            }
            text.push_str(if i == last { "\n" } else { "  " });
        }
    };
    line(&mut text, &out.columns);
    for r in &cells {
        line(&mut text, r);
    }
    let _ = writeln!(
        text,
        "({} of {} matching rows)",
        out.rows.len(),
        out.matched
    );
    text
}

fn csv(out: &Output) -> String {
    let field = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut text = out.columns.join(",");
    text.push('\n');
    for r in &out.rows {
        let row: Vec<String> = r
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                v => field(&v.text()),
            })
            .collect();
        text.push_str(&row.join(","));
        text.push('\n');
    }
    text
}

fn json_text(out: &Output) -> String {
    let rows: Vec<serde_json::Value> = out
        .rows
        .iter()
        .map(|r| {
            let mut o = serde_json::Map::new();
            for (c, v) in out.columns.iter().zip(r) {
                o.insert(c.clone(), value_json(v));
            }
            serde_json::Value::Object(o)
        })
        .collect();
    let mut s = serde_json::to_string_pretty(&json!({"matched": out.matched, "rows": rows}))
        .unwrap_or_default();
    s.push('\n');
    s
}

/// A value as JSON.
pub(crate) fn value_json(v: &Value<'_>) -> serde_json::Value {
    match v {
        Value::Int(n) => json!(n),
        Value::Str(s) => json!(s),
        Value::Bool(b) => json!(b),
        Value::Null => serde_json::Value::Null,
    }
}
