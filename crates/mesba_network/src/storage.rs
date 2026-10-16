// ============================================================================
// storage.rs —— 存储引擎（L1/L2 分层）
// ============================================================================
// 设计文档 §11.1 存储层级：
//   L0 寄存器级  → 脉冲缓存/目标栈/情境模型（几十KB，纳秒级）
//   L1 内存级    → 当前激活区块 + 工作连接权重缓存（秒级）
//   L2 SSD 级    → 区块快照文件（TB 级，毫秒级）
//
// MVP 阶段：使用 serde_json 实现序列化/反序列化（最简单稳妥）
// 生产环境：切换到 bincode 2.0 + mmap2 内存映射
// ============================================================================

use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mesba_core::{Block, Connection};

/// =========================================================================
/// StorageEngine —— 存储引擎
/// =========================================================================
/// 负责网络快照的持久化和恢复。MVP 使用 JSON 格式方便调试和 diff。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEngine {
    /// 网络名称（如 "MVP_Addition_Seed"）
    pub name: String,

    /// 区块序列化数据
    pub blocks: Vec<Block>,

    /// 连接序列化数据
    pub connections: Vec<Connection>,
}

impl StorageEngine {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            blocks: Vec::new(),
            connections: Vec::new(),
        }
    }

    /// 序列化到 .mesba.json 文件
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        Ok(())
    }

    /// 从 .mesba.json 文件反序列化
    pub fn load_from_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader)
            .map_err(|e| io::Error::other(e.to_string()))
    }

    /// 文件名建议
    pub fn suggested_filename(&self) -> PathBuf {
        PathBuf::from(format!("{}.mesba.json", self.name))
    }
}
