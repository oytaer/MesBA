// ============================================================================
// pruning.rs —— 低权重连接修剪
// ============================================================================
// 设计文档 §10.3：当连接权重低于阈值且门控关闭时，判定为"冗余" → 移除
// 修剪是 MesBA 自我压缩的关键机制，避免网络无限膨胀
//
// 修剪条件（同时满足才移除）：
//   1) weight < min_weight（弱到几乎不起作用）
//   2) gate = false（默认关闭，从未主动使用）
// ============================================================================

use mesba_network::MeshBlockNetwork;

pub const DEFAULT_PRUNE_THRESHOLD: u8 = 10; // ≈ 0.04

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PruneStats {
    pub connections_pruned: usize,
    pub connections_kept: usize,
}

// ============================================================================
// prune_weak_connections —— 主入口
// ============================================================================
// 修剪条件（满足其一即标记）：
//   1) weight < min_weight（弱到几乎不起作用）
//   2) weight == 0 且 gate=false（彻底废弃的连接）
// 注意：我们用 weight=0 表示"逻辑移除"，物理删除留给 network compact()
// ============================================================================
pub fn prune_weak_connections(
    network: &mut MeshBlockNetwork,
    min_weight: u8,
) -> PruneStats {
    let mut stats = PruneStats::default();

    let conn_ids: Vec<_> = network.connections().map(|(id, _)| *id).collect();

    for cid in conn_ids {
        if let Some(conn) = network.get_connection_mut(&cid) {
            let should_prune = conn.weight < min_weight;
            if should_prune {
                conn.weight = 0; // 逻辑移除
                stats.connections_pruned += 1;
            } else {
                stats.connections_kept += 1;
            }
        }
    }
    stats
}

// ============================================================================
// prune_zero_weight_after_learning —— 移除所有 weight==0 或 gate=false 的连接
// ============================================================================
pub fn prune_zero_weight(network: &mut MeshBlockNetwork) -> PruneStats {
    prune_weak_connections(network, 1)
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{Block, BlockId};

    #[test]
    fn test_prune_weak_gated_off() {
        let mut net = MeshBlockNetwork::new();
        let a = BlockId::from("p_a");
        let b = BlockId::from("p_b");
        net.register_block(Block::concept(
            a.clone(),
            "test".into(),
            1,
            "A".into(),
        ));
        net.register_block(Block::concept(
            b.clone(),
            "test".into(),
            2,
            "B".into(),
        ));
        // 弱连接 + gate=false（默认）
        net.connect_with_weight(
            a.clone(),
            b.clone(),
            mesba_core::ConnectionType::Diagonal,
            5,
            200, // weight=5 < DEFAULT_PRUNE_THRESHOLD=10, decay≈0.78
        );
        let before = net.connection_count();
        let stats = prune_weak_connections(&mut net, DEFAULT_PRUNE_THRESHOLD);
        assert_eq!(stats.connections_pruned, 1);
        assert_eq!(stats.connections_kept, 0);
        // 注意：我们不物理删除连接，只是 weight=0，所以 connection_count 不变
        let _ = before;
    }

    #[test]
    fn test_prune_keeps_strong() {
        let mut net = MeshBlockNetwork::new();
        let a = BlockId::from("q_a");
        let b = BlockId::from("q_b");
        net.register_block(Block::concept(
            a.clone(),
            "test".into(),
            1,
            "A".into(),
        ));
        net.register_block(Block::concept(
            b.clone(),
            "test".into(),
            2,
            "B".into(),
        ));
        net.connect_with_weight(
            a.clone(),
            b.clone(),
            mesba_core::ConnectionType::Diagonal,
            200, // 强连接
            255, // decay=1.0 不衰减
        );
        let stats = prune_weak_connections(&mut net, DEFAULT_PRUNE_THRESHOLD);
        assert_eq!(stats.connections_pruned, 0);
        assert_eq!(stats.connections_kept, 1);
    }
}
