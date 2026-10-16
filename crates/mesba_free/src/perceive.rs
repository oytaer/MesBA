// ============================================================================
// perceive.rs —— 感知模块（环 1：PERCEIVE）
// ============================================================================
// 职责：把外部输入从 PerceptualBuffer 取出 → 注入 PulseBuffer 作为 "a" / "b" 等初始变量
// PerceptualBuffer 的 Signal 向量 = 输入数字；
// set_var("a", digits[0])  set_var("b", digits[1])
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Perceive;

impl Perceive {
    pub fn new() -> Self {
        Self
    }

    /// 从 PerceptualBuffer 把信号注入 PulseBuffer
    /// PerceptualBuffer 的 Symbolic 槽位 = 数字输入（设计文档 MVP §15）
    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        let recent = ctrl.workspace.perceptual.read_recent(2);
        if recent.is_empty() {
            return; // 无感知输入
        }

        // 先 clone signal 再释放 immutable borrow（避免与 set_var 的 &mut borrow 冲突）
        let signal = recent
            .first()
            .map(|s| s.signal.clone())
            .unwrap_or_default();

        if let Some(first) = signal.first() {
            ctrl.set_var("a", *first);
        }
        if signal.len() >= 2 {
            ctrl.set_var("b", signal[1]);
        }
    }
}
