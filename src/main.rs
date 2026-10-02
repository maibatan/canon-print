use canon_print::cli::{Cli, Command};
use canon_print::commands::{self, Outcome};
use clap::Parser;
use serde_json::json;

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::List { all } => commands::list::run(*all),
        Command::Inspect => commands::inspect::run(),
        Command::Diagnose => Ok(commands::diagnose::run()),
        Command::Status {
            detach_kernel_driver,
            dump_protocol,
        } => Ok(commands::status::run(
            *detach_kernel_driver,
            dump_protocol.as_deref(),
            cli.verbose,
        )),
        Command::Print {
            file,
            copies,
            dry_run,
            paper,
            dump_protocol,
            detach_kernel_driver,
            ..
        } => Ok(commands::print::run(
            file,
            &commands::print::PrintOptions {
                paper,
                copies: *copies,
                dry_run: *dry_run,
                detach: *detach_kernel_driver,
                dump: dump_protocol.as_deref(),
                verbose: cli.verbose,
            },
        )),
    };
    let outcome = result.unwrap_or_else(|e| Outcome {
        json: json!({ "success": false, "error": e.kind(), "message": e.to_string() }),
        text: format!("error: {e}\n"),
        exit_code: e.exit_code(),
    });
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&outcome.json).expect("serializable")
        );
    } else {
        print!("{}", outcome.text);
    }
    std::process::exit(outcome.exit_code);
}
