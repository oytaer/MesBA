// ============================================================================
// reason.rs —— 慢推理模块（环 3b：REASON，System 2）
// ============================================================================
// 职责：执行 System 2 深度传播（最多 200 step），解决复杂/未知问题
// 与 Understand 的 System 1 对称，但 step 数上限更高
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Reason;

impl Reason {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 找第一个未触发的 Perceptual 区块做深度传播
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
                mesba_core::PulseId(2),
                mesba_core::BlockId::from("reason"),
                first_percept.clone(),
                255,
                vec![],
                0,
            ));
            let history = prop.run_system2();
            let ws = prop.retake_memory();
            ctrl.put_workspace(ws);

            let stats = crate::controller::PropagationStats {
                pulses_processed: history.iter().map(|r| r.pulses_processed).sum(),
                blocks_activated: history.iter().map(|r| r.blocks_activated).sum(),
                pulses_forwarded: history.iter().map(|r| r.pulses_forwarded).sum(),
            };
            ctrl.set_last_propagation(stats);
        }
    }
}
