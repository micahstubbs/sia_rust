//! Command-line argument parsing for the SIA orchestrator. Port of `sia/cli.py`.
//!
//! Top-level `sia` parser with `run` / `web` sub-commands. For backward
//! compatibility `sia --task gpqa` (no sub-command) is treated as `sia run --task
//! gpqa`; `sia web ...` opts into the visualizer.

use clap::{Arg, ArgAction, ArgGroup, Command};

use crate::config::Config;
use crate::layout::{names, BUNDLED_TASKS};

const SUBCOMMANDS: &[&str] = &["run", "web"];

fn add_run_args(cmd: Command, env_config: &Config) -> Command {
    cmd.arg(
        Arg::new("max_gen")
            .long("max_gen")
            .value_parser(clap::value_parser!(i64))
            .default_value(env_config.default_max_generations.to_string())
            .help("Maximum number of generations to run (default: 3)"),
    )
    .arg(
        Arg::new("run_id")
            .long("run_id")
            .value_parser(clap::value_parser!(i64))
            .default_value("1")
            .help("Run ID for this experiment (default: 1)"),
    )
    .arg(
        Arg::new("task")
            .long("task")
            .value_parser(clap::builder::PossibleValuesParser::new(BUNDLED_TASKS))
            .help("Name of a bundled task shipped with sia"),
    )
    .arg(
        Arg::new("task_dir")
            .long("task_dir")
            .help("Path to an external task directory (e.g., ./tasks/my-task)"),
    )
    .group(
        ArgGroup::new("task_source")
            .args(["task", "task_dir"])
            .required(true),
    )
    .arg(
        Arg::new("meta_agent_profile")
            .long("meta-agent-profile")
            .default_value(env_config.default_meta_agent_profile.clone())
            .help("Agent profile for the meta/feedback agent (name or path to a .json file)"),
    )
    .arg(
        Arg::new("target_agent_profile")
            .long("target-agent-profile")
            .default_value(env_config.default_target_agent_profile.clone())
            .help("Agent profile for the target agent (name or path to a .json file)"),
    )
    .arg(
        Arg::new("sandbox")
            .long("sandbox")
            .value_parser(["none", "docker"])
            .default_value(env_config.sandbox_mode.clone())
            .help("Sandbox mode for target agent execution: none (default) or docker"),
    )
    .arg(
        Arg::new("log_level")
            .long("log-level")
            .value_parser(["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"])
            .help("Logging verbosity (default: INFO, or the $SIA_LOG_LEVEL env var)."),
    )
    .arg(
        Arg::new("no_web")
            .long("no-web")
            .action(ArgAction::SetTrue)
            .help("Do not start the live visualizer dashboard during the run."),
    )
    .arg(
        Arg::new("web_port")
            .long("web-port")
            .value_parser(clap::value_parser!(u16))
            .default_value("8000")
            .help("Port for the live dashboard started during the run (default: 8000)."),
    )
    .arg(
        Arg::new("web_host")
            .long("web-host")
            .default_value("127.0.0.1")
            .help("Host for the live dashboard (default: 127.0.0.1)."),
    )
}

fn add_web_args(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("host")
            .long("host")
            .default_value("127.0.0.1")
            .help("Bind host (default: 127.0.0.1)."),
    )
    .arg(
        Arg::new("port")
            .long("port")
            .value_parser(clap::value_parser!(u16))
            .default_value("8000")
            .help("Bind port (default: 8000)."),
    )
    .arg(
        Arg::new("runs_dir")
            .long("runs-dir")
            .default_value(names::RUNS_ROOT)
            .help("Directory of runs to visualize (default: ./runs)."),
    )
    .arg(
        Arg::new("no_browser")
            .long("no-browser")
            .action(ArgAction::SetTrue)
            .help("Do not open a browser window automatically."),
    )
    .arg(
        Arg::new("log_level")
            .long("log-level")
            .value_parser(["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"])
            .help("Logging verbosity (default: INFO, or the $SIA_LOG_LEVEL env var)."),
    )
}

/// Build the top-level `sia` parser with `run` / `web` sub-commands.
pub fn build_parser(env_config: &Config) -> Command {
    let run = add_run_args(
        Command::new("run").about("Run the orchestrator (agent evolution)."),
        env_config,
    );
    let web = add_web_args(Command::new("web").about("Serve the runs visualizer over HTTP."));
    Command::new("sia")
        .about("SIA: Self-Improving AI framework")
        .subcommand_value_name("{run,web}")
        .subcommand(run)
        .subcommand(web)
}

/// Insert the default `run` sub-command unless the user asked for one (or for help),
/// matching the Python preprocessing.
fn with_default_subcommand(argv: &[String]) -> Vec<String> {
    let needs_default = match argv.first() {
        None => true,
        Some(first) => !SUBCOMMANDS.contains(&first.as_str()) && first != "-h" && first != "--help",
    };
    if needs_default {
        let mut v = vec!["run".to_string()];
        v.extend_from_slice(argv);
        v
    } else {
        argv.to_vec()
    }
}

/// Parse CLI arguments (excluding argv[0]), defaulting to the `run` sub-command.
///
/// Returns clap's `ArgMatches`; the binary reads the resolved sub-command from it.
/// On `--help`/parse error, clap prints and the caller maps to the right exit code.
pub fn parse_args(env_config: &Config, argv: &[String]) -> Result<clap::ArgMatches, clap::Error> {
    let raw = with_default_subcommand(argv);
    let mut full = vec!["sia".to_string()];
    full.extend(raw);
    build_parser(env_config).try_get_matches_from(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_subcommand_inserted() {
        assert_eq!(
            with_default_subcommand(&["--task".into(), "gpqa".into()]),
            vec!["run", "--task", "gpqa"]
        );
        assert_eq!(with_default_subcommand(&[]), vec!["run"]);
        assert_eq!(with_default_subcommand(&["web".into()]), vec!["web"]);
        assert_eq!(with_default_subcommand(&["--help".into()]), vec!["--help"]);
    }

    #[test]
    fn test_invalid_task_is_error() {
        let cfg = Config::default();
        assert!(parse_args(&cfg, &["run".into(), "--task".into(), "nonexistent".into()]).is_err());
    }

    #[test]
    fn test_no_args_is_error() {
        let cfg = Config::default();
        // Defaults to `run`, which requires --task/--task_dir.
        assert!(parse_args(&cfg, &[]).is_err());
    }

    #[test]
    fn test_valid_run_parses() {
        let cfg = Config::default();
        let m = parse_args(&cfg, &["run".into(), "--task".into(), "gpqa".into()]).unwrap();
        let (sub, sm) = m.subcommand().unwrap();
        assert_eq!(sub, "run");
        assert_eq!(sm.get_one::<String>("task").unwrap(), "gpqa");
    }
}
