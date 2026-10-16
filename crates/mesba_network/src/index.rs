// ============================================================================
// index.rs —— 网络查询索引（路由/查找/快速访问）
// ============================================================================
// 设计文档 §7.1 MBN 递归嵌套结构要求快速查询能力：
//   - 按域名前缀查找（如 "Math.*" → 所有数学领域区块）
//   - 按类型批量查找（如 所有规则区块）
//   - 按标签查找（如 所有带 "addition" 标签的区块）
//   - 子节点查找（父区块 → 所有纵向子区块）
//
// MeshBlockNetwork 内部已实现这些索引，本模块提供辅助构建函数和查询封装
// ============================================================================

use std::collections::HashMap;

use mesba_core::{BlockId, BlockType, ConnectionId};

/// =========================================================================
/// RouteIndex —— 快速路由索引：按领域 + 类型 + 标签三维索引
/// =========================================================================
/// 用于游离机制 Router 快速查找"处理数学加法问题的最佳初始注入点"
#[derive(Debug, Default, Clone)]
pub struct RouteIndex {
    /// (domain, block_type) → Vec<BlockId>
    domain_type: HashMap<(String, BlockType), Vec<BlockId>>,
    /// tag → Vec<BlockId>
    tag_to_blocks: HashMap<String, Vec<BlockId>>,
    /// block → [conn_ids]（邻居快速访问，由 network.connect() 时注册）
    #[allow(dead_code)]  // MVP 阶段未用，生产路由查询时启用
    block_neighbors: HashMap<BlockId, Vec<ConnectionId>>,
}

impl RouteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, block_id: BlockId, domain: &str, block_type: BlockType, tags: &[String]) {
        self.domain_type
            .entry((domain.to_string(), block_type))
            .or_default()
            .push(block_id.clone());
        for tag in tags {
            self.tag_to_blocks
                .entry(tag.clone())
                .or_default()
                .push(block_id.clone());
        }
    }

    pub fn find_by_domain_type(&self, domain: &str, block_type: BlockType) -> Vec<&BlockId> {
        self.domain_type
            .get(&(domain.to_string(), block_type))
            .map(|v| v.iter())
            .unwrap_or_default()
            .collect()
    }

    pub fn find_by_tag(&self, tag: &str) -> Vec<&BlockId> {
        self.tag_to_blocks.get(tag).map(|v| v.iter()).unwrap_or_default().collect()
    }
}
