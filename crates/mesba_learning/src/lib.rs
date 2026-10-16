// ============================================================================
// mesba_learning —— MesBA 六种学习算法（设计文档 §10）
// ============================================================================
// 独立算法（6 种）：
//   hebbian      —— 连接权重塑形（一起激活的连在一起）
//   threshold    —— 错误驱动阈值修正（正确→更易激活 / 错误→更难激活）
//   pruning      —— 低权重弱门控连接修剪
//   chunking     —— 共现发现 + Composite 区块组块化
//   abstraction  —— 抽象发现算法（Experience → Abstract 模式骨架）
//   analogy      —— 类比映射算法（Abstract → 目标领域 Diagonal 连接）
//
// 依赖只到 mesba_core + mesba_network（避免循环依赖）
// 上层 mesba_free::transfer::tick() 回调这里
// ============================================================================

pub mod abstraction;
pub mod analogy;
pub mod chunking;
pub mod hebbian;
pub mod pruning;
pub mod threshold;

// re-export 常用入口
pub use abstraction::{
    AbstractionCandidate, discover_abstractions, register_abstraction,
};
pub use analogy::{
    map_all_analogies, map_analogies, score_analogy,
};
pub use chunking::{
    ChunkStats, chunk_network, collect_co_activation_pairs,
    find_chunk_candidates,
};
pub use hebbian::{
    HebbianStats, DEFAULT_LEARN_RATE, DEFAULT_WEAK_CONNECTION_WEIGHT,
    hebbian_ensure_connections, hebbian_reinforce,
};
pub use pruning::{DEFAULT_PRUNE_THRESHOLD, PruneStats, prune_weak_connections};
pub use threshold::{
    MAX_THRESHOLD, MIN_THRESHOLD, THRESHOLD_STEP, ThresholdAdjustStats,
    adjust_from_error_signal, adjust_on_feedback,
};
