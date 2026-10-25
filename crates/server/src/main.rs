//! `me-server` command-line entry point.

use anyhow::{Result, bail};
use clap::Parser;
use me_server::config::{
    BackupArgs, CheckArgs, Cli, Command, Config, ImportArgs, InitArgs, generate_password,
    resolve_db_path,
};
use me_server::data::{check_all, init_all};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();

    match cli.command {
        Command::Serve(args) => {
            let config = Config::from_args(&args)?;
            me_server::serve(config).await?;
        }
        Command::Init(args) => run_init(&args)?,
        Command::Check(args) => run_check(&args)?,
        Command::Import(args) => run_import(&args)?,
        Command::Backup(args) => run_backup(&args)?,
        Command::Password => println!("{}", generate_password()),
    }

    Ok(())
}

fn run_init(args: &InitArgs) -> Result<()> {
    let db_path = resolve_db_path(args.db.clone());
    let store = me_store::SqliteStore::open(&db_path)?;
    let results = init_all(&store)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "succeeded",
            "db": db_path,
            "initialized": results,
        }))?
    );
    Ok(())
}

fn run_check(args: &CheckArgs) -> Result<()> {
    let db_path = resolve_db_path(args.db.clone());
    let store = me_store::SqliteStore::open_existing(&db_path)?;
    let result = check_all(&store);
    println!("{}", serde_json::to_string_pretty(&result)?);

    let all_ok = result
        .get("checks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|checks| {
            checks
                .iter()
                .all(|check| check.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        });
    if !all_ok {
        bail!("one or more data files failed validation");
    }
    Ok(())
}

fn run_import(args: &ImportArgs) -> Result<()> {
    let db_path = resolve_db_path(args.db.clone());
    let store = me_store::SqliteStore::open(&db_path)?;
    let imported = me_server::import::import_data_dir(&store, &args.from, args.replace)?;
    // Fill any collection the legacy tree did not contain, so the database is
    // immediately servable and `check` covers all resources.
    let initialized = init_all(&store)?;
    let checks = check_all(&store);
    let all_ok = checks
        .get("checks")
        .and_then(Value::as_array)
        .is_some_and(|checks| {
            checks
                .iter()
                .all(|check| check.get("ok").and_then(Value::as_bool) == Some(true))
        });

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "succeeded",
            "db": db_path,
            "imported": imported,
            "initialized": initialized,
            "checks": checks,
        }))?
    );

    if !all_ok {
        bail!("import finished but one or more collections failed validation");
    }
    Ok(())
}

fn run_backup(args: &BackupArgs) -> Result<()> {
    let db_path = resolve_db_path(args.db.clone());
    let store = me_store::SqliteStore::open_existing(&db_path)?;
    store.backup_to(&args.to)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "succeeded",
            "db": db_path,
            "backup": args.to,
        }))?
    );
    Ok(())
}

/// Initialize tracing once. Logs go to stderr so stdout stays machine-readable.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "me_server=info,tower_http=info".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
