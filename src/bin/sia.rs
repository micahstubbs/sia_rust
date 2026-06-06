//! `sia` binary entry point — Rust port of `sia.orchestrator.main`.
//!
//! Parses the `run` / `web` sub-commands (with the backward-compatible default)
//! and dispatches. The full run/web execution is wired in issue #9.

use sia::cli;
use sia::config::Config;

fn print_welcome() {
    let banner = format!(
        r#"
     _______. __       ___
    /       ||  |     /   \
   |   (----`|  |    /  ^  \
    \   \    |  |   /  /_\  \
.----)   |   |  |  /  _____  \
|_______/    |__| /__/     \__\

    Self-Improving AI framework

    • Version : v{version}
    • Docs    : https://github.com/hexo-ai/sia
    • Help    : sia --help
"#,
        version = sia::VERSION
    );
    println!("{banner}");
}

fn main() {
    let env_config = Config::from_env();
    print_welcome();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let matches = match cli::parse_args(&env_config, &argv) {
        Ok(m) => m,
        // clap prints help/version (exit 0) or a usage error (exit 2) with the right code.
        Err(e) => e.exit(),
    };

    let result = match matches.subcommand() {
        Some(("web", sm)) => sia::run::run_web(sm),
        Some(("run", sm)) => sia::run::run_orchestrator(sm, &env_config),
        _ => Ok(()),
    };

    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
