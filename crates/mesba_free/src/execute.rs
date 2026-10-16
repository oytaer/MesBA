// ============================================================================
// execute.rs —— 执行模块（环 6：EXECUTE）
// ============================================================================
// 职责：执行 GoalStack 栈顶当前 Goal → 调用外部工具 / 注入执行脉冲
// MVP：遍历 trace 中的 RuleStep 执行实际动作；注入 Result 脉冲
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Executor;

impl Executor {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 找 Operation 类型的区块 → 注入激活脉冲让 Propagator 执行 algorithm
        let op_blocks: Vec<_> = ctrl
            .network
            .find_by_type(mesba_core::BlockType::Operation)
            .into_iter()
            .map(|b| b.id.clone())
            .collect();

        // System 1 级别的额外激活（让 Operation 区块被 Propagator 执行）
        if let Some(op) = op_blocks.first() {
            let workspace = ctrl.take_workspace();
            let mut prop = mesba_engine::PulsePropagator::with_memory(
                &mut ctrl.network,
                workspace,
            );
            prop.inject(mesba_core::Pulse::activate(
                mesba_core::PulseId(3),
                mesba_core::BlockId::from("executor"),
                op.clone(),
                255,
                vec![],
                0,
            ));
            let _ = prop.run_system1();
            let ws = prop.retake_memory();
            ctrl.put_workspace(ws);
        }
    }
}
