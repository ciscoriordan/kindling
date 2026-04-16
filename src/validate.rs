// Kindle Publishing Guidelines validator entry point.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::checks;
use crate::extracted::ExtractedEpub;
use crate::kdp_rules;
use crate::kdp_rules::Severity;

/// Severity of a validation finding. Alias kept for the existing test/call
/// sites during the Phase 0 refactor.
pub type Level = Severity;

/// A single finding from a validation check.
#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub rule_id: Option<&'static str>,
    pub section: String,
    pub message: String,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rule_id {
            Some(id) => {
                let rule = kdp_rules::get(id);
                write!(
                    f,
                    "[{} {}] section {} (p.{}): {}",
                    self.level, id, self.section, rule.pdf_page, self.message
                )?;
            }
            None => {
                write!(f, "[{}] section {}: {}", self.level, self.section, self.message)?;
            }
        }
        if let Some(ref file) = self.file {
            write!(f, " ({}", file.display())?;
            if let Some(line) = self.line {
                write!(f, ":{}", line)?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

/// Full report from `validate` / `validate_opf`.
#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    pub findings: Vec<Finding>,
}

impl ValidationReport {
    pub fn new() -> Self {
        ValidationReport { findings: Vec::new() }
    }

    pub fn push(&mut self, f: Finding) {
        self.findings.push(f);
    }

    /// Emit a finding for `rule_id` from the KDP rules catalog.
    pub fn emit(&mut self, rule_id: &'static str, context: impl Into<String>) {
        let rule = kdp_rules::get(rule_id);
        let context = context.into();
        let message = if context.is_empty() {
            rule.description.to_string()
        } else {
            format!("{} {}", rule.description, context)
        };
        self.push(Finding {
            level: rule.level,
            rule_id: Some(rule_id),
            section: rule.section.to_string(),
            message,
            file: None,
            line: None,
        });
    }

    /// Emit a finding for `rule_id` with file/line location.
    pub fn emit_at(
        &mut self,
        rule_id: &'static str,
        context: impl Into<String>,
        file: Option<PathBuf>,
        line: Option<usize>,
    ) {
        let rule = kdp_rules::get(rule_id);
        let context = context.into();
        let message = if context.is_empty() {
            rule.description.to_string()
        } else {
            format!("{} {}", rule.description, context)
        };
        self.push(Finding {
            level: rule.level,
            rule_id: Some(rule_id),
            section: rule.section.to_string(),
            message,
            file,
            line,
        });
    }

    pub fn error_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Warning).count()
    }

    pub fn info_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Info).count()
    }
}

/// Run every registered `Check` against `epub` and return the collected report.
pub fn validate(epub: &ExtractedEpub) -> ValidationReport {
    let mut report = ValidationReport::new();
    report.emit("R4.1.1", "");
    for check in checks::CHECKS {
        check.run(epub, &mut report);
    }
    let profile = epub.profile;
    report.findings.retain(|f| {
        f.rule_id
            .map_or(true, |id| kdp_rules::get(id).applies_to(profile))
    });
    report
}

/// Thin wrapper that parses the OPF then calls `validate`.
pub fn validate_opf(opf_path: &Path) -> Result<ValidationReport, Box<dyn std::error::Error>> {
    let epub = ExtractedEpub::from_opf_path(opf_path)?;
    Ok(validate(&epub))
}
