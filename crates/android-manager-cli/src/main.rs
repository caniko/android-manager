use android_manager_runtime::{
    render_plan_human, AdbBackend, ApkSignerInspector, ExecutionPlan, Runtime, SystemRunner,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Declarative Android device manager")]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    lock_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Check,
    Inventory,
    Lock,
    Plan,
    Apply {
        #[arg(long)]
        plan: Option<PathBuf>,
    },
    Verify,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut runtime = Runtime::load(
        &cli.config,
        AdbBackend::<SystemRunner>::default(),
        Box::new(ApkSignerInspector),
    )?;
    if let Some(path) = cli.lock_file {
        runtime.use_lock_path(path)?;
    }
    match cli.command {
        Command::Lock => {
            let lock = runtime.lock()?;
            if cli.json {
                print!("{}", serde_json::to_string_pretty(&lock)? + "\n");
            } else {
                eprintln!("wrote {}", runtime.lock_path.display());
            }
        }
        Command::Inventory => {
            let snapshot = runtime.inventory()?;
            print!("{}", serde_json::to_string_pretty(&snapshot)? + "\n");
        }
        Command::Check => {
            let plan = runtime.check()?;
            emit(&runtime, &plan, cli.json)?;
        }
        Command::Plan => {
            let plan = runtime.plan()?;
            emit(&runtime, &plan, cli.json)?;
        }
        Command::Apply { plan } => {
            let expected = match plan {
                Some(path) => Some(
                    serde_json::from_str::<ExecutionPlan>(
                        &fs::read_to_string(path).context("read plan")?,
                    )
                    .context("parse plan")?,
                ),
                None => None,
            };
            let journal = runtime.apply(expected.as_ref())?;
            let verified = runtime.verify()?;
            if cli.json {
                print!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "journal": journal,
                        "verify": verified,
                    }))? + "\n"
                );
            } else {
                eprintln!("apply {}", journal.status);
                eprint!("{}", render_plan_human(&verified));
            }
        }
        Command::Verify => {
            let plan = runtime.verify()?;
            emit(&runtime, &plan, cli.json)?;
        }
    }
    Ok(())
}

fn emit(
    runtime: &Runtime<AdbBackend<SystemRunner>>,
    plan: &ExecutionPlan,
    json: bool,
) -> Result<()> {
    let rendered = runtime.render_plan(plan, json)?;
    if json {
        print!("{rendered}");
    } else {
        eprint!("{rendered}");
    }
    Ok(())
}
