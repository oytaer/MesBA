// ============================================================================
// id.rs —— 区块ID / 连接ID / 脉冲ID 类型定义
// ============================================================================
// 功能：为区块、连接、脉冲提供类型安全的标识符
// 设计原则：
//   1. BlockId 采用分层域名格式："领域.子域.类型.名称"（如 Math.Arithmetic.Fact.Add_9_9）
//   2. ConnectionId 和 PulseId 用 u64 自增 ID，高效且紧凑
//   3. 所有 ID 类型都实现 Display 和 FromStr，便于日志和序列化
//
// 对应设计文档：
//   - 网状区块架构 v2 §4.1 Block.id 定义
// ============================================================================

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ============================================================================
// BlockId —— 区块标识符（分层域名格式）
// ============================================================================
// 采用 string 类型存储分层 ID，格式为：
//   "{domain}.{sub_domain}.{block_type}.{name}"
// 示例：
//   "Math.Arithmetic.Fact.Add_9_9"         // 事实区块
//   "Math.Arithmetic.Rule.Addition.Strategy"  // 规则区块
//   "Perceive.Text.DigitRecognizer"        // 感知区块
//   "Abstract.PositionalProcessing"        // 抽象区块
//
// 优势：
//   - 人类可读，便于调试和日志追踪
//   - 支持按域名字符串前缀进行索引查询（如查找所有 Math.* 区块）
//   - 与 MBN 递归嵌套树结构天然对齐
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockId(pub String);

impl BlockId {
    /// 构造 BlockId
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 获取完整 ID 字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 获取所属领域（第一个点分块）
    pub fn domain(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    /// 获取区块类型（最后第二个点分块位置，即"类型"段）
    /// 例如 "Math.Arithmetic.Fact.Add_9_9" → "Fact"
    pub fn block_type_segment(&self) -> &str {
        let parts: Vec<&str> = self.0.split('.').collect();
        if parts.len() >= 2 {
            parts[parts.len() - 2]
        } else {
            ""
        }
    }

    /// 是否属于某个领域（前缀匹配）
    /// 例如 is_in_domain("Math") 对 "Math.Arithmetic.Fact.Add_9_9" 返回 true
    pub fn is_in_domain(&self, domain: &str) -> bool {
        self.0.starts_with(domain)
            && (self.0.len() == domain.len()
                || self.0.as_bytes().get(domain.len()) == Some(&b'.'))
    }

    /// 检查是否是指定类型的区块（快速前缀匹配）
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }

    /// 拼接后缀形成新的 BlockId（用于查找子区块）
    pub fn with_suffix(&self, suffix: &str) -> BlockId {
        BlockId(format!("{}.{}", self.0, suffix))
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for BlockId {
    fn from(s: &str) -> Self {
        BlockId(s.to_string())
    }
}

impl From<String> for BlockId {
    fn from(s: String) -> Self {
        BlockId(s)
    }
}

impl FromStr for BlockId {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(BlockId(s.to_string()))
    }
}

// ============================================================================
// ConnectionId —— 连接标识符（u64 自增）
// ============================================================================
// 连接数量可能达到 5B（700B 目标），u64 足够覆盖。
// 用全局原子计数器生成确保唯一性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    pub const NULL: ConnectionId = ConnectionId(0);

    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn_{}", self.0)
    }
}

// ============================================================================
// PulseId —— 脉冲标识符（u64 自增）
// ============================================================================
// 每次脉冲传播都生成新的唯一 ID，用于追踪脉冲传播路径和调试。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PulseId(pub u64);

impl PulseId {
    pub const NULL: PulseId = PulseId(0);

    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for PulseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pulse_{}", self.0)
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_id_basic() {
        let id = BlockId::new("Math.Arithmetic.Fact.Add_9_9");
        assert_eq!(id.as_str(), "Math.Arithmetic.Fact.Add_9_9");
        assert_eq!(id.domain(), "Math");
        assert_eq!(id.block_type_segment(), "Fact");
    }

    #[test]
    fn test_block_id_domain_check() {
        let id = BlockId::new("Math.Arithmetic.Fact.Add_9_9");
        assert!(id.is_in_domain("Math"));
        assert!(id.is_in_domain("Math.Arithmetic"));
        assert!(!id.is_in_domain("Physics"));
        assert!(!id.is_in_domain("Math2"));  // 前缀但不是同一域
    }

    #[test]
    fn test_block_id_with_suffix() {
        let parent = BlockId::new("Math.Arithmetic");
        let child = parent.with_suffix("Fact.Add_5_3");
        assert_eq!(child.as_str(), "Math.Arithmetic.Fact.Add_5_3");
    }

    #[test]
    fn test_block_id_conversions() {
        let s = "Math.Arithmetic.Concept.Addition";
        let id: BlockId = s.into();
        assert_eq!(id.as_str(), s);
        let id2: BlockId = String::from(s).into();
        assert_eq!(id2.as_str(), s);
    }

    #[test]
    fn test_connection_id() {
        let id = ConnectionId::new(42);
        assert_eq!(id.as_u64(), 42);
        assert_eq!(format!("{}", id), "conn_42");
        assert_eq!(ConnectionId::NULL.as_u64(), 0);
    }

    #[test]
    fn test_pulse_id() {
        let id = PulseId::new(100);
        assert_eq!(id.as_u64(), 100);
        assert_eq!(format!("{}", id), "pulse_100");
    }
}
