// ============================================================================
// topology.rs —— 网状区块网络拓扑管理
// ============================================================================
// 设计文档 §5.3 连接拓扑规则明确要求：
//   1. 纵向树：每个子区块有且仅有一个父区块
//   2. 横向网：同级子区块间可自由建立横向连接
//   3. 斜向桥：跨领域/层级少量斜向连接（类比迁移的桥梁）
//   4. 时序链：规则区块步骤间时序连接形成执行链
//   5. 回环控：回环连接必须有门控
//   6. 度约束：原子块≤32连接，领域块≤256连接
//   7. 权重衰减：长期未激活的连接权重自然衰减
//
// 本模块提供拓扑验证、度约束、纵向树规则、修剪辅助等函数
// ============================================================================

use mesba_core::{BlockId, BlockType};

/// 原子区块（Fact/Judgment/Perceptual/Operation/Experience）最大连接数
pub const ATOMIC_MAX_DEGREE: usize = 32;

/// 领域区块（Concept/Rule/Plan/Abstract）最大连接数
pub const DOMAIN_MAX_DEGREE: usize = 256;

/// =========================================================================
/// 度约束检查：区块当前连接数是否已达上限
/// =========================================================================
pub fn is_degree_exceeded(block_type: BlockType, current_degree: usize) -> bool {
    let limit = match block_type {
        BlockType::Fact
        | BlockType::Judgment
        | BlockType::Perceptual
        | BlockType::Operation
        | BlockType::Experience => ATOMIC_MAX_DEGREE,
        _ => DOMAIN_MAX_DEGREE,
    };
    current_degree >= limit
}

/// =========================================================================
/// 纵向树规则：父区块唯一性验证
/// =========================================================================
/// 每个子区块只能有一个纵向父区块。
/// 返回 true 表示 target 当前无纵向入边，可以建立新纵向连接。
pub fn can_add_vertical(has_parent: bool) -> bool {
    !has_parent
}

/// =========================================================================
/// 连接创建规则校验 —— 综合拓扑检查
/// =========================================================================
pub enum TopologyViolation {
    /// 子区块已有父区块（纵向树唯一性）
    MultipleParents { child: BlockId },
    /// 区块度数超限
    DegreeExceeded { block: BlockId, limit: usize, current: usize },
    /// 回环连接未设置门控
    LoopWithoutGate,
    /// 斜向连接比例超过允许上限
    TooManyDiagonal,
}

/// =========================================================================
/// 度约束衰减 —— 长期未使用连接权重自然衰减
/// =========================================================================
/// 模拟遗忘机制：连接的 last_updated_step 远离当前 step 时，
/// 权重按 ATTIENATION_RATE 每步自然衰减。
pub const DECAY_RATE_PER_STEP: u8 = 1; // 每步减 1（uint8 饱和减）
pub const MAX_STEPS_BEFORE_DECAY: u64 = 1000; // 超过 1000 步未激活开始衰减
