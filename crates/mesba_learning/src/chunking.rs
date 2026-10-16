// ============================================================================
// chunking.rs —— 组块化（Chunking / Composite Block 发现）
// ============================================================================
// 设计文档 §10.4：如果两对区块总是一起被激活（共现率高），
// 就把它们"合并"成一个 Composite 区块
// ============================================================================

use std::collections::HashMap;

use mesba_core::{BlockId, TraceEntry};
use mesba_network::MeshBlockNetwork;

/// 一次组块化迭代的统计
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ChunkStats {
    pub candidate_pairs: usize, // 达到阈值的共现对
    pub chunks_created: usize,  // 真正合并成功的
}

// ============================================================================
// collect_co_activation_pairs —— 从 TraceEntry 收集共现对（String key）
// ============================================================================
pub fn collect_co_activation_pairs(
    trace: &[TraceEntry],
) -> HashMap<(String, String), u64> {
    let mut counts: HashMap<(String, String), u64> = HashMap::new();
    if trace.len() < 2 {
        return counts;
    }
    for window in trace.windows(2) {
        let a = &window[0].block_id.0;
        let b = &window[1].block_id.0;
        if a != b {
            let key = if a <= b {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            };
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
}

// ============================================================================
// find_chunk_candidates —— 找出"总是一起激活"的候选
// ============================================================================
pub fn find_chunk_candidates(
    counts: &HashMap<(String, String), u64>,
    co_activation_threshold: u64,
) -> Vec<(BlockId, BlockId, u64)> {
    counts
        .iter()
        .filter(|(_, c)| **c >= co_activation_threshold)
        .map(|((a_str, b_str), c)| {
            (
                BlockId::new(a_str.clone()),
                BlockId::new(b_str.clone()),
                *c,
            )
        })
        .collect()
}

// ============================================================================
// chunk_network —— 主入口：对网络做一轮组块化分析
// ============================================================================
// MVP 阶段：只做"发现候选"，真正合并 Composite 留给上层
// ============================================================================
pub fn chunk_network(
    network: &MeshBlockNetwork,
    counts: &HashMap<(String, String), u64>,
    co_activation_threshold: u64,
) -> ChunkStats {
    let candidates = find_chunk_candidates(counts, co_activation_threshold);
    let existing_block_ids: std::collections::HashSet<String> = network
        .blocks()
        .map(|(id, _)| id.0.clone())
        .collect();

    let valid_pairs = candidates
        .iter()
        .filter(|(a, b, _)| {
            existing_block_ids.contains(&a.0)
                && existing_block_ids.contains(&b.0)
        })
        .count();

    ChunkStats {
        candidate_pairs: candidates.len(),
        chunks_created: valid_pairs,
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::ReadResult;

    #[test]
    fn test_collect_co_activation_pairs() {
        // 4 个 TraceEntry：相邻对 = (10,20), (20,10), (10,20) → 有序对 key (10,20) 出现 3 次
        let trace = vec![
            TraceEntry {
                block_id: BlockId::from("10"),
                read_result: ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("20"),
                read_result: ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("10"),
                read_result: ReadResult::None,
            },
            TraceEntry {
                block_id: BlockId::from("20"),
                read_result: ReadResult::None,
            },
        ];
        let counts = collect_co_activation_pairs(&trace);
        assert_eq!(
            *counts.get(&("10".to_string(), "20".to_string())).unwrap(),
            3 // 3 个相邻窗口都归到有序对 (10,20)
        );
    }

    #[test]
    fn test_find_chunk_candidates() {
        let mut counts: HashMap<(String, String), u64> = HashMap::new();
        counts.insert(("a".into(), "b".into()), 10); // 高频
        counts.insert(("c".into(), "d".into()), 2);  // 低频
        let candidates = find_chunk_candidates(&counts, 5);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].2 >= 10);
    }
}
