use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use autoratchet::collect::collect_from_directory;
use autoratchet::compare::{CompareResult, Verdict, compare_paths};
use autoratchet::gate::GateOptions;
use autoratchet::git::{CheckOptions, check};
use autoratchet::report::{format_json, format_markdown, format_text};
use autoratchet::tools::Tools;

const USAGE: &str = "Usage:
  autoratchet check [--base <ref>] [options] [pathspec...]
      Compare the working tree with merge-base(<ref>, HEAD). Default ref: main.
      Pathspecs limit what is measured, e.g. src ':!src/generated'.

  autoratchet compare --base <dir|json> --head <dir|json> [options]
      Compare two directories (or JSON snapshots) directly.

  autoratchet measure <dir>
      Print every function's scores as JSON.

Options:
  --threshold <n>               maximum cyclomatic score for new functions (default 10)
  --cognitive-threshold <n>     maximum cognitive score for new functions (default 15)
  --require-override-approval   fail on any new override instead of treating it as an approval
  --format <text|json|markdown> output format (default text)
  -o, --output <file>           write the report to a file instead of stdout

Exit codes: 0 pass, 1 regression, 2 bad arguments, 3 measurement incomplete, 4 tool error.";

enum Error {
    Usage(String),
    Tool(String),
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::Tool(message)
    }
}

const VALUE_OPTIONS: [&str; 7] = [
    "base",
    "head",
    "threshold",
    "cyclomatic-threshold",
    "cognitive-threshold",
    "format",
    "output",
];
const FLAG_OPTIONS: [&str; 2] = ["require-override-approval", "help"];

struct Args {
    values: HashMap<String, String>,
    flags: Vec<String>,
    positionals: Vec<String>,
}

fn parse_args(argv: &[String]) -> Result<Args, Error> {
    let mut args = Args {
        values: HashMap::new(),
        flags: Vec::new(),
        positionals: Vec::new(),
    };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            args.positionals.extend(it.by_ref().cloned());
            break;
        }
        let (name, inline) = if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (long.to_string(), None),
            }
        } else if let Some(short) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let (flag, rest) = short.split_at(1);
            let name = match flag {
                "o" => "output",
                "h" => "help",
                _ => return Err(Error::Usage(format!("Unknown option '{arg}'"))),
            };
            (name.to_string(), (!rest.is_empty()).then(|| rest.to_string()))
        } else {
            args.positionals.push(arg.clone());
            continue;
        };
        if VALUE_OPTIONS.contains(&name.as_str()) {
            let value = match inline {
                Some(v) => v,
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| Error::Usage(format!("Option '--{name} <value>' argument missing")))?,
            };
            args.values.insert(name, value);
        } else if FLAG_OPTIONS.contains(&name.as_str()) && inline.is_none() {
            args.flags.push(name);
        } else {
            return Err(Error::Usage(format!("Unknown option '{arg}'")));
        }
    }
    Ok(args)
}

fn number(value: Option<&String>, flag: &str) -> Result<Option<f64>, Error> {
    let Some(value) = value else { return Ok(None) };
    match value.trim().parse::<f64>() {
        Ok(n) if n.is_finite() && n >= 1.0 => Ok(Some(n)),
        _ => Err(Error::Usage(format!("{flag} needs a positive number, got \"{value}\""))),
    }
}

fn run(argv: &[String]) -> Result<u8, Error> {
    let args = parse_args(argv)?;
    if args.flags.iter().any(|f| f == "help") {
        println!("{USAGE}");
        return Ok(0);
    }
    let Some((command, paths)) = args.positionals.split_first() else {
        println!("{USAGE}");
        return Ok(2);
    };
    let format = args.values.get("format").map_or("text", String::as_str);
    if !matches!(format, "text" | "json" | "markdown") {
        return Err(Error::Usage(format!(
            "--format must be text, json or markdown, got \"{format}\""
        )));
    }
    let gate = GateOptions {
        cyclomatic_threshold: number(
            args.values.get("threshold").or(args.values.get("cyclomatic-threshold")),
            "--threshold",
        )?,
        cognitive_threshold: number(args.values.get("cognitive-threshold"), "--cognitive-threshold")?,
        require_override_approval: args.flags.iter().any(|f| f == "require-override-approval"),
    };

    let result: CompareResult = match command.as_str() {
        "check" => {
            let tools = Tools::discover()?;
            let options = CheckOptions {
                base: args.values.get("base").cloned().unwrap_or_else(|| "main".into()),
                paths: paths.to_vec(),
                cwd: std::env::current_dir().map_err(|e| e.to_string())?,
                gate,
            };
            check(&options, &tools)?
        }
        "compare" => {
            let (Some(base), Some(head)) = (args.values.get("base"), args.values.get("head")) else {
                return Err(Error::Usage("compare needs --base and --head".into()));
            };
            let tools = Tools::discover()?;
            compare_paths(&PathBuf::from(base), &PathBuf::from(head), &gate, &tools)?
        }
        "measure" => {
            let [dir] = paths else {
                return Err(Error::Usage("measure needs one directory".into()));
            };
            let snapshots = collect_from_directory(&PathBuf::from(dir), &Tools::discover()?)?;
            let functions: Vec<_> = snapshots.values().collect();
            let out = serde_json::json!({ "functions": functions, "problems": snapshots.problems });
            println!("{}", serde_json::to_string_pretty(&out).expect("serializable"));
            return Ok(if snapshots.problems.is_empty() { 0 } else { 3 });
        }
        other => return Err(Error::Usage(format!("unknown command \"{other}\""))),
    };

    let report = match format {
        "json" => format_json(&result),
        "markdown" => format_markdown(&result),
        _ => format_text(&result),
    };
    match args.values.get("output") {
        Some(file) => std::fs::write(file, format!("{report}\n")).map_err(|e| format!("{file}: {e}"))?,
        None => println!("{report}"),
    }
    Ok(match result.verdict() {
        Verdict::Pass => 0,
        Verdict::Fail => 1,
        Verdict::Incomplete => 3,
    })
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(code) => ExitCode::from(code),
        Err(Error::Usage(message)) => {
            eprintln!("autoratchet: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Tool(message)) => {
            eprintln!("autoratchet: {message}");
            ExitCode::from(4)
        }
    }
}
