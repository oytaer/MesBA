// ============================================================================
// mesba_cli —— MesBA 命令行工具
// ============================================================================
// 设计文档 §16：顶层 CLI 组装全部 crates，统一 JSON 输出
// 模块划分：
//   lib.rs  — clap Parser + run() 主分发（所有可测试逻辑放这里）
//   main.rs — 薄入口，只调用 cli::run()（< 10 行）
//
// 子命令：
//   add <a> <b>      — 便捷加法（默认种子网络）
//   query            — JSON 推理入口（--input / 从 stdin 读 JSON）
//   version          — 版本信息
// ============================================================================

use std::collections::HashMap;
use std::io::{self, Read};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mesba_runtime::InferenceRuntime;

// ============================================================================
// CLI 入口：clap derive
// ============================================================================
#[derive(Debug, Parser)]
#[command(
    name = "mesba",
    about = "MesBA CLI — Mesh Block Architecture v2",
    version,
    long_about = "网状区块架构 v2 — 一个受生物启发的分布式知识表示与推理系统"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 便捷：单数字加法（默认种子网络，直接输出 JSON）
    Add {
        /// 第一个数字（0~9）
        #[arg(value_name = "A")]
        a: i32,
        /// 第二个数字（0~9）
        #[arg(value_name = "B")]
        b: i32,
    },

    /// 通用推理：从 --input 或 stdin 读 JSON 变量 → 输出 JSON 结果
    Query {
        /// 输入 JSON 文件路径；不指定则从 stdin 读
        #[arg(short = 'i', long = "input", value_name = "FILE")]
        input: Option<String>,

        /// 覆盖最大 tick 数（防死循环）
        #[arg(short = 't', long = "max-ticks", default_value_t = 200)]
        max_ticks: u64,
    },

    /// 打印版本信息
    Version,
}

// ============================================================================
// run —— 主分发逻辑（lib.rs 里可测试）
// ============================================================================
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Add { a, b } => handle_add(a, b),
        Command::Query { input, max_ticks } => handle_query(input, max_ticks),
        Command::Version => handle_version(),
    }
}

// ============================================================================
// handle_add —— 加法便捷子命令
// ============================================================================
fn handle_add(a: i32, b: i32) -> Result<()> {
    let mut rt = InferenceRuntime::default_seed_addition();
    let resp = rt.run_add(a, b);
    println!("{}", resp.to_json());
    Ok(())
}

// ============================================================================
// handle_query —— 通用推理（读 JSON → 输出 JSON）
// ============================================================================
fn handle_query(input_path: Option<String>, max_ticks: u64) -> Result<()> {
    // 1) 读取输入 JSON
    let json_str = match input_path {
        Some(path) => std::fs::read_to_string(&path)
            .with_context(|| format!("无法读取输入文件: {}", path))?,
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .context("无法从 stdin 读取 JSON 输入")?;
            buf
        }
    };

    // 2) 解析为 HashMap<String, i32>
    let input: HashMap<String, i32> = serde_json::from_str(&json_str)
        .with_context(|| format!("JSON 解析失败: {}", json_str.trim()))?;

    // 3) 运行推理
    let mut rt = InferenceRuntime::default_seed_addition();
    rt.set_max_ticks(max_ticks);
    let resp = rt.run_query(input);

    // 4) 输出 JSON
    println!("{}", resp.to_json());
    Ok(())
}

// ============================================================================
// handle_version —— 版本信息
// ============================================================================
fn handle_version() -> Result<()> {
    println!("MesBA v2 — 网状区块架构 CLI");
    println!("  mesba_core:      {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_runtime:   {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_free:      {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_engine:    {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_memory:    {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_network:   {}", env!("CARGO_PKG_VERSION"));
    println!("  mesba_learning:  {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_3_5() {
        // 不通过 CLI，直接调 InferenceRuntime 验证
        let mut rt = InferenceRuntime::default_seed_addition();
        let resp = rt.run_add(3, 5);
        assert_eq!(resp.code, crate::api::CODE_OK);
        assert_eq!(resp.data.final_result, Some(8));
    }

    #[test]
    fn test_add_json_format() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let resp = rt.run_add(7, 2);
        let json = resp.to_json();
        // ApiResponse 用 #[serde(flatten)] pub data: T → data 字段被展开到顶层
        assert!(json.contains("\"code\":200"));
        assert!(json.contains("\"msg\":"));
        assert!(json.contains("\"time\":"));
        assert!(json.contains("\"final_result\":9")); // data.final_result 直接在顶层
    }

    #[test]
    fn test_query_parses_json_input() {
        let json = r#"{"a": 4, "b": 6}"#;
        let input: HashMap<String, i32> = serde_json::from_str(json).unwrap();
        assert_eq!(input.get("a"), Some(&4));
        assert_eq!(input.get("b"), Some(&6));
    }

    #[test]
    fn test_cli_parses_add_subcommand() {
        use clap::Parser;
        let args = vec!["mesba", "add", "3", "5"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Command::Add { a: 3, b: 5 }));
    }

    #[test]
    fn test_cli_parses_version() {
        use clap::Parser;
        let args = vec!["mesba", "version"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn test_cli_parses_query() {
        use clap::Parser;
        let args = vec!["mesba", "query", "-i", "input.json", "-t", "50"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(
            cli.command,
            Command::Query {
                input: Some(_),
                max_ticks: 50,
            }
        ));
    }
}

// ============================================================================
// 重导出（测试用）
// ============================================================================
#[cfg(test)]
pub(crate) mod api {
    pub use mesba_runtime::api::CODE_OK;
}
