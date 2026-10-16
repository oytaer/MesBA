// ============================================================================
// understand.rs —— 理解模块（环 2：UNDERSTAND）
// ============================================================================
// 职责：执行 System 1 快速传播（最多 5 step），
// 让感知输入激活网络中相关区块（Concept/Perceptual → Fact/Operation）
// 传播结束后把最新 trace 写入 workspace.trace 供后续环消费。
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Understand;

impl Understand {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 注入激活脉冲：从 PulseBuffer 当前变量推断要激活的 Perceptual 区块
        let has_a = ctrl.get_var("a").is_some();
        let has_b = ctrl.get_var("b").is_some();

        // 找第一个 Perceptual 类型区块并注入激活
        let percept_blocks: Vec<_> = ctrl
            .network
            .find_by_type(mesba_core::BlockType::Perceptual)
            .into_iter()
            .map(|b| b.id.clone())
            .collect();

        if let Some(first_percept) = percept_blocks.first() {
            let workspace = ctrl.take_workspace();
            let mut prop = mesba_engine::PulsePropagator::with_memory(
                &mut ctrl.network,
                workspace,
            );
            prop.inject(mesba_core::Pulse::activate(
                mesba_core::PulseId(1),
                mesba_core::BlockId::from("controller"),
                first_percept.clone(),
                255,
                vec![],
                0,
            ));
            // System 1 最多 5 step
            let history = prop.run_system1();
            let ws = prop.retake_memory();
            ctrl.put_workspace(ws);

            let stats = crate::controller::PropagationStats {
                pulses_processed: history.iter().map(|r| r.pulses_processed).sum(),
                blocks_activated: history.iter().map(|r| r.blocks_activated).sum(),
                pulses_forwarded: history.iter().map(|r| r.pulses_forwarded).sum(),
            };
            ctrl.set_last_propagation(stats);
        }

        let _ = (has_a, has_b); // 保持 a/b 变量存在（Perceive 已注入）
    }
}
