//! The `gramps-events` binary — the thin clap front-end (plan §6.5).
//!
//! Argument parsing, file loading and printing live here; every command's
//! logic is in the `cli` library so it stays unit-testable. `serve` (the
//! web UI) joined the subcommand set in milestone 13.

use std::process::ExitCode;

use clap::Parser;
use cli::args::{self, Cli, Command};
use cli::run;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Inspect { file } => {
            let db = args::load_database(&file)?;
            print!("{}", run::inspect(&db));
            Ok(())
        }
        Command::List(list) => {
            let db = args::load_database(&list.file)?;
            let opts = args::report_options(&list.filters)?;
            let out = run::list(&db, &opts, list.view.into());
            print!("{out}");
            Ok(())
        }
        Command::Report(report) => {
            let db = args::load_database(&report.file)?;
            let opts = args::report_options(&report.filters)?;
            let formats = args::formats_from(&report.format);
            let out_dir = report
                .out_dir
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let prefix = report
                .out_prefix
                .unwrap_or_else(|| args::default_out_prefix(&report.file));
            let written = run::report(&db, &opts, formats, &out_dir, &prefix)?;
            for path in written {
                println!("{}", path.display());
            }
            Ok(())
        }
        Command::Serve(serve) => {
            run::serve(serve.port)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch;
    use clap::Parser;

    /// The whole binary path round-trips: args → db → report --format all
    /// writes four files into a temp dir.
    #[test]
    fn end_to_end_report_all_via_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = {
            // Materialize the fixture to disk so dispatch's filesystem
            // path can read it.
            let path = dir.path().join("data.gramps");
            std::fs::write(&path, include_bytes!("../../../tests/fixtures/data.gramps")).unwrap();
            path
        };
        let cli = cli::args::Cli::try_parse_from([
            "gramps-events",
            "report",
            fixture.to_str().unwrap(),
            "--format",
            "all",
            "--out-dir",
            dir.path().to_str().unwrap(),
            "--out-prefix",
            "x",
            "--reference-year",
            "2030",
        ])
        .unwrap();
        dispatch(cli).unwrap();
        assert!(dir.path().join("x.csv").exists());
        assert!(dir.path().join("x.json").exists());
        assert!(dir.path().join("x.parquet").exists());
        assert!(dir.path().join("x.pdf").exists());
    }
}
