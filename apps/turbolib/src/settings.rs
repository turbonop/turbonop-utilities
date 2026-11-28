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
//! | `precomp` | `none`, `library`, `clustered`, `clustered=<n>` |
//! | `debug-types` | `complete`, `shared` (Mach-O synthesized members' classes) |
//! | `report` | `odr`: list the names whose definitions differ |
//! | `threads` (or `j`) | a number of threads, at least 1 |
//! | `diagnose` | say what was passed through, and why |
//! | `stats` | report what was done, when the library is written |
//! | `extract-whole` | `/EXTRACT` re-inlines folded units |
//! | `verify` | `on` (or no value), `off`: check the folded library before it replaces the output |
//! | `strict` | a fold that fails is an error, not the unfolded library (for testing) |
//! | `inject-failure` | `panic`, `verify`, `layout`: make folding fail, to test the fallback |
//!
//! Folding never breaks a build: if it panics, if what it made can't be
//! laid out, or (under `verify`) if the check finds the folded library not
//! equivalent, the library is written unfolded, as `lib.exe` writes it,
//! with one warning line saying why (`strict` makes that an error).
//!
//! The default folding is still `none` (`identical` is the plan's default,
//! once the build systems using turbolib choose it). `pick` matters only
//! with `select-any`, and `locals` only when something folds.

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

/// `debug-types`: how a Mach-O synthesized member describes the classes
/// its moved code names (see the plan's 7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugTypes {
    /// Whole, so that a dSYM has every class definition the original's
    /// has, as the original's link kept it (the default).
    Complete,
    /// As declarations where every giver defines them in every link that
    /// loads it: smaller, but which unit's version of a class the dSYM has
    /// may change.
    Shared,
}

/// `precomp`: shared types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precomp {
    None,
    Library,
    /// Up to `n` precompiled-types members (`None`: as many as the cost
    /// model chooses, up to 16).
    Clustered(Option<u32>),
}

/// turbolib's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub fold: Fold,
    pub pick: Pick,
    pub pull: Pull,
    pub locals: Locals,
    pub precomp: Precomp,
    pub debug_types: DebugTypes,
    pub threads: Option<usize>,
    pub diagnose: bool,
    pub stats: bool,
    pub extract_whole: bool,
    /// `report=odr`.
    pub report_odr: bool,
    /// Check the folded library (`turbonop_verify`) before it is put in
    /// place, and write the unfolded one if the check fails.
    pub verify: bool,
    /// A failed fold is an error rather than the unfolded library.
    pub strict: bool,
    /// A failure to make on purpose, to test the fallback.
    pub inject: Option<Inject>,
}

/// `inject-failure`: where folding is made to fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inject {
    /// Folding panics (on one of the worker threads, when there are any).
    Panic,
    /// The self-check reports a failure.
    Verify,
    /// Laying out the folded library fails.
    Layout,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            fold: Fold::None,
            pick: Pick::First,
            pull: Pull::Preserve,
            locals: Locals::Keep,
            precomp: Precomp::None,
            debug_types: DebugTypes::Complete,
            threads: None,
            diagnose: false,
            stats: false,
            extract_whole: false,
            report_odr: false,
            verify: true,
            strict: false,
            inject: None,
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
            ("debug-types", Some("complete")) => self.debug_types = DebugTypes::Complete,
            ("debug-types", Some("shared")) => self.debug_types = DebugTypes::Shared,
            ("precomp", Some("none")) => self.precomp = Precomp::None,
            ("precomp", Some("library")) => self.precomp = Precomp::Library,
            ("precomp", Some("clustered")) => self.precomp = Precomp::Clustered(None),
            ("precomp", Some(v)) if v.starts_with("clustered=") => {
                let n: u32 = v["clustered=".len()..]
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(bad)?;
                self.precomp = Precomp::Clustered(Some(n));
            }
            ("threads" | "j", Some(v)) => {
                let n: usize = v.parse().ok().filter(|&n| n > 0).ok_or_else(bad)?;
                self.threads = Some(n);
            }
            ("diagnose", None) => self.diagnose = true,
            ("stats", None) => self.stats = true,
            ("extract-whole", None) => self.extract_whole = true,
            ("report", Some("odr")) => self.report_odr = true,
            ("verify", None | Some("on")) => self.verify = true,
            ("verify", Some("off")) => self.verify = false,
            ("strict", None) => self.strict = true,
            ("inject-failure", Some("panic")) => self.inject = Some(Inject::Panic),
            ("inject-failure", Some("verify")) => self.inject = Some(Inject::Verify),
            ("inject-failure", Some("layout")) => self.inject = Some(Inject::Layout),
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

    /// The modes asked for that turbolib can't do yet, as an error (none,
    /// now).
    pub fn check_supported(&self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
