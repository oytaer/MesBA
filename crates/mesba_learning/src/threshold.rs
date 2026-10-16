// ============================================================================
// threshold.rs —— 错误驱动阈值修正算法
// ============================================================================
// 设计文档 §10.2：区块被错误激活（或激活不足）时自动调整 threshold
//   - 正向奖励（激活了 → 结果符合预期）：threshold -= step
//     让该区块下次更容易被激活（加强自信）
//   - 负向惩罚（激活了 → 结果不符预期）：threshold += step
//     让该区块下次更难被激活（避免误触发）
//
// 输入：network + 被激活的 block_ids 集合 + 信号（correct=true/false）
// ============================================================================

use mesba_core::{BlockId, sat8_sub};
use mesba_network::MeshBlockNetwork;

/// 每次调整的步长（uint8 整数步，不是 Q8.8）
pub const THRESHOLD_STEP: u8 = 5;
pub const MIN_THRESHOLD: u8 = 10;
pub const MAX_THRESHOLD: u8 = 240;

/// 一次阈值修正的统计
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ThresholdAdjustStats {
    pub lowered: usize, // 正向
    pub raised: usize,  // 负向
}

// ============================================================================
// adjust_on_feedback —— 主入口
// ============================================================================
/// 对给定区块列表统一调整阈值
///   - correct=true  → 正向奖励：阈值降低 step
///   - correct=false → 负向惩罚：阈值升高 step
pub fn adjust_on_feedback(
    network: &mut MeshBlockNetwork,
    activated_blocks: &[BlockId],
    correct: bool,
    step: u8,
) -> ThresholdAdjustStats {
    let mut stats = ThresholdAdjustStats::default();
    let step = step.max(1);

    for bid in activated_blocks {
        if let Some(block) = network.get_block_mut(bid) {
            if correct {
                // 正向：threshold -= step，但不能低于 MIN_THRESHOLD
                let new_t = if block.state.threshold <= step + MIN_THRESHOLD {
                    MIN_THRESHOLD
                } else {
                    sat8_sub(block.state.threshold, step)
                };
                block.state.threshold = new_t;
                stats.lowered += 1;
            } else {
                // 负向：threshold += step，但不能超过 MAX_THRESHOLD（用 checked_add 防溢出）
                let new_t = match block.state.threshold.checked_add(step) {
                    Some(v) => v.min(MAX_THRESHOLD),
                    None => MAX_THRESHOLD,
                };
                block.state.threshold = new_t;
                stats.raised += 1;
            }
        }
    }
    stats
}

// ============================================================================
// adjust_from_error_signal —— 从反馈信号（error: i32 正为错负为正）调整
// ============================================================================
pub fn adjust_from_error_signal(
    network: &mut MeshBlockNetwork,
    activated_blocks: &[BlockId],
    error_sign: i32,
    step: u8,
) -> ThresholdAdjustStats {
    // error_sign <= 0 → 正确；error_sign > 0 → 错误
    let correct = error_sign <= 0;
    adjust_on_feedback(network, activated_blocks, correct, step)
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{Block, BlockId};

    #[test]
    fn test_correct_lowers_threshold() {
        let mut net = MeshBlockNetwork::new();
        let bid = BlockId::from("fact_a");
        net.register_block(Block::fact(
            bid.clone(),
            "test".into(),
            1,
            vec![],
        ));
        let before = net.get_block(&bid).unwrap().state.threshold;
        let stats = adjust_on_feedback(&mut net, std::slice::from_ref(&bid), true, THRESHOLD_STEP);
        let after = net.get_block(&bid).unwrap().state.threshold;
        assert!(after < before, "correct should lower threshold");
        assert_eq!(stats.lowered, 1);
        assert_eq!(stats.raised, 0);
    }

    #[test]
    fn test_incorrect_raises_threshold() {
        let mut net = MeshBlockNetwork::new();
        let bid = BlockId::from("fact_b");
        net.register_block(Block::fact(
            bid.clone(),
            "test".into(),
            1,
            vec![],
        ));
        let before = net.get_block(&bid).unwrap().state.threshold;
        let stats = adjust_on_feedback(&mut net, std::slice::from_ref(&bid), false, THRESHOLD_STEP);
        let after = net.get_block(&bid).unwrap().state.threshold;
        assert!(after > before, "incorrect should raise threshold");
        assert_eq!(stats.raised, 1);
    }

    #[test]
    fn test_threshold_clamped_to_min() {
        let mut net = MeshBlockNetwork::new();
        let bid = BlockId::from("fact_c");
        net.register_block(Block::fact(
            bid.clone(),
            "test".into(),
            1,
            vec![],
        ));
        // 用大 step 多次降 → 最终停在 MIN_THRESHOLD
        for _ in 0..100 {
            let _ = adjust_on_feedback(&mut net, std::slice::from_ref(&bid), true, 20);
        }
        let final_t = net.get_block(&bid).unwrap().state.threshold;
        assert_eq!(final_t, MIN_THRESHOLD);
    }

    #[test]
    fn test_threshold_clamped_to_max() {
        let mut net = MeshBlockNetwork::new();
        let bid = BlockId::from("fact_d");
        net.register_block(Block::fact(
            bid.clone(),
            "test".into(),
            1,
            vec![],
        ));
        for _ in 0..100 {
            let _ = adjust_on_feedback(&mut net, std::slice::from_ref(&bid), false, 20);
        }
        let final_t = net.get_block(&bid).unwrap().state.threshold;
        assert_eq!(final_t, MAX_THRESHOLD);
    }
}
