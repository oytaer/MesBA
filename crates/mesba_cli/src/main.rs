// ============================================================================
// mesba_cli main.rs —— 薄入口，只调用 cli::run()
// ============================================================================

use anyhow::Result;
use clap::Parser;
use mesba_cli::{Cli, run};

fn main() -> Result<()> {
    // MVP：不启用 tracing-subscriber（避免阻塞 stdio）；需要时加 --verbose
    let cli = Cli::parse();
    run(cli)
}
