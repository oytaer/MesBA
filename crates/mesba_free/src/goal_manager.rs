// ============================================================================
// goal_manager.rs —— 目标管理器（环 4 前置）
// ============================================================================
// 职责：管理 GoalStack 的整个生命周期
//   - 初始：根据 PerceptualBuffer 输入创建根 Goal
//   - 每 tick：检查栈顶子目标是否满足 completion 条件 → 满足则 pop
//   - GoalStack 深度保护：≥ MAX_DEPTH=7 时最深层目标自动合并
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct GoalManager;

impl GoalManager {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 如果 GoalStack 空，根据 a/b 变量创建根 Goal
        if ctrl.workspace.goal_stack.is_empty() {
            let has_input =
                ctrl.get_var("a").is_some() && ctrl.get_var("b").is_some();
            if has_input {
                // 自动 push 一个根 Goal（Completion = final_result 必须存在且正）
                let _ = ctrl.workspace.push_root_goal_if_absent("final_result");
            }
        }

        // 检查栈顶 completion 条件 → 满足则 pop + 恢复 checkpoint
        ctrl.workspace.check_and_pop_completed();
    }
}
