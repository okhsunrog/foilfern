//! Help texts and parser error messages in the format of Qt's
//! `QCommandLineParser` (used by upstream apps/librepcb-cli).
//!
//! The command line is parsed by `clap`, but the help texts and error
//! messages are part of the CLI's output which scripts and the upstream CLI
//! tests compare verbatim. `clap`'s own formatting differs, so this module
//! reproduces `QCommandLineParser::helpText()` (including its word
//! wrapping) and `errorText()`. Hand-written because no crate reproduces
//! Qt's output format.

use clap::error::{ContextKind, ContextValue, ErrorKind};
use librepcb_i18n::tr;

/// An option in a help text.
#[derive(Debug, Clone)]
pub struct HelpOption {
    /// Names without dashes, e.g. `["h", "help"]`.
    pub names: Vec<String>,
    /// Value name, if the option takes a value.
    pub value_name: Option<String>,
    /// Description.
    pub description: String,
}

/// A positional argument in a help text.
#[derive(Debug, Clone)]
pub struct HelpPositional {
    /// Name shown in the "Arguments" section.
    pub name: String,
    /// Description.
    pub description: String,
    /// Syntax shown in the usage line.
    pub syntax: String,
}

/// Qt's `QCommandLineParser::helpText()`.
pub fn help_text(
    executable: &str,
    description: &str,
    options: &[HelpOption],
    positionals: &[HelpPositional],
) -> String {
    let mut usage = executable.to_owned();
    if !options.is_empty() {
        usage.push(' ');
        usage.push_str(&tr!("QCommandLineParser", "[options]"));
    }
    for arg in positionals {
        usage.push(' ');
        usage.push_str(&arg.syntax);
    }
    let mut text = tr!("QCommandLineParser", "Usage: {0}", usage);
    text.push('\n');
    if !description.is_empty() {
        text.push_str(description);
        text.push('\n');
    }
    text.push('\n');
    if !options.is_empty() {
        text.push_str(&tr!("QCommandLineParser", "Options:"));
        text.push('\n');
    }
    let names: Vec<String> = options
        .iter()
        .map(|option| {
            let mut s = option
                .names
                .iter()
                .map(|name| {
                    let dashes = if name.chars().count() == 1 { "-" } else { "--" };
                    format!("{dashes}{name}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(value_name) = &option.value_name {
                s.push_str(&format!(" <{value_name}>"));
            }
            s
        })
        .collect();
    let longest = names.iter().map(|n| n.chars().count()).max().unwrap_or(0) + 1;
    let width = longest.min(50);
    for (name, option) in names.iter().zip(options) {
        text.push_str(&wrap_text(name, width, &option.description));
    }
    if !positionals.is_empty() {
        if !options.is_empty() {
            text.push('\n');
        }
        text.push_str(&tr!("QCommandLineParser", "Arguments:"));
        text.push('\n');
        for arg in positionals {
            text.push_str(&wrap_text(&arg.name, width, &arg.description));
        }
    }
    text
}

/// Qt's `wrapText()` of `QCommandLineParser` (a line is at most 79
/// characters wide), reproduced exactly including its quirks: the first
/// character of a continuation line is not counted towards the line width.
fn wrap_text(names: &str, width: usize, description: &str) -> String {
    let indentation = "  ";
    let names: Vec<char> = names.chars().collect();
    let mut name_index = 0;
    // Returns the next `width` characters of the names.
    let next_name_section = |name_index: &mut usize| -> String {
        let end = (*name_index + width).min(names.len());
        let section: String = names[*name_index..end].iter().collect();
        *name_index = end;
        section
    };

    let desc: Vec<char> = description.chars().collect();
    let len = desc.len();
    let max = 79_i64 - (indentation.len() + width + 1) as i64;
    let mut text = String::new();
    let mut line_start = 0;
    let mut last_breakable: Option<usize> = None;
    let mut x: i64 = 0;
    let mut i = 0;
    while i < len {
        x += 1;
        let c = desc[i];
        if c.is_whitespace() {
            last_breakable = Some(i);
        }
        let mut brk: Option<(usize, usize)> = None; // (break at, next line start)
        if let (true, Some(lb)) = (x > max, last_breakable) {
            // Time to break and we know where.
            brk = Some((lb, lb + 1));
        } else if (x > max - 1 && last_breakable.is_none()) || i == len - 1 {
            // Time to break but found nowhere [-> break here], or end of
            // last line.
            brk = Some((i + 1, i + 1));
        } else if c == '\n' {
            // Forced break.
            brk = Some((i, i + 1));
        }
        if let Some((break_at, next_line_start)) = brk {
            let section = next_name_section(&mut name_index);
            text.push_str(indentation);
            text.push_str(&format!("{section:<width$}"));
            text.push(' ');
            text.extend(&desc[line_start..break_at.max(line_start)]);
            text.push('\n');
            x = 0;
            last_breakable = None;
            line_start = next_line_start;
            if line_start < len && desc[line_start].is_whitespace() {
                line_start += 1; // Don't start a line with a space.
            }
            i = line_start;
        }
        i += 1;
    }
    while name_index < names.len() {
        let section = next_name_section(&mut name_index);
        text.push_str(indentation);
        text.push_str(&section);
        text.push('\n');
    }
    text
}

/// Parses `args` (the first element is the executable) like
/// `QCommandLineParser::parse()` with its default modes and returns the text
/// of `errorText()`, or `None` if the arguments are valid.
///
/// Long options are `--name` or `--name=value`, short options are compacted
/// (`-abc` are the options `a`, `b` and `c`; an option taking a value takes
/// the rest of the argument or the next argument), `--` ends the options.
/// Like Qt, all arguments are parsed: the last value error ("Missing value
/// after ...", "Unexpected value after ...") wins, otherwise all unknown
/// options are listed.
pub fn qt_parse_error(args: &[String], options: &[HelpOption]) -> Option<String> {
    let takes_value = |name: &str| {
        options
            .iter()
            .find(|o| o.names.iter().any(|n| n == name))
            .map(|o| o.value_name.is_some())
    };
    let mut unknown = Vec::new();
    let mut error = None;
    let mut iter = args.iter().skip(1);
    // Upstream `parseOptionValue()` for a known option.
    let mut parse_value = |name: &str, argument: &str, iter: &mut std::iter::Skip<_>| {
        let Some(with_value) = takes_value(name) else {
            return;
        };
        let assign = argument.find('=');
        if with_value && assign.is_none() {
            if Iterator::next(iter).is_none() {
                error = Some(tr!(
                    "QCommandLineParser",
                    "Missing value after '{0}'.",
                    argument
                ));
            }
        } else if let (false, Some(pos)) = (with_value, assign) {
            error = Some(tr!(
                "QCommandLineParser",
                "Unexpected value after '{0}'.",
                &argument[..pos]
            ));
        }
    };
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or_default();
            if takes_value(name).is_some() {
                parse_value(name, arg, &mut iter);
            } else {
                unknown.push(name.to_owned());
            }
        } else if let Some(short) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let mut last = "";
            let mut value_found = false;
            for (i, c) in short.char_indices() {
                last = &short[i..i + c.len_utf8()];
                let rest = &short[i + c.len_utf8()..];
                match takes_value(last) {
                    None => unknown.push(last.to_owned()),
                    Some(true) => {
                        value_found = !rest.is_empty();
                        break;
                    }
                    Some(false) if rest.starts_with('=') => break,
                    Some(false) => {}
                }
            }
            if !value_found {
                parse_value(last, arg, &mut iter);
            }
        }
    }
    error.or(match unknown.as_slice() {
        [] => None,
        [name] => Some(tr!("QCommandLineParser", "Unknown option '{0}'.", name)),
        names => Some(tr!(
            "QCommandLineParser",
            "Unknown options: {0}.",
            names.join(", ")
        )),
    })
}

/// Converts a `clap` parse error into the error text of Qt's
/// `QCommandLineParser::errorText()`.
pub fn parse_error_text(err: &clap::Error) -> String {
    let arg_name = || -> String {
        match err.get(ContextKind::InvalidArg) {
            Some(ContextValue::String(s)) => s.clone(),
            Some(ContextValue::Strings(v)) => v.first().cloned().unwrap_or_default(),
            _ => String::new(),
        }
    };
    // "--board <name>" -> "--board", "--foo=bar" -> "--foo"
    let option = || -> String {
        let name = arg_name();
        name.split([' ', '=']).next().unwrap_or_default().to_owned()
    };
    match err.kind() {
        ErrorKind::UnknownArgument => {
            let name = option();
            let name = name.trim_start_matches('-');
            tr!("QCommandLineParser", "Unknown option '{0}'.", name)
        }
        ErrorKind::InvalidValue | ErrorKind::NoEquals => {
            tr!("QCommandLineParser", "Missing value after '{0}'.", option())
        }
        ErrorKind::TooManyValues => {
            tr!(
                "QCommandLineParser",
                "Unexpected value after '{0}'.",
                option()
            )
        }
        _ => {
            // Not expected to happen (positional arguments are validated by
            // the CLI itself): show clap's message without its "error: "
            // prefix and usage.
            let rendered = err.render().to_string();
            let first = rendered.lines().next().unwrap_or_default();
            first.strip_prefix("error: ").unwrap_or(first).to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errors_like_qt() {
        let options = vec![
            HelpOption {
                names: vec!["v".into(), "verbose".into()],
                value_name: None,
                description: String::new(),
            },
            HelpOption {
                names: vec!["b".into(), "board".into()],
                value_name: Some("name".into()),
                description: String::new(),
            },
        ];
        let error = |args: &[&str]| {
            let args: Vec<String> = std::iter::once("cli")
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect();
            qt_parse_error(&args, &options)
        };
        let unknown = |names: &str| Some(format!("Unknown options: {names}."));
        assert_eq!(error(&["--foo", "--bar=1", "x"]), unknown("foo, bar"));
        assert_eq!(error(&["-xyz", "--foo"]), unknown("x, y, z, foo"));
        assert_eq!(error(&["-abc"]), Some("Unknown option 'a'.".into()));
        assert_eq!(error(&["-vx", "--board", "--foo", "-y"]), unknown("x, y"));
        assert_eq!(error(&["-x"]), Some("Unknown option 'x'.".into()));
        assert_eq!(error(&["--board=--foo", "-bvx", "-", "--", "--bar"]), None);
        assert_eq!(
            error(&["--foo", "--board"]),
            Some("Missing value after '--board'.".into())
        );
        assert_eq!(error(&["-xvb"]), Some("Missing value after '-xvb'.".into()));
        assert_eq!(
            error(&["--verbose=1", "--foo"]),
            Some("Unexpected value after '--verbose'.".into())
        );
        assert_eq!(
            error(&["-v=1"]),
            Some("Unexpected value after '-v'.".into())
        );
    }

    #[test]
    fn wrap_short_description() {
        assert_eq!(
            wrap_text("-h, --help", 14, "Print this message."),
            "  -h, --help     Print this message.\n"
        );
    }

    #[test]
    fn wrap_long_description() {
        let text = wrap_text(
            "--erc",
            34,
            "Run the electrical rule check, print all non-approved warnings/errors and report \
             failure (exit code = 1) if there are non-approved messages.",
        );
        assert_eq!(
            text,
            "  --erc                              Run the electrical rule check, print all\n\
            \x20                                    non-approved warnings/errors and report\n\
            \x20                                    failure (exit code = 1) if there are\n\
            \x20                                    non-approved messages.\n"
        );
    }

    #[test]
    fn wrap_empty_description() {
        assert_eq!(wrap_text("--foo", 10, ""), "  --foo\n");
    }
}
