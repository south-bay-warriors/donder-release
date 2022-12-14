use std::process;
use clap::Parser;
use anyhow::Result;
use chrono::Local;

macro_rules! logInfo {
    ($($arg:tt)*) => ({
        let time = Local::now().format("[%T%.3f]");
        println!("{} Info: {}", time, format_args!($($arg)*));
    })
}

macro_rules! logError {
    ($($arg:tt)*) => ({
        let time = Local::now().format("[%T%.3f]");
        eprintln!("{} Error: {}", time, format_args!($($arg)*));
    })
}

#[macro_use]
mod ctx;
mod git;
mod api;
mod changelog;
mod bump_files;
mod package;

use ctx::Ctx;

/// donder-release CLI
/// - Quickly create releases on Github from the command line or CI using conventional commits.
#[derive(Parser)]
struct Cli {
    /// Initialize configuration file
    #[clap(short, long, default_value = "false")]
    init: bool,
    /// Configuration file path
    #[arg(long, short, default_value = "donder-release.yaml")]
    config: String,
    /// If you have a monorepo and want to release a specific package
    #[arg(long, short, required = false, value_delimiter = ',')]
    packages: Vec<String>,
    /// Release optional pre ID (e.g: alpha, beta, rc)
    #[arg(long, default_value = "")]
    pre_id: String,
    /// Preview a pending release without publishing it
    #[arg(long, default_value = "false")]
    dry_run: bool,
    /// Outputs the CLI version
    #[arg(long, short, default_value = "false")]
    version: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    if args.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Output CLI version
    if args.init {
        ctx::init_config().unwrap_or_else(|e| {
            logError!("Initializing config - {}", e.to_string());
            process::exit(1);
        });
        return Ok(());
    }

    // Load configuration file into context
    let ctx = Ctx::new(args.config, args.pre_id, args.dry_run, args.packages)
        .unwrap_or_else(|e| {
            logError!("Loading configuration - {}", e.to_string());
            process::exit(1);
        });

    // Sync local git repo with remote
    ctx.git.sync().unwrap_or_else(|e| {
        logError!("Syncing git repo - {}", e.to_string());
        process::exit(1);
    });

    // Log mode
    match ctx.preview {
        true => logInfo!("Running in preview mode, release will not be published"),
        false => logInfo!("Running in publish mode, release will be published"),
    }

    for mut pkg in ctx.packages {
        if !pkg.name.is_empty() {
            logInfo!("Processing package {}", pkg.name);
        }

        // Get last release info
        pkg.last_release(&ctx.git, &ctx.pre_id).unwrap_or_else(|e| {
            logError!("Getting last release - {}", e.to_string());
            process::exit(1);
        });

        // Get commits
        pkg.get_commits(&ctx.git).unwrap_or_else(|e| {
            logError!("Getting commits - {}", e.to_string());
            process::exit(1);
        });

        
        // Generate changelog
        let has_changelog = pkg.load_changelog(&ctx.pre_id, &ctx.types)
            .unwrap_or_else(|e| {
                logError!("Generating changelog - {}", e.to_string());
                process::exit(1);
            });

        if has_changelog {
            // Write release notes
            pkg.write_notes(&ctx.preview, &ctx.git, &ctx.types, &ctx.changelog_file)
                .unwrap_or_else(|e| {
                    logError!("Writing release notes - {}", e.to_string());
                    process::exit(1);
                });
        
            // Publish or preview release
            match ctx.preview {
                true => {
                    logInfo!("Previewing release");
                    
                    for line in pkg.changelog.notes.lines() {
                        println!("{}", line);
                    }

                    println!();
                },
                false => {
                    // Bump files
                    pkg.bump_files()
                        .unwrap_or_else(|e| {
                            logError!("Bumping files - {}", e.to_string());
                            process::exit(1);
                        });
        
                    // Publish release
                    pkg.publish_release(&ctx.git, &ctx.api, &ctx.release_message)
                        .await
                        .unwrap_or_else(|e| {
                            logError!("Publishing release - {}", e.to_string());
                            process::exit(1);
                        });
                }
            }
        }
    }

    logInfo!("completed successfully 🎉");

    Ok(())
}
