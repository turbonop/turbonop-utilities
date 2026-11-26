//! What a comparison found: findings by category, each with a count and a
//! few examples, in plain words.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// How many examples each category keeps.
pub const EXAMPLES: usize = 5;

/// What a finding means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The two sides aren't equivalent.
    Failure,
    /// A difference that the rewrite is allowed to make (a folded
    /// function's PDB module, a PDB's signature).
    Expected,
    /// Something that wasn't checked, and why.
    Unchecked,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Severity::Failure => "FAILED",
            Severity::Expected => "expected",
            Severity::Unchecked => "not checked",
        }
    }
}

/// One category's findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub count: u64,
    pub examples: Vec<String>,
}

/// Findings by category, and what was checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    findings: BTreeMap<(Severity, String), Finding>,
    /// What was compared, as (what, how many), in the order first given.
    checked: Vec<(String, u64)>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a finding of `category`, with an example made only if the
    /// category has room for one.
    pub fn add(&mut self, severity: Severity, category: &str, example: impl FnOnce() -> String) {
        let f = self
            .findings
            .entry((severity, category.to_string()))
            .or_insert_with(|| Finding {
                severity,
                count: 0,
                examples: Vec::new(),
            });
        f.count += 1;
        if f.examples.len() < EXAMPLES {
            f.examples.push(example());
        }
    }

    /// Records a failure.
    pub fn fail(&mut self, category: &str, example: impl FnOnce() -> String) {
        self.add(Severity::Failure, category, example);
    }

    /// Records an expected difference.
    pub fn expect(&mut self, category: &str, example: impl FnOnce() -> String) {
        self.add(Severity::Expected, category, example);
    }

    /// Records something that wasn't checked.
    pub fn unchecked(&mut self, category: &str, example: impl FnOnce() -> String) {
        self.add(Severity::Unchecked, category, example);
    }

    /// Adds `n` to the count of what was checked under `what`.
    pub fn checked(&mut self, what: &str, n: u64) {
        match self.checked.iter_mut().find(|(w, _)| w == what) {
            Some((_, count)) => *count += n,
            None => self.checked.push((what.to_string(), n)),
        }
    }

    /// Adds another report's findings (examples in order, up to the
    /// limit).
    pub fn merge(&mut self, other: Report) {
        for ((severity, category), f) in other.findings {
            let mine = self
                .findings
                .entry((severity, category))
                .or_insert_with(|| Finding {
                    severity,
                    count: 0,
                    examples: Vec::new(),
                });
            mine.count += f.count;
            let room = EXAMPLES.saturating_sub(mine.examples.len());
            mine.examples.extend(f.examples.into_iter().take(room));
        }
        for (what, n) in other.checked {
            self.checked(&what, n);
        }
    }

    /// The number of failures, over every category.
    pub fn failures(&self) -> u64 {
        self.findings
            .values()
            .filter(|f| f.severity == Severity::Failure)
            .map(|f| f.count)
            .sum()
    }

    /// Whether nothing failed.
    pub fn passed(&self) -> bool {
        self.failures() == 0
    }

    /// The finding of `category`, if there is one.
    pub fn finding(&self, severity: Severity, category: &str) -> Option<&Finding> {
        self.findings.get(&(severity, category.to_string()))
    }

    /// Whether a failure of `category` was found.
    pub fn failed(&self, category: &str) -> bool {
        self.finding(Severity::Failure, category).is_some()
    }

    /// Every finding: severity, category, and the finding.
    pub fn findings(&self) -> impl Iterator<Item = (Severity, &str, &Finding)> + '_ {
        self.findings.iter().map(|((s, c), f)| (*s, c.as_str(), f))
    }

    /// The count checked under `what` (0 if nothing was).
    pub fn checked_count(&self, what: &str) -> u64 {
        self.checked
            .iter()
            .find(|(w, _)| w == what)
            .map_or(0, |(_, n)| *n)
    }

    /// The report as text: what was checked, then each category with its
    /// count and examples, failures first, then a verdict.
    pub fn text(&self) -> String {
        let mut out = String::new();
        if !self.checked.is_empty() {
            out.push_str("checked:\n");
            for (what, n) in &self.checked {
                let _ = writeln!(out, "  {n:>10}  {what}");
            }
        }
        for ((severity, category), f) in &self.findings {
            let _ = writeln!(out, "{}: {category}: {}", severity.name(), f.count);
            for e in &f.examples {
                let _ = writeln!(out, "    {e}");
            }
            if f.count > f.examples.len() as u64 {
                let _ = writeln!(out, "    ...");
            }
        }
        let failures = self.failures();
        if failures == 0 {
            out.push_str("equivalent: no failures\n");
        } else {
            let categories = self
                .findings
                .keys()
                .filter(|(s, _)| *s == Severity::Failure)
                .count();
            let _ = writeln!(
                out,
                "NOT equivalent: {failures} failures in {categories} categories"
            );
        }
        out
    }
}

/// Bytes as text: printable ASCII as it is, anything else escaped as
/// `\xNN`.
pub fn show(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x20..0x7F).contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out
}

#[cfg(test)]
mod tests;
