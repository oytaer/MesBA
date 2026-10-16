// ============================================================================
// decay.rs —— 全局衰减 + 不应期 countdown（§6.3 伪代码 step 8）
// ============================================================================
// 选型文档要求：脉冲衰减计算拆为独立模块（与 propagator.rs 解耦）
//
// 核心职责：
//   每个 Propagator step 末尾 → 所有 block 做 activation 衰减 + refractory countdown
//   设计文档伪代码：
//     target.state.activation = sat8_sub(target.state.activation, DECAY_RATE)
//     if target.state.refractory > 0:
//         target.state.refractory -= 1
// ============================================================================

use mesba_core::{sat8_sub, ACTIVATION_DECAY, REFRACTORY_PERIOD};
use mesba_network::MeshBlockNetwork;

/// 衰减配置（构造时注入默认值，允许上层自定义）
#[derive(Debug, Clone, Copy)]
pub struct DecayConfig {
    /// 每 step activation 衰减量（默认 ACTIVATION_DECAY）
    pub decay_rate: u8,
    /// 不应期持续步数（默认 REFRACTORY_PERIOD）
    pub refractory_period: u8,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            decay_rate: ACTIVATION_DECAY,
            refractory_period: REFRACTORY_PERIOD,
        }
    }
}

impl DecayConfig {
    pub fn new(decay_rate: u8, refractory_period: u8) -> Self {
        Self {
            decay_rate,
            refractory_period,
        }
    }
}

/// 对单个 block 执行一步衰减 + refractory countdown
fn apply_to_block(block: &mut mesba_core::Block, cfg: &DecayConfig) {
    if block.state.activation > 0 {
        block.state.activation = sat8_sub(block.state.activation, cfg.decay_rate);
    }
    if block.state.refractory > 0 {
        block.state.refractory = sat8_sub(block.state.refractory, 1);
    }
}

/// 对整个 network 的所有 block 执行一步衰减
/// Propagator Pass 2c 在转发完成后调用
pub fn apply_decay_all(network: &mut MeshBlockNetwork, cfg: &DecayConfig) {
    for (_id, block) in network.blocks_mut() {
        apply_to_block(block, cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{Block, BlockContent, BlockType};

    fn make_concept_block(id: &str) -> Block {
        Block::new(
            mesba_core::BlockId::from(id),
            BlockType::Concept,
            "test".into(),
            0,
            BlockContent::Concept {
                definition: String::new(),
                properties: Vec::new(),
                related_concepts: Vec::new(),
            },
        )
    }

    #[test]
    fn test_activation_decay() {
        let cfg = DecayConfig::new(10, 2);
        let mut block = make_concept_block("test.concept.decay");
        block.state.activation = 200;
        apply_to_block(&mut block, &cfg);
        assert_eq!(block.state.activation, 190);

        // 连续衰减到 0
        for _ in 0..25 {
            apply_to_block(&mut block, &cfg);
        }
        assert_eq!(block.state.activation, 0);
    }

    #[test]
    fn test_refractory_countdown() {
        let cfg = DecayConfig::new(10, 2);
        let mut block = make_concept_block("test.refractory");
        block.state.refractory = 5;
        apply_to_block(&mut block, &cfg);
        assert_eq!(block.state.refractory, 4);
        apply_to_block(&mut block, &cfg);
        assert_eq!(block.state.refractory, 3);
    }

    #[test]
    fn test_refractory_no_underflow() {
        let cfg = DecayConfig::new(10, 2);
        let mut block = make_concept_block("test.refractory.zero");
        block.state.refractory = 0;
        apply_to_block(&mut block, &cfg);
        assert_eq!(block.state.refractory, 0); // sat8_sub 饱和不减
    }

    #[test]
    fn test_default_config() {
        let cfg = DecayConfig::default();
        assert_eq!(cfg.decay_rate, ACTIVATION_DECAY);
        assert_eq!(cfg.refractory_period, REFRACTORY_PERIOD);
    }
}
