// ============================================================================
// hebbian.rs —— Hebbian 连接塑形算法
// ============================================================================
// 设计文档 §10：神经科学启发的"一起激活的神经元连在一起"
// 对 trace 中每对相邻区块 (from, to)：
//   - 若 from→to 连接存在且 gate=true → weight += learn_rate * hebbian_rate / 255
//   - 若 trace 中没有这对 → weight *= (255 - learn_rate) / 255 衰减
//   - 若 trace 频繁共现但无连接 → 新建 Diagonal 弱连接
// ============================================================================

use std::collections::HashSet;

use mesba_core::{
    BlockId, ConnectionType, TraceEntry, sat8_add, uint8_multiply,
};
use mesba_network::MeshBlockNetwork;

// ============================================================================
// 全局默认学习率（Q8.8，0.0 ~ 1.0）
// ============================================================================
pub const DEFAULT_LEARN_RATE: u8 = 20; // ≈ 0.08（温和塑形）
pub const DEFAULT_WEAK_CONNECTION_WEIGHT: u8 = 40; // ≈ 0.16（弱连接）
pub const DEFAULT_WEAK_DECAY: u8 = 200; // ≈ 0.78（衰减保留约 80% 信号）

// ============================================================================
// 把 BlockId 转成可 hash 的有序对（用 String key）
// ============================================================================
fn pair_key(a: &BlockId, b: &BlockId) -> (String, String) {
    if a.0 <= b.0 {
        (a.0.clone(), b.0.clone())
    } else {
        (b.0.clone(), a.0.clone())
    }
}

// ============================================================================
// hebbian_reinforce —— 主入口：根据 trace 强化 + 衰减权重
// ============================================================================
pub fn hebbian_reinforce(
    network: &mut MeshBlockNetwork,
    trace: &[TraceEntry],
    learn_rate: u8,
) -> HebbianStats {
    if trace.len() < 2 {
        return HebbianStats::default();
    }

    // 1) 收集 trace 中所有相邻 block 对（co-activation，无方向）
    let mut co_activated_pairs: HashSet<(String, String)> = HashSet::new();
    for window in trace.windows(2) {
        let from = &window[0].block_id;
        let to = &window[1].block_id;
        if from != to {
            co_activated_pairs.insert(pair_key(from, to));
        }
    }

    let mut stats = HebbianStats::default();
    let learn_rate = learn_rate.max(1);

    // 2) 遍历所有连接：同激活 → 强化；否则 → 衰减
    let conn_ids: Vec<_> = network.connections().map(|(id, _)| *id).collect();
    for cid in conn_ids {
        let conn = match network.connections().find(|(id, _)| **id == cid) {
            Some((_, c)) => c.clone(),
            None => continue,
        };

        let pair = pair_key(&conn.source_id, &conn.target_id);
        let co_activated = co_activated_pairs.contains(&pair);

        // gate=false 的连接（回环/时序）不参与 Hebbian
        if !conn.gate {
            continue;
        }

        if let Some(conn_mut) = network.get_connection_mut(&cid) {
            if co_activated {
                // 强化：Δw = learn_rate * hebbian_rate / 255
                let delta = uint8_multiply(learn_rate, conn.hebbian_rate.max(1));
                // sat8_add 本身饱和到 255，不需要额外 min
                conn_mut.weight = sat8_add(conn_mut.weight, delta);
                stats.connections_reinforced += 1;
            } else {
                // 衰减：纵向/斜向连接自然衰减
                if conn.conn_type == ConnectionType::Vertical
                    || conn.conn_type == ConnectionType::Diagonal
                {
                    let decayed = uint8_multiply(conn_mut.weight, 255 - learn_rate);
                    conn_mut.weight = decayed;
                    stats.connections_decayed += 1;
                }
            }
        }
    }
    stats
}

// ============================================================================
// hebbian_ensure_connections —— 对 trace 中频繁共现但无连接的区块对新建弱连接
// ============================================================================
pub fn hebbian_ensure_connections(
    network: &mut MeshBlockNetwork,
    trace: &[TraceEntry],
    min_co_activation: usize,
) -> usize {
    if trace.len() < 2 {
        return 0;
    }

    // 统计相邻区块对出现次数
    let mut pair_counts: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for window in trace.windows(2) {
        let from = &window[0].block_id;
        let to = &window[1].block_id;
        if from != to {
            let key = pair_key(from, to);
            *pair_counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut created = 0usize;
    for ((a_str, b_str), count) in &pair_counts {
        if *count < min_co_activation {
            continue;
        }
        let a = BlockId::new(a_str.clone());
        let b = BlockId::new(b_str.clone());

        let exists_a_to_b = network
            .connections()
            .any(|(_, c)| c.source_id == a && c.target_id == b);
        let exists_b_to_a = network
            .connections()
            .any(|(_, c)| c.source_id == b && c.target_id == a);

        if !exists_a_to_b {
            network.connect_with_weight(
                a.clone(),
                b.clone(),
                ConnectionType::Diagonal,
                DEFAULT_WEAK_CONNECTION_WEIGHT,
                DEFAULT_WEAK_DECAY,
            );
            created += 1;
        }
        if !exists_b_to_a {
            network.connect_with_weight(
                b.clone(),
                a.clone(),
                ConnectionType::Diagonal,
                DEFAULT_WEAK_CONNECTION_WEIGHT,
                DEFAULT_WEAK_DECAY,
            );
            created += 1;
        }
    }
    created
}

// ============================================================================
// HebbianStats —— 学习统计
// ============================================================================
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HebbianStats {
    pub connections_reinforced: usize,
    pub connections_decayed: usize,
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::Block;

    fn make_two_connected_blocks() -> MeshBlockNetwork {
        let mut net = MeshBlockNetwork::new();
        net.register_block(Block::concept(
            BlockId::from("A"),
            "test".into(),
            1,
            "A".into(),
        ));
        net.register_block(Block::concept(
            BlockId::from("B"),
            "test".into(),
            2,
            "B".into(),
        ));
        net.connect(
            BlockId::from("A"),
            BlockId::from("B"),
            ConnectionType::Diagonal,
        );
        net
    }

    #[test]
    fn test_hebbian_reinforces_co_activated() {
        let mut net = make_two_connected_blocks();
        let trace = vec![
            TraceEntry {
                block_id: BlockId::from("A"),
                read_result: mesba_core::ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("B"),
                read_result: mesba_core::ReadResult::None,
            },
        ];
        let before = net
            .connections()
            .next()
            .map(|(_, c)| c.weight)
            .unwrap();
        let _ = hebbian_reinforce(&mut net, &trace, DEFAULT_LEARN_RATE);
        let after = net
            .connections()
            .next()
            .map(|(_, c)| c.weight)
            .unwrap();
        assert!(after > before, "co-activated connection should strengthen");
    }

    #[test]
    fn test_hebbian_decays_non_co_activated() {
        let mut net = make_two_connected_blocks();
        // 只有 A 自己 —— 没有 A→B pair → 应该衰减
        let trace = vec![
            TraceEntry {
                block_id: BlockId::from("A"),
                read_result: mesba_core::ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("A"),
                read_result: mesba_core::ReadResult::None,
            },
        ];
        let before = net
            .connections()
            .next()
            .map(|(_, c)| c.weight)
            .unwrap();
        let _ = hebbian_reinforce(&mut net, &trace, DEFAULT_LEARN_RATE);
        let after = net
            .connections()
            .next()
            .map(|(_, c)| c.weight)
            .unwrap();
        assert!(after < before, "non-co-activated connection should decay");
    }

    #[test]
    fn test_ensure_connections_creates_new() {
        let mut net = MeshBlockNetwork::new();
        net.register_block(Block::concept(
            BlockId::from("X"),
            "test".into(),
            1,
            "X".into(),
        ));
        net.register_block(Block::concept(
            BlockId::from("Y"),
            "test".into(),
            2,
            "Y".into(),
        ));
        let trace = vec![
            TraceEntry {
                block_id: BlockId::from("X"),
                read_result: mesba_core::ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("Y"),
                read_result: mesba_core::ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("X"),
                read_result: mesba_core::ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("Y"),
                read_result: mesba_core::ReadResult::None,
            },
        ];
        let created = hebbian_ensure_connections(&mut net, &trace, 2);
        assert!(created > 0, "should create at least one weak diagonal connection");
    }
}
