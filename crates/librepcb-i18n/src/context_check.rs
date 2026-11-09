//! Checks that the translation contexts and source strings of all `tr!` and
//! `trn!` calls in the workspace exist in the catalog template
//! (`lang/librepcb.pot`, extracted from upstream), so that the upstream
//! translations are found at runtime. Contexts are the full upstream Qt
//! contexts, e.g. `librepcb::editor::CmdBoardEdit` for classes in the
//! `librepcb::editor` namespace.
//!
//! The scanner is deliberately simple: it finds `tr!(`/`trn!(` outside of
//! line comments, reads the context (a string literal, or a constant or
//! `let` binding declared before the call in the same file, or a constant of
//! the same crate) and the source string (if it is a literal). The values of
//! a binding are the string literals of its initializer and the values of
//! the constants (`UPPER_CASE` names) it mentions, e.g.
//! `let ctx = if bus { RENAME_BUS } else { RENAME_NET };` has both
//! contexts, and each of them must contain the string. A context which is a
//! parameter of the enclosing function has the values of the corresponding
//! argument of all calls of the function in the crate (resolved the same
//! way). Strings without upstream counterpart are listed in
//! [`NEW_CONTEXTS`] and [`NEW_STRINGS`].
//!
//! Not scanned: this crate (its tests use made-up contexts). The `@tr()`
//! strings of the `.slint` files are not checked here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::compile::catalog_keys;

/// Contexts of Rust-only features without upstream counterpart.
const NEW_CONTEXTS: &[&str] = &[
    // Built-in autorouter (no upstream counterpart).
    "Autorouter",
    // Qt's own strings (not in LibrePCB's catalogs): the command line
    // parser (see COMPAT.md "CLI") and the standard dialog buttons.
    "QCommandLineParser",
    "QDialogButtonBox",
];

/// Strings without upstream counterpart: `(context, source string)`.
const NEW_STRINGS: &[(&str, &str)] = &[
    // MCP placement and schematic tidy commands.
    ("CmdMoveSelectedSchematicItems", "Move Schematic Items"),
    // Read-only element editors refusing changes.
    (
        "librepcb::editor::LibraryEditorTab",
        "The library element is read-only.",
    ),
    // App: Features of the application which are not available yet.
    (
        "librepcb::editor::MainWindow",
        "Not available yet in this version: {0}",
    ),
    ("librepcb::editor::MainWindow", "Not Available Yet"),
    (
        "librepcb::editor::MainWindow",
        "This dialog is not available yet in this version of LibrePCB.",
    ),
    // App: Embedded MCP server (no upstream counterpart).
    ("librepcb::editor::MainWindow", "MCP server running at {0}"),
    ("librepcb::editor::MainWindow", "MCP Server"),
    ("librepcb::editor::MainWindow", "AI agent: {0}"),
    ("librepcb::editor::MainWindow", "AI Agent Connected"),
    (
        "librepcb::editor::MainWindow",
        "An AI agent is editing the project \"{0}\" through the MCP server. Its changes can be undone like your own.",
    ),
    // App: Project lifecycle and library management (COMPAT.md "app").
    (
        "librepcb::editor::LibrariesModel",
        "Failed to Install Library",
    ),
    ("librepcb::editor::GuiApplication", "Read-Only Mode"),
    (
        "librepcb::editor::DirectoryLockHandlerDialog",
        "Open Read-Only",
    ),
    (
        "librepcb::editor::ProjectEditor",
        "The project contains unsaved changes. Please save it first.",
    ),
    (
        "librepcb::editor::InitializeWorkspaceWizard_Upgrade",
        "The workspace data directory will be copied to a new directory for the current file format. The old directory is kept, so older LibrePCB versions can still use it.",
    ),
    // App: Form dialogs (COMPAT.md "app", dialogs).
    (
        "librepcb::editor::AddComponentDialog",
        "Failed to open the component.",
    ),
    ("librepcb::editor::PathEditorWidget", "Vertices"),
    (
        "librepcb::editor::SymbolInstancePropertiesDialog",
        "Click OK again to swap the names.",
    ),
    (
        "librepcb::editor::OutputJobsDialog",
        "Click on the {0} button below to add output jobs. Or just close this dialog to not generate any output files.",
    ),
    (
        "librepcb::editor::OutputJobHomeWidget",
        "This output job type is not supported by this version of LibrePCB.",
    ),
    ("librepcb::editor::DeviceTab", "Component not found: {0}"),
    ("librepcb::editor::DeviceTab", "Package not found: {0}"),
    // App: Workspace settings: text shortcut editor and API server list.
    ("librepcb::editor::WorkspaceSettingsDialog", "Command"),
    ("librepcb::editor::WorkspaceSettingsDialog", "Shortcuts"),
    ("librepcb::editor::WorkspaceSettingsDialog", "Shortcuts:"),
    ("librepcb::editor::WorkspaceSettingsDialog", "No Shortcut"),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "One key sequence per line, e.g. \"Ctrl+Shift+S\"",
    ),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "Invalid key sequence: {0}",
    ),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "API servers provide online libraries, live part information and the PCB ordering service.",
    ),
    ("librepcb::editor::WorkspaceSettingsDialog", "API Servers:"),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "Use for libraries",
    ),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "Use for parts information",
    ),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "Use for PCB ordering",
    ),
    (
        "librepcb::editor::WorkspaceSettingsDialog",
        "Allow the editors to automatically display live information about parts (lifecycle status, stock availability, price, ...) by requesting it from the configured API endpoints.",
    ),
    // App: Outputs from the menus and undoable grid changes.
    (
        "librepcb::editor::OutputJobsDialog",
        "Success! Generated files:",
    ),
    ("librepcb::editor::MainWindow", "{0}: {1} file(s) written"),
    ("librepcb::editor::SchematicTab", "Change Grid Properties"),
    ("librepcb::editor::Board2dTab", "Change Grid Properties"),
];

/// Directories (relative to the workspace root) which are not scanned.
const EXCLUDED: &[&str] = &["crates/librepcb-i18n"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Parses a (possibly raw) string literal starting at `pos`; returns its
/// value and the position behind it.
fn string_literal(src: &str, pos: usize) -> Option<(String, usize)> {
    let rest = &src[pos..];
    if let Some(raw) = rest.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let body = raw[hashes..].strip_prefix('"')?;
        let end_marker = format!("\"{}", "#".repeat(hashes));
        let end = body.find(&end_marker)?;
        let consumed = 1 + hashes + 1 + end + end_marker.len();
        return Some((body[..end].to_owned(), pos + consumed));
    }
    let body = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, pos + 1 + i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                'u' => {
                    let mut hex = String::new();
                    for (_, c) in chars.by_ref() {
                        match c {
                            '{' => {}
                            '}' => break,
                            c => hex.push(c),
                        }
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                '\n' => {
                    // Line continuation: skip the leading whitespace of the
                    // next line.
                    let skip = chars.clone().take_while(|(_, c)| c.is_whitespace()).count();
                    for _ in 0..skip {
                        chars.next();
                    }
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn skip_whitespace(src: &str, pos: usize) -> usize {
    pos + (src[pos..].len() - src[pos..].trim_start().len())
}

/// Parses an identifier or path (`a::B`) at `pos`.
fn identifier(src: &str, pos: usize, path: bool) -> Option<(&str, usize)> {
    let len = src[pos..]
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || (path && c == ':')))
        .unwrap_or(src.len() - pos);
    (len > 0).then(|| (&src[pos..pos + len], pos + len))
}

/// Returns the values of the initializer of `const NAME: &str =` or
/// `let NAME =` declared last before `before`: its string literals and the
/// values of the constants it names (declared before in the same file, or
/// in `crate_consts`). `None` if there is no such declaration or it has no
/// values.
fn local_binding(
    src: &str,
    name: &str,
    before: usize,
    crate_consts: &[(String, String)],
) -> Option<Vec<String>> {
    let mut found = None;
    for prefix in ["const ", "let "] {
        let pattern = format!("{prefix}{name}");
        let mut start = 0;
        while let Some(i) = src[start..before].find(&pattern) {
            let at = start + i;
            start = at + pattern.len();
            let after = &src[start..];
            let next = after.chars().next();
            if next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if at > 0 && src[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let Some(eq) = after.find('=') else { continue };
            let Some(semi) = after.find(';') else {
                continue;
            };
            if semi < eq {
                continue;
            }
            if found.is_none_or(|(f, _)| f < at) {
                found = Some((at, (start + eq + 1, start + semi)));
            }
        }
    }
    let (at, (init_start, init_end)) = found?;
    let mut values = Vec::new();
    // The initializer without its string literals, for the constants.
    let mut code = String::new();
    let mut pos = init_start;
    while let Some(i) = src[pos..init_end].find('"') {
        code.push_str(&src[pos..pos + i]);
        let (value, end) = string_literal(src, pos + i)?;
        values.push(value);
        pos = end;
    }
    code.push_str(&src[pos..init_end]);
    for ident in code.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let is_const = ident.len() > 1
            && ident.starts_with(|c: char| c.is_ascii_uppercase())
            && ident
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !is_const || ident == name {
            continue;
        }
        match local_binding(src, ident, at, crate_consts) {
            Some(v) => values.extend(v),
            None => values.extend(
                crate_consts
                    .iter()
                    .filter(|(n, _)| n == ident)
                    .map(|(_, v)| v.clone()),
            ),
        }
    }
    values.dedup();
    (!values.is_empty()).then_some(values)
}

/// The sources and string constants of a crate.
struct CrateSources {
    /// `(name, value)` of all `const NAME: &str = "value";`.
    consts: Vec<(String, String)>,
    /// The content of all files.
    sources: Vec<String>,
}

/// The top-level comma separated parts between the parenthesis at `open`
/// and its match, with their offsets (`angle`: `<>` nest too, for
/// parameter lists).
fn arguments(src: &str, open: usize, angle: bool) -> Option<Vec<(usize, &str)>> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut part_start = open + 1;
    let mut i = open + 1;
    while i < src.len() {
        let c = src[i..].chars().next()?;
        match c {
            '"' => {
                i = string_literal(src, i)?.1;
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            '<' if angle => depth += 1,
            '>' if angle && !src[..i].ends_with('-') => depth = depth.checked_sub(1)?,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            ')' => {
                if !src[part_start..i].trim().is_empty() {
                    parts.push((part_start, &src[part_start..i]));
                }
                return Some(parts);
            }
            ',' if depth == 0 => {
                parts.push((part_start, &src[part_start..i]));
                part_start = i + 1;
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Resolves an identifier used as context before `at`: a binding or
/// constant (see [`local_binding()`]), a constant of the crate, or a
/// parameter of the enclosing function (the arguments of its calls,
/// `depth` levels deep).
fn resolve_identifier(
    src: &str,
    name: &str,
    at: usize,
    krate: &CrateSources,
    depth: usize,
) -> Option<Vec<String>> {
    if let Some(values) = local_binding(src, name, at, &krate.consts) {
        return Some(values);
    }
    let values: Vec<_> = krate
        .consts
        .iter()
        .filter(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .collect();
    if !values.is_empty() {
        return Some(values);
    }
    if depth == 0 {
        return None;
    }
    // The enclosing function and the index of the parameter.
    let fn_at = src[..at]
        .rmatch_indices("fn ")
        .map(|(i, _)| i)
        .find(|&i| i == 0 || !src[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))?;
    let (fn_name, end) = identifier(src, fn_at + 3, false)?;
    let open = end + src[end..].find('(')?;
    let params = arguments(src, open, true)?;
    let is_method = params.first().is_some_and(|(_, p)| p.contains("self"));
    let index = params.iter().position(|(_, p)| {
        let p = p.trim();
        let p = p.strip_prefix("mut ").unwrap_or(p);
        p.strip_prefix(name)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })?;
    let index = if is_method {
        index.checked_sub(1)?
    } else {
        index
    };
    let mut values = Vec::new();
    let mut calls = 0;
    for file in &krate.sources {
        let pattern = format!("{fn_name}(");
        for (call_at, _) in file.match_indices(&pattern) {
            let before = &file[..call_at];
            if before.ends_with(|c: char| c.is_alphanumeric() || c == '_')
                || before.ends_with("fn ")
                || before.ends_with('.') != is_method
            {
                continue;
            }
            let args = arguments(file, call_at + fn_name.len(), false)?;
            let (arg_at, arg) = *args.get(index)?;
            let arg_at = arg_at + (arg.len() - arg.trim_start().len());
            let arg = arg.trim().trim_start_matches('&');
            if let Some((value, _)) = string_literal(arg, 0) {
                values.push(value);
            } else {
                let (ident, _) = identifier(arg, 0, true)?;
                let ident = ident.rsplit("::").next().unwrap_or(ident);
                values.extend(resolve_identifier(file, ident, arg_at, krate, depth - 1)?);
            }
            calls += 1;
        }
    }
    values.sort();
    values.dedup();
    (calls > 0).then_some(values)
}

/// A `tr!`/`trn!` call found by the scanner.
struct Call {
    location: String,
    /// Possible contexts, or the unresolved expression.
    contexts: Result<Vec<String>, String>,
    msgid: Option<String>,
}

fn scan_file(path: &Path, krate: &CrateSources, calls: &mut Vec<Call>) {
    let src = std::fs::read_to_string(path).expect("read source file");
    let display = path
        .strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string();
    for macro_name in ["tr!(", "trn!("] {
        let mut start = 0;
        while let Some(i) = src[start..].find(macro_name) {
            let at = start + i;
            start = at + macro_name.len();
            if src[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
            if src[line_start..at].contains("//") {
                continue;
            }
            let line = src[..at].matches('\n').count() + 1;
            let location = format!("{display}:{line}");
            let pos = skip_whitespace(&src, start);
            let (contexts, pos) = if let Some((ctx, end)) = string_literal(&src, pos) {
                (Ok(vec![ctx]), end)
            } else if let Some((ident, end)) = identifier(&src, pos, true) {
                let name = ident.rsplit("::").next().unwrap_or(ident);
                let values = resolve_identifier(&src, name, at, krate, 2);
                (values.ok_or_else(|| ident.to_owned()), end)
            } else {
                let expr: String = src[pos..].chars().take(20).collect();
                (Err(expr), pos)
            };
            let pos = skip_whitespace(&src, pos);
            let msgid = src[pos..].strip_prefix(',').and_then(|_| {
                let pos = skip_whitespace(&src, pos + 1);
                string_literal(&src, pos).map(|(s, _)| s)
            });
            calls.push(Call {
                location,
                contexts,
                msgid,
            });
        }
    }
}

/// Returns `(name, value)` of all `const NAME: &str = "value";` of a crate.
fn crate_consts(files: &[PathBuf]) -> Vec<(String, String)> {
    let mut consts = Vec::new();
    for file in files {
        let src = std::fs::read_to_string(file).expect("read source file");
        let mut start = 0;
        while let Some(i) = src[start..].find("const ") {
            let pos = start + i + "const ".len();
            start = pos;
            let Some((name, end)) = identifier(&src, pos, false) else {
                continue;
            };
            let rest = src[end..].trim_start();
            let Some(rest) = rest
                .strip_prefix(": &str")
                .or_else(|| rest.strip_prefix(": &'static str"))
            else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            let literal_pos = src.len() - rest.trim_start().len();
            if let Some((value, _)) = string_literal(&src, literal_pos) {
                consts.push((name.to_owned(), value));
            }
        }
    }
    consts
}

#[test]
fn tr_contexts_and_strings_exist_in_catalog() {
    let root = workspace_root();
    let pot = std::fs::read_to_string(root.join("lang/librepcb.pot")).expect("read template");
    let keys = catalog_keys(&pot).expect("parse template");
    let contexts: BTreeSet<&str> = keys.iter().map(|(c, _)| c.as_str()).collect();

    let mut calls = Vec::new();
    let mut crate_dirs: Vec<_> = std::fs::read_dir(root.join("crates"))
        .expect("read crates")
        .flatten()
        .map(|e| e.path())
        .collect();
    crate_dirs.push(root.join("tools/ts2po"));
    crate_dirs.sort();
    for dir in crate_dirs {
        let relative = dir.strip_prefix(&root).unwrap_or(&dir);
        if EXCLUDED.iter().any(|e| relative == Path::new(e)) {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        let krate = CrateSources {
            consts: crate_consts(&files),
            sources: files
                .iter()
                .map(|f| std::fs::read_to_string(f).expect("read source file"))
                .collect(),
        };
        for file in &files {
            scan_file(file, &krate, &mut calls);
        }
    }
    assert!(
        calls.len() > 500,
        "scanner found only {} calls",
        calls.len()
    );

    let mut errors = Vec::new();
    for call in &calls {
        let call_contexts = match &call.contexts {
            Ok(c) => c,
            Err(expr) => {
                errors.push(format!("{}: unresolved context `{expr}`", call.location));
                continue;
            }
        };
        for ctx in call_contexts {
            if NEW_CONTEXTS.contains(&ctx.as_str()) {
                continue;
            }
            match &call.msgid {
                Some(msgid) => {
                    let known = keys.contains(&(ctx.clone(), msgid.clone()))
                        || NEW_STRINGS.contains(&(ctx.as_str(), msgid.as_str()));
                    if !known {
                        errors.push(format!(
                            "{}: ({ctx:?}, {msgid:?}) not in the catalog",
                            call.location
                        ));
                    }
                }
                None if !contexts.contains(ctx.as_str()) => {
                    errors.push(format!("{}: unknown context {ctx:?}", call.location))
                }
                None => {}
            }
        }
    }
    assert!(
        errors.is_empty(),
        "{} translation lookups would fail (use the full upstream context, \
         e.g. \"librepcb::editor::CmdBoardEdit\", or add genuinely new strings \
         to NEW_STRINGS):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

#[test]
fn string_literals() {
    let src = r##"x "a\"b\n\u{e9}" r#"raw "q""# "line \
                 continued""##;
    assert_eq!(
        string_literal(src, 2),
        Some(("a\"b\n\u{e9}".to_owned(), 16))
    );
    assert_eq!(
        string_literal(src, src.find("r#").unwrap()).map(|(s, _)| s),
        Some("raw \"q\"".to_owned())
    );
    let pos = src.rfind("\"line").unwrap();
    assert_eq!(
        string_literal(src, pos).map(|(s, _)| s),
        Some("line continued".to_owned())
    );
}

#[test]
fn parameters() {
    let src = "fn f(form: &mut Form, context: &str, set: &Set<A, B>) { tr!(context, \"m\") }\n\
               fn g() { let c = \"B\"; f(&mut x, c, &y); f(a, \"A\", (b, c)); }";
    let krate = CrateSources {
        consts: Vec::new(),
        sources: vec![src.to_owned()],
    };
    let at = src.find("tr!").unwrap();
    assert_eq!(
        resolve_identifier(src, "context", at, &krate, 1),
        Some(vec!["A".to_owned(), "B".to_owned()])
    );
    assert_eq!(resolve_identifier(src, "context", at, &krate, 0), None);
    assert_eq!(
        arguments(src, src.find('(').unwrap(), true).map(|a| a.len()),
        Some(3)
    );
}

#[test]
fn local_bindings() {
    let src = "const CTX: &str = \"A\";\nfn f() { let ctx = if x { \"B\" } else { \"C\" }; tr!(ctx, \"m\") }\n\
               fn g() { let c = if x { CTX } else { OTHER_CTX }; tr!(c, \"m\") }";
    let consts = [("OTHER_CTX".to_owned(), "D".to_owned())];
    let at = src.find("tr!").unwrap();
    assert_eq!(
        local_binding(src, "CTX", at, &consts),
        Some(vec!["A".to_owned()])
    );
    assert_eq!(
        local_binding(src, "ctx", at, &consts),
        Some(vec!["B".to_owned(), "C".to_owned()])
    );
    assert_eq!(local_binding(src, "other", at, &consts), None);
    let at = src.rfind("tr!").unwrap();
    assert_eq!(
        local_binding(src, "c", at, &consts),
        Some(vec!["A".to_owned(), "D".to_owned()])
    );
}
