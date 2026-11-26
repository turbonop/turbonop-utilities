//! turbolib's own options, which every personality takes in its own
//! spelling: `/TURBOLIB:<option>` for `lib`, `--<option>` elsewhere, and in
//! the `TURBOLIB_OPTIONS` environment variable (`--<option>` words, read
//! before the command line's, so the command line wins), for build systems
//! that can't add arguments.
//!
//! An option is `name` or `name=value`, in any case:
//!
//! | Option | Values |
//! |---|---|
//! | `fold` | `none`, `identical`, `select-any` |
//! | `pick` | `first`, `largest` (with `select-any`) |
//! | `pull` | `preserve`, `relaxed` |
//! | `locals` | `keep`, `copy`, `merge` |
//! | `precomp` | `none`, `library`, `clustered=<n>` |
//! | `threads` (or `j`) | a number of threads, at least 1 |
//! | `diagnose` | say what was passed through, and why |
//! | `stats` | report what was done, when the library is written |
//! | `extract-whole` | `/EXTRACT` re-inlines folded units |
//!
//! Until phase 2 ships, the only folding is `none`, which is the default;
//! the plan's default, `identical`, takes its place then. Asking for a mode
//! that isn't written yet is an error, not a silent `none`.

/// `fold`: which copies count as the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    None,
    Identical,
    SelectAny,
}

/// `pick`: which copy survives under `select-any`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    First,
    Largest,
}

/// `pull`: whether which members a link loads may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    Preserve,
    Relaxed,
}

/// `locals`: units that reach internal-linkage symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locals {
    Keep,
    Copy,
    Merge,
}

/// `precomp`: shared types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precomp {
    None,
    Library,
    Clustered(u32),
}

/// turbolib's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub fold: Fold,
    pub pick: Pick,
    pub pull: Pull,
    pub locals: Locals,
    pub precomp: Precomp,
    pub threads: Option<usize>,
    pub diagnose: bool,
    pub stats: bool,
    pub extract_whole: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            fold: Fold::None,
            pick: Pick::First,
            pull: Pull::Preserve,
            locals: Locals::Keep,
            precomp: Precomp::None,
            threads: None,
            diagnose: false,
            stats: false,
            extract_whole: false,
        }
    }
}

impl Settings {
    /// Applies one option, `name` or `name=value`.
    pub fn apply(&mut self, option: &str) -> Result<(), String> {
        let (name, value) = match option.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (option, None),
        };
        let lower = value.map(str::to_ascii_lowercase);
        let bad = || format!("{option}: not one of turbolib's options");
        match (name.to_ascii_lowercase().as_str(), lower.as_deref()) {
            ("fold", Some("none")) => self.fold = Fold::None,
            ("fold", Some("identical")) => self.fold = Fold::Identical,
            ("fold", Some("select-any")) => self.fold = Fold::SelectAny,
            ("pick", Some("first")) => self.pick = Pick::First,
            ("pick", Some("largest")) => self.pick = Pick::Largest,
            ("pull", Some("preserve")) => self.pull = Pull::Preserve,
            ("pull", Some("relaxed")) => self.pull = Pull::Relaxed,
            ("locals", Some("keep")) => self.locals = Locals::Keep,
            ("locals", Some("copy")) => self.locals = Locals::Copy,
            ("locals", Some("merge")) => self.locals = Locals::Merge,
            ("precomp", Some("none")) => self.precomp = Precomp::None,
            ("precomp", Some("library")) => self.precomp = Precomp::Library,
            ("precomp", Some(v)) if v.starts_with("clustered=") => {
                let n: u32 = v["clustered=".len()..]
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(bad)?;
                self.precomp = Precomp::Clustered(n);
            }
            ("threads" | "j", Some(v)) => {
                let n: usize = v.parse().ok().filter(|&n| n > 0).ok_or_else(bad)?;
                self.threads = Some(n);
            }
            ("diagnose", None) => self.diagnose = true,
            ("stats", None) => self.stats = true,
            ("extract-whole", None) => self.extract_whole = true,
            _ => return Err(bad()),
        }
        Ok(())
    }

    /// Applies `TURBOLIB_OPTIONS`'s words: `--name[=value]`, and `-j N` or
    /// `-jN`.
    pub fn apply_env(&mut self, text: &str) -> Result<(), String> {
        let mut words = text.split_whitespace();
        while let Some(w) = words.next() {
            if w == "-j" {
                let n = words.next().ok_or("-j needs a number")?;
                self.apply(&format!("threads={n}"))?;
            } else if let Some(n) = w.strip_prefix("-j") {
                self.apply(&format!("threads={n}"))?;
            } else if let Some(o) = w.strip_prefix("--") {
                self.apply(o)?;
            } else {
                return Err(format!("{w}: options start with --"));
            }
        }
        Ok(())
    }

    /// The modes asked for that turbolib can't do yet, as an error.
    pub fn check_supported(&self) -> Result<(), String> {
        if self.fold == Fold::SelectAny {
            return Err("fold=select-any isn't written yet (phase 4)".into());
        }
        if self.locals != Locals::Keep {
            return Err("only locals=keep is written so far (phase 4)".into());
        }
        if self.precomp != Precomp::None {
            return Err(
                "only precomp=none is written so far (shared types come in phase 3)".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
