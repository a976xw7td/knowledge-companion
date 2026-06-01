//! kcctl — KnowledgeCompanion CLI for maintenance and diagnostics.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use knowledge_companion::{config, db, init_logging};

// ── Terminal colors (no dependencies) ──────────────────────────────────────
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn ok(s: &str) -> String {
    format!("{GREEN}{s}{RESET}")
}
fn fail(s: &str) -> String {
    format!("{RED}{s}{RESET}")
}
fn warn(s: &str) -> String {
    format!("{YELLOW}{s}{RESET}")
}
#[allow(dead_code)]
fn info(s: &str) -> String {
    format!("{CYAN}{s}{RESET}")
}

#[derive(Parser)]
#[command(name = "kcctl", version, about = "KnowledgeCompanion maintenance CLI")]
struct Cli {
    /// Override bundle root path
    #[arg(long, global = true)]
    bundle_root: Option<String>,

    /// JSON output mode
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the KnowledgeSuite directory structure
    Init,
    /// Run health check
    Health,
    /// Run comprehensive diagnostics
    Doctor,
    /// Export sanitized diagnostics bundle
    #[command(name = "diagnostics")]
    Diagnostics {
        /// Output zip file path
        output: String,
    },
    /// Check SQLite database integrity
    #[command(name = "db")]
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Show bundle root and config info
    Info,
    /// Trigger manual sync of all watched roots
    Sync,
    /// Embedded job management
    #[command(subcommand)]
    Jobs(JobCommand),
    /// Start HTTP MCP server (requires [http_mcp] enabled=true in config)
    #[command(name = "serve-http")]
    ServeHttp,
    /// Watch knowledge roots for file changes and auto-sync
    Watch,
}

#[derive(Subcommand)]
enum JobCommand {
    /// Show embedding job statistics
    Status,
    /// Process one batch of pending embedding jobs
    #[command(name = "run-once")]
    RunOnce,
}

#[derive(Subcommand)]
enum DbAction {
    /// Run PRAGMA integrity_check
    IntegrityCheck,
    /// Show database statistics
    Stats,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Detect bundle root
    let bundle_root = if let Some(ref path) = cli.bundle_root {
        std::env::set_var("KC_BUNDLE_ROOT", path);
        config::bundle::detect_bundle_root()?
    } else {
        config::bundle::detect_bundle_root()
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| "/".into()))
    };

    // Setup logging
    let cfg = config::bundle::load_config(&bundle_root).unwrap_or_default();
    let log_dir = config::bundle::resolve_path(&bundle_root, &cfg.storage.log_dir);
    std::fs::create_dir_all(&log_dir).ok();
    init_logging(&log_dir);

    match cli.command {
        Commands::Init => cmd_init(&bundle_root, cli.json),
        Commands::Health => cmd_health(&bundle_root, cli.json),
        Commands::Doctor => cmd_doctor(&bundle_root, cli.json),
        Commands::Diagnostics { output } => cmd_diagnostics(&bundle_root, &output, cli.json),
        Commands::Db { action } => cmd_db(&bundle_root, action, cli.json),
        Commands::Info => cmd_info(&bundle_root, cli.json),
        Commands::Sync => cmd_sync(&bundle_root, cli.json),
        Commands::Jobs(cmd) => cmd_jobs(&bundle_root, cmd, cli.json),
        Commands::ServeHttp => cmd_serve_http(&bundle_root),
        Commands::Watch => cmd_watch(&bundle_root),
    }
}

fn cmd_init(bundle_root: &std::path::Path, json: bool) -> Result<()> {
    let dirs = [
        "data/logs",
        "data/cache",
        "knowledge",
        "workspace/.claw",
        "scripts",
        "skills",
    ];

    let mut created = Vec::new();
    for d in &dirs {
        let path = bundle_root.join(d);
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
            created.push(d.to_string());
        }
    }

    let config_path = bundle_root.join("config/knowledge-companion.toml");
    if !config_path.exists() {
        std::fs::create_dir_all(config_path.parent().unwrap())?;
        std::fs::write(
            &config_path,
            include_str!("../../config/knowledge-companion.toml"),
        )?;
        created.push("config/knowledge-companion.toml".to_string());
    }

    if json {
        println!(
            "{}",
            serde_json::json!({"status": "ok", "created": created, "bundle_root": bundle_root})
        );
    } else {
        println!("KnowledgeSuite initialized at {}", bundle_root.display());
        for d in &created {
            println!("  created: {}", d);
        }
    }
    Ok(())
}

fn cmd_health(_bundle_root: &std::path::Path, json: bool) -> Result<()> {
    let health = knowledge_companion::services::health::check_health();
    if json {
        println!("{}", serde_json::to_string_pretty(&health)?);
    } else {
        println!("Version:     {}", health.version);
        println!("Bundle root: {}", health.bundle_root);
        let status_color = if health.status == "ok" {
            ok(&health.status)
        } else {
            fail(&health.status)
        };
        println!("Status:      {}", status_color);
        println!(
            "DB:          {}",
            health.db_status.as_deref().unwrap_or("unknown")
        );
        println!(
            "Config:      {}",
            if health.config_loaded.unwrap_or(false) {
                "loaded"
            } else {
                "not loaded"
            }
        );
        println!(
            "Knowledge:   {}",
            if health.knowledge_dir_exists.unwrap_or(false) {
                "exists"
            } else {
                "missing"
            }
        );
        println!(
            "Data writable: {}",
            if health.data_dir_writable.unwrap_or(false) {
                "yes"
            } else {
                "no"
            }
        );
        if let Some(ref errors) = health.errors {
            for e in errors {
                println!("  ERROR: {}", e);
            }
        }
    }
    Ok(())
}

fn cmd_doctor(bundle_root: &std::path::Path, json: bool) -> Result<()> {
    let health = knowledge_companion::services::health::check_health();
    let cfg = config::bundle::load_config(bundle_root).unwrap_or_default();

    // DB integrity
    let db_path = config::bundle::resolve_path(bundle_root, &cfg.storage.db_path);
    let db_ok = if db_path.exists() {
        db::connection::check_integrity(&db_path).is_ok()
    } else {
        false
    };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "health": health,
                "db_integrity": db_ok,
                "config_sections": {
                    "app": true,
                    "storage": true
                }
            })
        );
    } else {
        println!("=== Health ===");
        println!("  Status: {}", health.status);
        println!("  Version: {}", health.version);
        println!(
            "  Knowledge dir: {}",
            if health.knowledge_dir_exists.unwrap_or(false) {
                "ok"
            } else {
                "missing"
            }
        );

        println!("\n=== Database ===");
        println!("  Path: {}", db_path.display());
        println!("  Exists: {}", db_path.exists());
        if db_path.exists() {
            println!("  Integrity: {}", if db_ok { "ok" } else { "FAILED" });
        }

        println!("\n=== Config ===");
        println!("  App name: {}", cfg.app.name);
        println!("  Sync interval: {}s", cfg.app.sync_interval_seconds);
        println!("  Max file size: {}MB", cfg.app.max_file_size_mb);
    }

    if health.status != "ok" {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_diagnostics(bundle_root: &std::path::Path, output: &str, _json: bool) -> Result<()> {
    let health = knowledge_companion::services::health::check_health();

    // Build a JSON diagnostics report (no secrets, no user content)
    let report = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "bundle_root": bundle_root,
        "health": {
            "status": health.status,
            "knowledge_dir_exists": health.knowledge_dir_exists,
            "data_dir_writable": health.data_dir_writable,
            "config_loaded": health.config_loaded,
            "db_status": health.db_status,
        }
    });

    // Write as JSON (not ZIP for now, zip support can come later)
    std::fs::write(output, serde_json::to_string_pretty(&report)?)?;
    println!("Diagnostics exported to {}", output);
    Ok(())
}

fn cmd_db(bundle_root: &std::path::Path, action: DbAction, json: bool) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root).unwrap_or_default();
    let db_path = config::bundle::resolve_path(bundle_root, &cfg.storage.db_path);

    match action {
        DbAction::IntegrityCheck => match db::connection::check_integrity(&db_path) {
            Ok(_) => {
                if json {
                    println!("{}", serde_json::json!({"status": "ok"}));
                } else {
                    println!("Database integrity: ok");
                }
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "error", "message": e.to_string()})
                    );
                } else {
                    println!("Database integrity: FAILED — {}", e);
                }
                std::process::exit(1);
            }
        },
        DbAction::Stats => {
            let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
            if json {
                println!(
                    "{}",
                    serde_json::json!({"path": db_path, "size_bytes": size})
                );
            } else {
                println!("Database path: {}", db_path.display());
                println!("Size: {} bytes ({:.1} MB)", size, size as f64 / 1_048_576.0);
            }
        }
    }
    Ok(())
}

fn cmd_sync(bundle_root: &std::path::Path, json: bool) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root).unwrap_or_default();
    let db_path = config::bundle::resolve_path(bundle_root, &cfg.storage.db_path);
    let conn =
        knowledge_companion::db::connection::open(&db_path).context("Failed to open database")?;

    let results = knowledge_companion::sync::sync_all(&conn, bundle_root)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for r in &results {
            let status = if r.failed > 0 { fail("✗") } else { ok("✓") };
            println!(
                " {status} {BOLD}{}{RESET}  {GREEN}+{}{RESET}  {YELLOW}~{}{RESET}  {RED}-{}{RESET}  {CYAN}{}ms{RESET}",
                r.root_name, r.created, r.modified, r.deleted, r.duration_ms
            );
            if r.failed > 0 {
                println!("    {} {} file(s) failed", warn("!"), r.failed);
            }
        }
    }
    Ok(())
}

fn cmd_info(bundle_root: &std::path::Path, json: bool) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root).unwrap_or_default();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "bundle_root": bundle_root,
                "version": env!("CARGO_PKG_VERSION"),
                "config_name": cfg.app.name,
            })
        );
    } else {
        println!("KnowledgeSuite Info");
        println!("  Bundle root:  {}", bundle_root.display());
        println!("  Version:      {}", env!("CARGO_PKG_VERSION"));
        println!("  App name:     {}", cfg.app.name);
    }
    Ok(())
}

fn cmd_serve_http(bundle_root: &std::path::Path) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root)?;
    if !cfg.http_mcp.enabled {
        anyhow::bail!("HTTP MCP is not enabled. Set [http_mcp] enabled=true in config.");
    }
    println!(
        "Starting HTTP MCP server on {}:{}",
        cfg.http_mcp.bind, cfg.http_mcp.port
    );
    if cfg.http_mcp.bind != "127.0.0.1"
        && cfg.http_mcp.bind != "localhost"
        && cfg.http_mcp.bind != "::1"
    {
        println!("⚠ WARNING: Binding to non-loopback address without TLS. Use with caution.");
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(knowledge_companion::http::serve(cfg.http_mcp))
}

fn cmd_watch(bundle_root: &std::path::Path) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root)?;
    let db_path = config::bundle::resolve_path(bundle_root, &cfg.storage.db_path);
    let root_clone = bundle_root.to_path_buf();

    println!("Starting file watcher on enabled knowledge roots...");
    println!("Press Ctrl+C to stop.");
    println!();

    knowledge_companion::sync::watcher::watch_all(5000, move || {
        if let Ok(conn) = knowledge_companion::db::connection::open(&db_path) {
            match knowledge_companion::sync::sync_all(&conn, &root_clone) {
                Ok(results) => {
                    for r in &results {
                        if r.created + r.modified + r.deleted + r.failed > 0 {
                            println!(
                                "{CYAN}[auto-sync]{RESET} {BOLD}{}{RESET}  {GREEN}+{}{RESET}  {YELLOW}~{}{RESET}  {RED}-{}{RESET}",
                                r.root_name, r.created, r.modified, r.deleted
                            );
                        }
                    }
                }
                Err(e) => eprintln!("[auto-sync] Error: {}", e),
            }
        }
    })?;

    std::thread::park();
    Ok(())
}

fn cmd_jobs(bundle_root: &std::path::Path, cmd: JobCommand, json: bool) -> Result<()> {
    let cfg = config::bundle::load_config(bundle_root).unwrap_or_default();
    let db_path = config::bundle::resolve_path(bundle_root, &cfg.storage.db_path);
    let conn = knowledge_companion::db::connection::open(&db_path).context("DB open")?;

    match cmd {
        JobCommand::Status => {
            let stats = knowledge_companion::sync::jobs::job_stats(&conn)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                println!("Embedding Jobs:");
                if let Some(p) = stats.get("pending") {
                    println!("  pending:    {}", p);
                }
                if let Some(p) = stats.get("processing") {
                    println!("  processing: {}", p);
                }
                if let Some(p) = stats.get("completed") {
                    println!("  completed:  {}", p);
                }
                if let Some(p) = stats.get("failed") {
                    println!("  failed:     {}", p);
                }
            }
        }
        JobCommand::RunOnce => {
            println!("Processing one batch of embedding jobs...");
            let jobs = knowledge_companion::sync::jobs::claim_pending_jobs(&conn, 8)?;
            if jobs.is_empty() {
                println!("No pending jobs.");
                return Ok(());
            }
            println!("Claimed {} jobs", jobs.len());
            // Process each job
            for job in &jobs {
                knowledge_companion::sync::jobs::mark_job_started(&conn, &job.id)?;
                let chunk_id = &job.chunk_id;
                // Get chunk content
                let content: Option<String> = conn
                    .query_row("SELECT content FROM chunks WHERE id=?1", [chunk_id], |r| {
                        r.get(0)
                    })
                    .ok();
                if let Some(text) = content {
                    match build_embedder_for_jobs(bundle_root) {
                        Some(embedder) => match embedder.embed_sync(&[text]) {
                            Ok(embeddings) => {
                                if let Some(emb) = embeddings.first() {
                                    if let Err(e) =
                                        knowledge_companion::index::vector::store_embedding(
                                            &conn, chunk_id, emb,
                                        )
                                    {
                                        eprintln!("  FAIL {}: {}", &job.id[..8], e);
                                        knowledge_companion::sync::jobs::mark_job_failed(
                                            &conn,
                                            &job.id,
                                            &format!("Store: {}", e),
                                            3,
                                        )?;
                                    } else {
                                        knowledge_companion::sync::jobs::mark_job_done(
                                            &conn, &job.id, chunk_id,
                                        )?;
                                        println!("  OK   {}", &job.id[..8]);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  RETRY {}: {}", &job.id[..8], e);
                                knowledge_companion::sync::jobs::mark_job_failed(
                                    &conn,
                                    &job.id,
                                    &format!("API: {}", e),
                                    3,
                                )?;
                            }
                        },
                        None => {
                            knowledge_companion::sync::jobs::mark_job_failed(
                                &conn,
                                &job.id,
                                "No embedder config",
                                3,
                            )?;
                        }
                    }
                } else {
                    knowledge_companion::sync::jobs::mark_job_failed(
                        &conn,
                        &job.id,
                        "Chunk not found",
                        3,
                    )?;
                }
            }
            println!("Done.");
        }
    }
    Ok(())
}

fn build_embedder_for_jobs(
    bundle_root: &std::path::Path,
) -> Option<knowledge_companion::index::embed::RemoteEmbedder> {
    let config = config::bundle::load_config(bundle_root).ok()?;
    if !config.embedding.enabled || config.embedding.api_key_env.is_empty() {
        return None;
    }
    let key = std::env::var(&config.embedding.api_key_env).ok()?;
    if key.is_empty() {
        return None;
    }
    Some(knowledge_companion::index::embed::RemoteEmbedder::new(
        knowledge_companion::index::embed::EmbedConfig {
            base_url: config.embedding.base_url,
            api_key: key,
            model: config.embedding.model,
            dimensions: config.embedding.dimensions,
            timeout_seconds: config.embedding.timeout_seconds,
            batch_size: config.embedding.batch_size,
        },
    ))
}
