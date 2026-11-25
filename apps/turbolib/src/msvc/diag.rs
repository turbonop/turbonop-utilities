//! Warnings and errors, worded and numbered as `lib.exe` and `llvm-lib`
//! word them, so that build logs and scripts that look for them still work.
//!
//! `lib.exe`'s messages read `<where> : <kind> LNK<number>: <text>`, where
//! `<where>` is `LINK` or the file concerned. Its exit code is the number
//! of the error that stopped it, or 0. Under `/WX`, the first warning
//! makes it stop with `LNK1218: warning treated as error; no output file
//! generated`, and its exit code is that warning's number (measured:
//! `/WX` with LNK4042 exits with 4042). `/IGNORE:<n>` silences warning n.
//!
//! `llvm-lib`'s messages are plain lines, and it exits with 1 on any error.

use super::args::Flavor;
use std::fmt;

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Warning,
    /// An error after which `lib.exe` goes on (to report more), but writes
    /// nothing.
    Error,
    /// An error that stops at once.
    Fatal,
}

/// One diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub kind: Kind,
    /// `lib.exe`'s number (LNK*n*); 0 for messages only `llvm-lib` gives.
    pub code: u32,
    /// The file it concerns; `None` for `LINK`.
    pub file: Option<String>,
    pub text: String,
}

impl Diag {
    pub fn warning(code: u32, text: impl Into<String>) -> Diag {
        Diag {
            kind: Kind::Warning,
            code,
            file: None,
            text: text.into(),
        }
    }

    pub fn fatal(code: u32, text: impl Into<String>) -> Diag {
        Diag {
            kind: Kind::Fatal,
            code,
            file: None,
            text: text.into(),
        }
    }

    /// An `llvm-lib` message, which has no number.
    pub fn llvm(kind: Kind, text: impl Into<String>) -> Diag {
        Diag {
            kind,
            code: 0,
            file: None,
            text: text.into(),
        }
    }

    pub fn on(mut self, file: impl Into<String>) -> Diag {
        self.file = Some(file.into());
        self
    }

    /// The message as the tool prints it.
    pub fn render(&self, flavor: Flavor) -> String {
        match flavor {
            Flavor::Lib => {
                let kind = match self.kind {
                    Kind::Warning => "warning",
                    Kind::Error => "error",
                    Kind::Fatal => "fatal error",
                };
                let at = self.file.as_deref().unwrap_or("LINK");
                format!("{at} : {kind} LNK{}: {}", self.code, self.text)
            }
            Flavor::LlvmLib => match &self.file {
                Some(f) => format!("{f}: {}", self.text),
                None => self.text.clone(),
            },
        }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Flavor::Lib))
    }
}

/// Which warnings are silenced or made errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WarningPolicy {
    /// `/WX`: every warning is an error.
    pub all_errors: bool,
    /// `/WX:n,...`: these warnings are errors.
    pub errors: Vec<u32>,
    /// `/IGNORE:n,...`: these warnings aren't shown.
    pub ignored: Vec<u32>,
}

/// Collects diagnostics, applying the warning policy, and says how the run
/// ends.
#[derive(Debug)]
pub struct Reporter {
    pub flavor: Flavor,
    pub policy: WarningPolicy,
    /// What would be printed, in order.
    pub lines: Vec<String>,
    /// The exit code so far.
    pub exit: i32,
    /// Whether output may still be written.
    pub write_output: bool,
}

impl Reporter {
    pub fn new(flavor: Flavor, policy: WarningPolicy) -> Reporter {
        Reporter {
            flavor,
            policy,
            lines: Vec::new(),
            exit: 0,
            write_output: true,
        }
    }

    /// Reports `d`. Returns `Err(())` if the run must stop now: a fatal
    /// error, or a warning `/WX` makes one.
    #[allow(clippy::result_unit_err)]
    pub fn report(&mut self, d: Diag) -> Result<(), ()> {
        match d.kind {
            Kind::Warning => {
                if self.flavor == Flavor::Lib && self.policy.ignored.contains(&d.code) {
                    return Ok(());
                }
                self.lines.push(d.render(self.flavor));
                let as_error = self.policy.all_errors || self.policy.errors.contains(&d.code);
                if as_error {
                    match self.flavor {
                        Flavor::Lib => {
                            self.lines.push(
                                Diag {
                                    kind: Kind::Error,
                                    code: 1218,
                                    file: None,
                                    text: "warning treated as error; no output file generated"
                                        .into(),
                                }
                                .render(Flavor::Lib),
                            );
                            self.exit = d.code as i32;
                        }
                        Flavor::LlvmLib => {
                            self.lines
                                .push("treating warning as error due to /WX".into());
                            self.exit = 1;
                        }
                    }
                    self.write_output = false;
                    return Err(());
                }
                Ok(())
            }
            Kind::Error => {
                self.lines.push(d.render(self.flavor));
                if self.exit == 0 {
                    self.exit = self.code_of(&d);
                }
                self.write_output = false;
                Ok(())
            }
            Kind::Fatal => {
                self.lines.push(d.render(self.flavor));
                self.exit = self.code_of(&d);
                self.write_output = false;
                Err(())
            }
        }
    }

    fn code_of(&self, d: &Diag) -> i32 {
        match self.flavor {
            Flavor::Lib => d.code as i32,
            Flavor::LlvmLib => 1,
        }
    }
}

#[cfg(test)]
mod tests;
