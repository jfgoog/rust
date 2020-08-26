//! Command like parsing for rust-analyzer.
//!
//! If run started args, we run the LSP server loop. With a subcommand, we do a
//! one-time batch processing.

use std::{env, fmt::Write, path::PathBuf};

use anyhow::{bail, Result};
use pico_args::Arguments;
use rust_analyzer::cli::{AnalysisStatsCmd, BenchCmd, BenchWhat, Position, Verbosity};
use ssr::{SsrPattern, SsrRule};
use vfs::AbsPathBuf;

pub(crate) struct Args {
    pub(crate) verbosity: Verbosity,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) command: Command,
}

pub(crate) enum Command {
    Parse { no_dump: bool },
    Symbols,
    Highlight { rainbow: bool },
    AnalysisStats(AnalysisStatsCmd),
    Bench(BenchCmd),
    Diagnostics { path: PathBuf, load_output_dirs: bool, with_proc_macro: bool },
    Ssr { rules: Vec<SsrRule> },
    StructuredSearch { debug_snippet: Option<String>, patterns: Vec<SsrPattern> },
    ProcMacro,
    RunServer,
    Version,
    Help,
}

const HELP: &str = "\
rust-analyzer

USAGE:
    rust-analyzer [FLAGS] [COMMAND] [COMMAND_OPTIONS]

FLAGS:
    --version         Print version
    -h, --help        Print this help

    -v,  --verbose
    -vv, --spammy
    -q,  --quiet      Set verbosity

    --log-file <PATH> Log to the specified filed instead of stderr

ENVIRONMENTAL VARIABLES:
    RA_LOG            Set log filter in env_logger format
    RA_PROFILE        Enable hierarchical profiler

COMMANDS:

not specified         Launch LSP server

parse < main.rs       Parse tree
    --no-dump         Suppress printing

symbols < main.rs     Parse input an print the list of symbols

highlight < main.rs   Highlight input as html
    --rainbow         Enable rainbow highlighting of identifiers

analysis-stats <PATH> Batch typecheck project and print summary statistics
    <PATH>            Directory with Cargo.toml
    --randomize       Randomize order in which crates, modules, and items are processed
    --parallel        Run type inference in parallel
    --memory-usage    Collect memory usage statistics
    -o, --only <PATH> Only analyze items matching this path
    --with-deps       Also analyze all dependencies
    --load-output-dirs
                      Load OUT_DIR values by running `cargo check` before analysis
    --with-proc-macro Use proc-macro-srv for proc-macro expanding

analysis-bench <PATH> Benchmark specific analysis operation
    <PATH>            Directory with Cargo.toml
    --highlight <PATH>
                      Compute syntax highlighting for this file
    --complete <PATH:LINE:COLUMN>
                      Compute completions at this location
    --goto-def <PATH:LINE:COLUMN>
                      Compute goto definition at this location
    --memory-usage    Collect memory usage statistics
    --load-output-dirs
                      Load OUT_DIR values by running `cargo check` before analysis
    --with-proc-macro Use proc-macro-srv for proc-macro expanding

diagnostics <PATH>
    <PATH>            Directory with Cargo.toml
    --load-output-dirs
                      Load OUT_DIR values by running `cargo check` before analysis
    --with-proc-macro Use proc-macro-srv for proc-macro expanding

ssr [RULE...]
    <RULE>            A structured search replace rule (`$a.foo($b) ==> bar($a, $b)`)
    --debug <snippet> Prints debug information for any nodes with source exactly
                      equal to <snippet>

search [PATTERN..]
    <PATTERN>         A structured search replace pattern (`$a.foo($b)`)
    --debug <snippet> Prints debug information for any nodes with source exactly
                      equal to <snippet>
";

impl Args {
    pub(crate) fn parse() -> Result<Args> {
        let mut matches = Arguments::from_env();

        if matches.contains("--version") {
            matches.finish().or_else(handle_extra_flags)?;
            return Ok(Args {
                verbosity: Verbosity::Normal,
                log_file: None,
                command: Command::Version,
            });
        }

        let verbosity = match (
            matches.contains(["-vv", "--spammy"]),
            matches.contains(["-v", "--verbose"]),
            matches.contains(["-q", "--quiet"]),
        ) {
            (true, _, true) => bail!("Invalid flags: -q conflicts with -vv"),
            (true, _, false) => Verbosity::Spammy,
            (false, false, false) => Verbosity::Normal,
            (false, false, true) => Verbosity::Quiet,
            (false, true, false) => Verbosity::Verbose,
            (false, true, true) => bail!("Invalid flags: -q conflicts with -v"),
        };
        let log_file = matches.opt_value_from_str("--log-file")?;

        if matches.contains(["-h", "--help"]) {
            eprintln!("{}", HELP);
            return Ok(Args { verbosity, log_file: None, command: Command::Help });
        }

        let subcommand = match matches.subcommand()? {
            Some(it) => it,
            None => {
                matches.finish().or_else(handle_extra_flags)?;
                return Ok(Args { verbosity, log_file, command: Command::RunServer });
            }
        };
        let command = match subcommand.as_str() {
            "parse" => {
                let no_dump = matches.contains("--no-dump");
                matches.finish().or_else(handle_extra_flags)?;
                Command::Parse { no_dump }
            }
            "symbols" => {
                matches.finish().or_else(handle_extra_flags)?;
                Command::Symbols
            }
            "highlight" => {
                let rainbow = matches.contains("--rainbow");
                matches.finish().or_else(handle_extra_flags)?;
                Command::Highlight { rainbow }
            }
            "analysis-stats" => {
                let randomize = matches.contains("--randomize");
                let parallel = matches.contains("--parallel");
                let memory_usage = matches.contains("--memory-usage");
                let only: Option<String> = matches.opt_value_from_str(["-o", "--only"])?;
                let with_deps: bool = matches.contains("--with-deps");
                let load_output_dirs = matches.contains("--load-output-dirs");
                let with_proc_macro = matches.contains("--with-proc-macro");
                let path = {
                    let mut trailing = matches.free()?;
                    if trailing.len() != 1 {
                        bail!("Invalid flags");
                    }
                    trailing.pop().unwrap().into()
                };

                Command::AnalysisStats(AnalysisStatsCmd {
                    randomize,
                    parallel,
                    memory_usage,
                    only,
                    with_deps,
                    path,
                    load_output_dirs,
                    with_proc_macro,
                })
            }
            "analysis-bench" => {
                let highlight_path: Option<String> = matches.opt_value_from_str("--highlight")?;
                let complete_path: Option<Position> = matches.opt_value_from_str("--complete")?;
                let goto_def_path: Option<Position> = matches.opt_value_from_str("--goto-def")?;
                let what = match (highlight_path, complete_path, goto_def_path) {
                    (Some(path), None, None) => {
                        let path = env::current_dir().unwrap().join(path);
                        BenchWhat::Highlight { path: AbsPathBuf::assert(path) }
                    }
                    (None, Some(position), None) => BenchWhat::Complete(position),
                    (None, None, Some(position)) => BenchWhat::GotoDef(position),
                    _ => panic!(
                        "exactly one of  `--highlight`, `--complete` or `--goto-def` must be set"
                    ),
                };
                let memory_usage = matches.contains("--memory-usage");
                let load_output_dirs = matches.contains("--load-output-dirs");
                let with_proc_macro = matches.contains("--with-proc-macro");

                let path = {
                    let mut trailing = matches.free()?;
                    if trailing.len() != 1 {
                        bail!("Invalid flags");
                    }
                    trailing.pop().unwrap().into()
                };

                Command::Bench(BenchCmd {
                    memory_usage,
                    path,
                    what,
                    load_output_dirs,
                    with_proc_macro,
                })
            }
            "diagnostics" => {
                let load_output_dirs = matches.contains("--load-output-dirs");
                let with_proc_macro = matches.contains("--with-proc-macro");
                let path = {
                    let mut trailing = matches.free()?;
                    if trailing.len() != 1 {
                        bail!("Invalid flags");
                    }
                    trailing.pop().unwrap().into()
                };

                Command::Diagnostics { path, load_output_dirs, with_proc_macro }
            }
            "proc-macro" => Command::ProcMacro,
            "ssr" => {
                let mut rules = Vec::new();
                while let Some(rule) = matches.free_from_str()? {
                    rules.push(rule);
                }
                Command::Ssr { rules }
            }
            "search" => {
                let debug_snippet = matches.opt_value_from_str("--debug")?;
                let mut patterns = Vec::new();
                while let Some(rule) = matches.free_from_str()? {
                    patterns.push(rule);
                }
                Command::StructuredSearch { patterns, debug_snippet }
            }
            _ => {
                eprintln!("{}", HELP);
                return Ok(Args { verbosity, log_file: None, command: Command::Help });
            }
        };
        Ok(Args { verbosity, log_file, command })
    }
}

fn handle_extra_flags(e: pico_args::Error) -> Result<()> {
    if let pico_args::Error::UnusedArgsLeft(flags) = e {
        let mut invalid_flags = String::new();
        for flag in flags {
            write!(&mut invalid_flags, "{}, ", flag)?;
        }
        let (invalid_flags, _) = invalid_flags.split_at(invalid_flags.len() - 2);
        bail!("Invalid flags: {}", invalid_flags);
    } else {
        bail!(e);
    }
}
