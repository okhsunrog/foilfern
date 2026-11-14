//! Port of `ProjectLoader::MigrationLog` of
//! libs/librepcb/core/project/projectloader.{h,cpp}.
//!
//! Differences to upstream: the application version shown in the footer is
//! passed in (upstream `Application::getVersion()`), see
//! [`ProjectLoader::set_application_version()`](super::ProjectLoader::set_application_version).

use chrono::{DateTime, Local};
use foilfern_i18n::tr;

use crate::application;
use crate::serialization::{MigrationMessage, MigrationSeverity};
use crate::types::Version;

const HTML_LOG_HEADER: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>{{TITLE}}</title>
  <style>
    body { font-family: Arial, sans-serif; margin: 20px; background: #f5f5f5; }
    table { border-collapse: collapse; width: 100%; background: #fff; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }
    th, td { padding: 12px; border-bottom: 1px solid #ddd; width: auto; white-space: nowrap; text-align: center; vertical-align: middle; }
    th { background: #e0e0e0; font-weight: bold; }
    th:nth-child(3), td:nth-child(4) { text-align: left; width: 100%; white-space: normal; }
    .footer { font-size: 10pt; font-style: italic; color: #505050; }
    .note { background: #e6f3ff; }
    .warning { background: #fff3cd; }
    .critical { background: #f8d7da; }
    .note td:nth-child(3)::before { content: "ℹ️"; }
    .warning td:nth-child(3)::before { content: "⚠️"; }
    .critical td:nth-child(3)::before { content: "❌"; }
  </style>
</head>
<body>
  <h1>🚀 {{TITLE}}</h1>
"#;

const HTML_LOG_TABLE_HEADER: &str = r#"  <table>
    <thead>
      <tr>
        <th>{{HEADER_VERSION}}</th>
        <th>{{HEADER_OCCURRENCES}}</th>
        <th colspan="2">{{HEADER_MESSAGE}}</th>
      </tr>
    </thead>
    <tbody>
"#;

const HTML_LOG_FOOTER: &str = r#"    </tbody>
  </table>
  <p class="footer">{{GENERATED_AT}}</p>
</body>
</html>"#;

/// The log of a project file format migration (upstream
/// `ProjectLoader::MigrationLog`), saved as HTML file in the project's
/// `logs` directory.
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationLog {
    /// Name of the project file (e.g. `"project.lpp"`).
    pub project_name: String,
    /// When the migration was performed.
    pub date_time: DateTime<Local>,
    /// File format version of the project before the migration.
    pub from_version: Version,
    /// File format version of the project after the migration.
    pub to_version: Version,
    /// The messages emitted by the migrations, sorted (see
    /// [`sort_messages()`](Self::sort_messages)).
    pub messages: Vec<MigrationMessage>,
}

impl MigrationLog {
    /// Sorts the messages like upstream: oldest file format step first,
    /// then by descending severity, then by message.
    pub fn sort_messages(&mut self) {
        // It is most intuitive to have the oldest messages at top, and the
        // newest messages at bottom since they might "depend" on each other.
        self.messages.sort_by(|a, b| {
            a.to_version
                .cmp(&b.to_version)
                .then_with(|| b.severity.cmp(&a.severity))
                .then_with(|| a.message.cmp(&b.message))
        });
    }

    /// Returns the path of the log file relative to the project directory.
    ///
    /// The temporary log (shown before the project is saved) has a
    /// deterministic name, not containing the date or file format, so a
    /// later LibrePCB version still finds and deletes it.
    pub fn relative_file_path(&self, is_temporary: bool) -> String {
        // Important: Don't store the HTML in /tmp or ~/.cache because if
        // LibrePCB runs in a sandbox, other apps may not have access to read
        // those directories. So we store the HTML in the project directory
        // instead, but schedule it for deletion once the project is saved
        // (leading to a new, final migration log).
        if is_temporary {
            "logs/migration_preview.html".to_owned()
        } else {
            format!(
                "logs/{}_migration_to_v{}.html",
                self.date_time.format("%Y-%m-%d"),
                application::file_format_version()
            )
        }
    }

    /// Returns the log as HTML document; `app_version` is the version of
    /// the application shown in the footer.
    pub fn to_html(&self, is_temporary: bool, app_version: &str) -> String {
        let ctx = "librepcb::ProjectLoader";
        let mut html = HTML_LOG_HEADER.to_owned();
        if is_temporary {
            html += "  <div style=\"background:#d0d0d0;border: 1px solid \
                     black;padding:8px;margin-bottom:15px;\">\n";
            html += "    <span style=\"color:blue;\">";
            html += &escape(&tr!(
                ctx,
                "Note: This is a temporary file since the project has not been saved to \
                 disk yet. When you save the project, the final log file will be persisted \
                 in the projects '{0}' directory.",
                "logs"
            ));
            html += "</span></br>\n";
            html += "    <span style=\"color:red;font-weight:bold;\">";
            html += &escape(&tr!(
                ctx,
                "Attention: After saving the project, the file format migration cannot be \
                 reverted. Check if everything looks good before saving the project."
            ));
            html += "</span>\n";
            html += "  </div>\n";
        }
        html += "  <p>";
        html += &tr!(
            ctx,
            "The project has been migrated from file format <strong>{0} to {1}</strong>, \
             which emitted the following messages:",
            format!("v{}", self.from_version),
            format!("v{}", self.to_version)
        );
        html += "</p>\n";
        html += HTML_LOG_TABLE_HEADER;
        if self.messages.is_empty() {
            html += "      <tr>\n";
            html += &format!(
                "        <td colspan=\"4\" style=\"text-align:left;\">{}</td>\n",
                escape(&tr!(ctx, "No messages emitted."))
            );
            html += "      </tr>\n";
        }
        for m in &self.messages {
            let cells = [
                format!("{} ⇨ {}", m.from_version, m.to_version),
                m.affected_items
                    .filter(|&n| n > 0)
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                String::new(), // Severity icon from CSS
                m.message.clone(),
            ];
            let class = match m.severity {
                MigrationSeverity::Note => "note",
                MigrationSeverity::Warning => "warning",
                MigrationSeverity::Critical => "critical",
            };
            html += &format!("      <tr class=\"{class}\">\n");
            for cell in &cells {
                html += &format!("        <td>{}</td>\n", escape(cell));
            }
            html += "      </tr>\n";
        }
        html += HTML_LOG_FOOTER;
        let generated_at = format!(
            "Generated by LibrePCB {app_version} at {}, {}",
            // Like Qt's `QDate::toString()` and `QTime::toString()`.
            self.date_time.format("%a %b %-d %Y"),
            self.date_time.format("%H:%M:%S")
        );
        html.replace("{{HEADER_VERSION}}", &tr!(ctx, "Version"))
            .replace("{{HEADER_OCCURRENCES}}", &tr!(ctx, "Occurrences"))
            .replace("{{HEADER_MESSAGE}}", &tr!(ctx, "Message"))
            .replace(
                "{{TITLE}}",
                &escape(&format!("Migration Log: {}", self.project_name)),
            )
            .replace("{{GENERATED_AT}}", &escape(&generated_at))
    }
}

/// Escapes `<`, `>`, `&` and `"` like `QString::toHtmlEscaped()`.
fn escape(s: &str) -> String {
    html_escape::encode_double_quoted_attribute(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_qt() {
        assert_eq!(escape(r#"a<b>&"c"'d'"#), "a&lt;b&gt;&amp;&quot;c&quot;'d'");
    }
}
