// Source: CMRemote, clean-room implementation.

//! CMRemote agent binary entry point.

use std::process::ExitCode;

use cmremote_agent::{
    cli::{CliArgs, USAGE},
    logging::{self, LogFormat},
    runtime,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cli = match CliArgs::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    if cli.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("cmremote-agent {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    logging::init(LogFormat::auto());

    match runtime::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "agent exited with error");
            ExitCode::FAILURE
        }
    }
}
