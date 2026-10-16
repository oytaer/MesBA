// ============================================================================
// emotion.rs —— 情感模块（System 1 快速通道）
// ============================================================================
// 职责：根据 EmotionalValence 决定是否继续 React 快速通道
// 当情感为负（如 error=1 / carry 异常）时转 System 2
// MVP：只看 workspace 里的 error / panic 变量
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Emotion {
    /// 当前情感价：-1 负 / 0 中性 / +1 正
    pub valence: i8,
}

impl Emotion {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 简单启发式：看有没有 error 变量被设
        let has_error = ctrl.get_var("error").unwrap_or(0) != 0;
        let has_panic = ctrl.get_var("panic").unwrap_or(0) != 0;
        if has_error || has_panic {
            self.valence = -1;
        } else {
            self.valence = 1;
        }
    }

    /// 是否应该打断 React 快速通道转到 System 2
    pub fn should_interrupt(&self) -> bool {
        self.valence < 0
    }
}
