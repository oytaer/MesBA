// ============================================================================
// safety.rs —— 安全门控 + 完整 Checkpoint 快照（§14.2）
// ============================================================================
// 设计文档要求：
//   §14.2 SecurityGate —— 在 Propagator Pass 1 前做安全检查
//   §14.2 Checkpoint 完整结构（snapshot_id + pulse_buffer + active_path + timestamp）
//
// 核心职责：
//   1. SecurityGate —— 安全门控配置 + 检查方法
//   2. Checkpoint —— 完整可序列化快照（active_path 用 TraceRecord 而非 TraceEntry
//      因为 TraceEntry 没有 Serialize 实现，我们用简化的 String 记录 block_id）
//   3. SecurityViolation —— 违规枚举（供上层生成 ErrorPulse）
// ============================================================================

use mesba_core::MemoryBackend;
use mesba_memory::GlobalWorkspace;
use mesba_network::MeshBlockNetwork;
use serde::{Deserialize, Serialize};

// ============================================================================
// SecurityViolation —— 违规类型
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecurityViolation {
    /// 超过单次会话最大 tick 数（防死循环）
    ExceedMaxTicks {
        current: u64,
        limit: u64,
    },
    /// 单次推理激活区块数过多（可能网络异常膨胀）
    ExceedMaxBlocksActivated {
        current: usize,
        limit: u64,
    },
    /// 网络单 block outgoing 连接过多（星型攻击）
    ExceedMaxConnections {
        block_id: String,
        connections: usize,
        limit: u64,
    },
}

impl SecurityViolation {
    /// 生成 ErrorPulse 的 strength（根据违规严重程度）
    pub fn severity_strength(&self) -> u8 {
        match self {
            SecurityViolation::ExceedMaxTicks { .. } => 255,
            SecurityViolation::ExceedMaxBlocksActivated { .. } => 200,
            SecurityViolation::ExceedMaxConnections { .. } => 180,
        }
    }

    /// 简短描述（供 ErrorPulse.payload 使用）
    pub fn short_msg(&self) -> String {
        match self {
            SecurityViolation::ExceedMaxTicks { current, limit } => {
                format!("ticks={} > max={}", current, limit)
            }
            SecurityViolation::ExceedMaxBlocksActivated { current, limit } => {
                format!("activated_blocks={} > max={}", current, limit)
            }
            SecurityViolation::ExceedMaxConnections { block_id, connections, limit } => {
                format!("block={} connections={} > max={}", block_id, connections, limit)
            }
        }
    }
}

// ============================================================================
// SecurityGate —— 安全门控配置 + 检查方法
// ============================================================================
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SecurityGate {
    /// 单次会话最大 tick 数（默认 200，与 System2 上限一致）
    pub max_ticks_per_session: u64,
    /// 单次推理最大激活区块数（默认 100，防止网络异常膨胀）
    pub max_blocks_activated: u64,
    /// 每个 block 最多 outgoing 连接数（默认 1000，防星型攻击）
    pub max_connections_per_block: u64,
}

impl Default for SecurityGate {
    fn default() -> Self {
        Self {
            max_ticks_per_session: 200,
            // seed_addition_table 约 120 blocks，设 500 留足余量
            max_blocks_activated: 500,
            // plan_single_digit 有 100+ outgoing，设 1000 防星型攻击
            max_connections_per_block: 1000,
        }
    }
}

impl SecurityGate {
    pub fn new(
        max_ticks_per_session: u64,
        max_blocks_activated: u64,
        max_connections_per_block: u64,
    ) -> Self {
        Self {
            max_ticks_per_session,
            max_blocks_activated,
            max_connections_per_block,
        }
    }

    /// ★ F-06 核心：Propagator Pass 1 前调用的安全检查
    /// 返回 Ok(()) = 安全；Err(SecurityViolation) = 发现违规
    /// Propagator / Controller 可根据违规类型决定：
    ///   - ExceedMaxTicks → 直接 kill + 生成 ErrorPulse（F-17 遗留缺口）
    ///   - ExceedMaxConnections → 静默 pruning（不 kill，只减连接）
    pub fn check_before_propagate(
        &self,
        network: &MeshBlockNetwork,
        workspace: &GlobalWorkspace,
        current_ticks: u64,
    ) -> Result<(), SecurityViolation> {
        // 1) 会话 tick 数检查
        if current_ticks >= self.max_ticks_per_session {
            return Err(SecurityViolation::ExceedMaxTicks {
                current: current_ticks,
                limit: self.max_ticks_per_session,
            });
        }

        // 2) 激活区块数检查（trace 里出现过的 block 数）
        let activated_cnt = workspace.trace().len();
        if activated_cnt > self.max_blocks_activated as usize {
            return Err(SecurityViolation::ExceedMaxBlocksActivated {
                current: activated_cnt,
                limit: self.max_blocks_activated,
            });
        }

        // 3) 星型连接检查（遍历所有 block 的 outgoing_connections）
        for (_id, block) in network.blocks() {
            let conns = network.outgoing_connections(&block.id).len();
            if conns > self.max_connections_per_block as usize {
                return Err(SecurityViolation::ExceedMaxConnections {
                    block_id: block.id.to_string(),
                    connections: conns,
                    limit: self.max_connections_per_block,
                });
            }
        }

        Ok(())
    }
}

// ============================================================================
// Checkpoint —— 完整快照结构（严格对齐设计文档 §14.2）
// ============================================================================
/// 用于调试/回放的快照（可序列化）
///
/// 设计文档 §14.2 要求字段：
///   snapshot_id / goal_stack / pulse_buffer / active_path / timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 快照唯一 ID（递增自增）
    pub snapshot_id: u64,
    /// 快照时间戳（epoch ms）
    pub timestamp: u64,
    /// 当前 GoalStack 子目标栈完整快照（设计文档 §14.2 新增）
    pub goal_stack: Vec<mesba_memory::SubGoal>,
    /// 当前 PulseBuffer 变量快照（key → value）
    pub pulse_buffer: Vec<(String, i32)>,
    /// 当前 trace 简化记录（block_id 列表）
    pub active_path: Vec<String>,
    /// 快照对应的 CognitiveRing（调试用）
    pub current_ring: String,
    /// 快照时已执行 tick 数
    pub ticks_at_checkpoint: u64,
}

impl Checkpoint {
    /// 从 GlobalWorkspace 构造快照
    pub fn new(
        snapshot_id: u64,
        workspace: &GlobalWorkspace,
        current_ring: &str,
        ticks_at_checkpoint: u64,
    ) -> Self {
        let pulse_buffer = workspace.pulse_buffer.snapshot().into_iter().collect();
        let active_path: Vec<String> = workspace
            .trace()
            .iter()
            .map(|e| e.block_id.to_string())
            .collect();
        // ★ 设计文档 §14.2：完整 GoalStack 快照（每个 SubGoal 可序列化）
        let goal_stack: Vec<mesba_memory::SubGoal> =
            workspace.goal_stack.snapshot().into_iter().cloned().collect();

        Self {
            snapshot_id,
            timestamp: crate::api::now_ms(),
            goal_stack,
            pulse_buffer,
            active_path,
            current_ring: current_ring.to_string(),
            ticks_at_checkpoint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::Block;
    use mesba_memory::GlobalWorkspace;

    #[test]
    fn test_security_gate_default() {
        let g = SecurityGate::default();
        assert_eq!(g.max_ticks_per_session, 200);
        // seed_addition_table 约 120 blocks → 默认 500 留足余量
        assert_eq!(g.max_blocks_activated, 500);
        // plan_single_digit 有 100+ outgoing → 默认 1000 防星型攻击
        assert_eq!(g.max_connections_per_block, 1000);
    }

    #[test]
    fn test_check_tick_limit_violation() {
        let gate = SecurityGate::new(10, 100, 50);
        let network = MeshBlockNetwork::new();
        let workspace = GlobalWorkspace::new();
        let err = gate
            .check_before_propagate(&network, &workspace, 100)
            .unwrap_err();
        assert!(matches!(err, SecurityViolation::ExceedMaxTicks { .. }));
        assert!(err.short_msg().contains("100"));
    }

    #[test]
    fn test_check_ok() {
        let gate = SecurityGate::default();
        let network = MeshBlockNetwork::new();
        let workspace = GlobalWorkspace::new();
        assert!(gate
            .check_before_propagate(&network, &workspace, 5)
            .is_ok());
    }

    #[test]
    fn test_violation_severity() {
        let tick = SecurityViolation::ExceedMaxTicks { current: 200, limit: 200 };
        assert_eq!(tick.severity_strength(), 255);

        let conn = SecurityViolation::ExceedMaxConnections {
            block_id: "x".into(),
            connections: 100,
            limit: 50,
        };
        assert_eq!(conn.severity_strength(), 180);
    }

    #[test]
    fn test_checkpoint_construction() {
        let mut workspace = GlobalWorkspace::new();
        workspace.pulse_buffer.set_var("final_result", 42);
        let cp = Checkpoint::new(1, &workspace, "REASON", 10);
        assert_eq!(cp.snapshot_id, 1);
        assert_eq!(cp.current_ring, "REASON");
        assert_eq!(cp.ticks_at_checkpoint, 10);
        assert!(cp.timestamp > 0);
        let has_final = cp
            .pulse_buffer
            .iter()
            .any(|(k, v)| k == "final_result" && *v == 42);
        assert!(has_final);
    }

    #[test]
    fn test_block_presence_network_sanity() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::new(
            mesba_core::BlockId::from("sanity.test"),
            mesba_core::BlockType::Concept,
            "sanity".into(),
            0,
            mesba_core::BlockContent::Concept {
                definition: "test".into(),
                properties: vec!["p1".into()],
                related_concepts: vec![],
            },
        );
        net.register_block(b);
        assert_eq!(net.block_count(), 1);
    }
}
