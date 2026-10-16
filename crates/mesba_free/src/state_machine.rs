// ============================================================================
// state_machine.rs —— 认知八环状态机 + System 1/2 切换判定
// ============================================================================
// 设计文档 §8 游离控制系统 + §9 认知八环 + §10 System 1/2 双环机制：
//   八环：PERCEIVE → UNDERSTAND → [REACT|REASON] → PLAN → EXECUTE → FEEDBACK → TRANSFER
//   System 1（React）：快反应，熟悉问题直接激活事实区块读出，最多 5 step
//   System 2（Reason）：慢推理，完整涟漪传播，最多 200 step
//
// 阈值常量严格对齐设计文档 §10.4（全 uint8 0~255 标度）：
//   FAMILIAR_THRESHOLD   = 200  (78% 熟悉)
//   COMPLEXITY_THRESHOLD =  80  (31% 复杂度)
//   RISK_THRESHOLD       =  50  (20% 风险)
//   UNCERTAINTY_THRESHOLD=  60  (24% 不确定)
// ============================================================================

use crate::controller::FreeMechanismController;

// ============================================================================
// CognitiveRing —— 认知八环枚举
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CognitiveRing {
    /// 感知：接收原始输入 → PerceptualBuffer
    Perceive,
    /// 理解：熟悉度/复杂度/风险评估 → System 1/2 分叉
    Understand,
    /// 学习：知识缺失时的即时获取（§8.3 LEARN 状态）
    Learn,
    /// 快反应（System 1）：熟悉情境，最多 5 step 直达结果
    React,
    /// 慢推理（System 2）：复杂推理，最多 200 step
    Reason,
    /// 规划：子目标分解 → GoalStack push
    Plan,
    /// 执行：注入执行脉冲 + 外部工具调用
    Execute,
    /// 反馈：三层校验 + 错误归因 + Checkpoint 回退
    Feedback,
    /// 迁移：Hebbian + 阈值修正 + Pruning + Chunking + Abstraction
    Transfer,
}

impl CognitiveRing {
    /// 转到下一状态（固定八环顺序；Learn 来自理解环节知识缺口检测，之后进 Plan）
    pub fn next(&self) -> Self {
        match self {
            CognitiveRing::Perceive => CognitiveRing::Understand,
            // Understand 分叉到 React/Reason/Learn 由外部逻辑覆盖
            CognitiveRing::Understand => CognitiveRing::Learn, // 默认走 Learn（知识缺口→学习）
            CognitiveRing::Learn => CognitiveRing::Plan,      // 学完 → 规划
            CognitiveRing::React => CognitiveRing::Plan,
            CognitiveRing::Reason => CognitiveRing::Plan,
            CognitiveRing::Plan => CognitiveRing::Execute,
            CognitiveRing::Execute => CognitiveRing::Feedback,
            CognitiveRing::Feedback => CognitiveRing::Transfer,
            CognitiveRing::Transfer => CognitiveRing::Perceive, // 循环
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CognitiveRing::Perceive => "PERCEIVE",
            CognitiveRing::Understand => "UNDERSTAND",
            CognitiveRing::Learn => "LEARN",
            CognitiveRing::React => "REACT",
            CognitiveRing::Reason => "REASON",
            CognitiveRing::Plan => "PLAN",
            CognitiveRing::Execute => "EXECUTE",
            CognitiveRing::Feedback => "FEEDBACK",
            CognitiveRing::Transfer => "TRANSFER",
        }
    }
}

// ============================================================================
// System 1/2 阈值常量（严格对齐设计文档 §10.4）
// 全部 uint8 0~255 标度
// ============================================================================

/// 熟悉度阈值：78% 以上熟悉 → System 1 有资格
/// 设计文档 §10.4：FAMILIAR_THRESHOLD = 200
pub const FAMILIAR_THRESHOLD: u8 = 200;

/// 复杂度阈值：31% 以下复杂度 → System 1 有资格
/// 设计文档 §10.4：COMPLEXITY_THRESHOLD = 80
pub const COMPLEXITY_THRESHOLD: u8 = 80;

/// 风险阈值：20% 以下风险 → System 1 有资格
/// 设计文档 §10.4：RISK_THRESHOLD = 50
pub const RISK_THRESHOLD: u8 = 50;

/// 不确定度阈值：24% 以下不确定 → System 1 有资格
/// 设计文档 §10.4：UNCERTAINTY_THRESHOLD = 60
pub const UNCERTAINTY_THRESHOLD: u8 = 60;

// ============================================================================
// decide_system —— System 1/2 切换判定（设计文档 §10.4 精确实现）
// ============================================================================
// 四个输入全部 uint8 0~255 标度：
//   familiarity    —— 熟悉度 0~255（越高 = 越熟悉）
//   complexity     —— 复杂度 0~255（越高 = 越复杂）
//   risk           —— 风险 0~255（越高 = 风险越大）
//   uncertainty    —— 不确定度 0~255（越高 = 越不确定）
//
// 切换规则（全部 AND）：
//   familiarity >= 200  AND  complexity < 80  AND  risk < 50  AND  uncertainty < 60
//   → System 1（React，最多 5 step）
//   否则 → System 2（Reason，最多 200 step）
pub fn decide_system(
    familiarity: u8,
    complexity: u8,
    risk: u8,
    uncertainty: u8,
) -> CognitiveRing {
    if familiarity >= FAMILIAR_THRESHOLD
        && complexity < COMPLEXITY_THRESHOLD
        && risk < RISK_THRESHOLD
        && uncertainty < UNCERTAINTY_THRESHOLD
    {
        CognitiveRing::React
    } else {
        CognitiveRing::Reason
    }
}

// ============================================================================
// 状态机 tick —— 驱动当前环执行 + 转到下一环
// ============================================================================
pub fn tick(ctrl: &mut FreeMechanismController) {
    use std::mem::take;

    // ★ F-19 防死循环看门狗：推进 tick + 检查连续同环≥3 / 全局步数上限
    if ctrl.advance_tick_watchdog() {
        return;
    }

    let current = ctrl.current_ring();
    match current {
        CognitiveRing::Perceive => {
            let mut m = take(&mut ctrl.perceive);
            m.tick(ctrl);
            ctrl.perceive = m;
        }
        CognitiveRing::Understand => {
            let mut m = take(&mut ctrl.understand);
            m.tick(ctrl);
            ctrl.understand = m;
            // Understand 结束后按 §10.4 标准计算四个 uint8 标度 → 调用 decide_system
            let familiarity = ctrl.compute_familiarity_u8();
            let complexity = ctrl.compute_complexity_u8();
            let risk = ctrl.compute_risk_u8();
            let uncertainty = ctrl.compute_uncertainty_u8();
            // 如果 Risk 非常高（≥ 200）= 知识严重缺失 → LEARN 优先
            let next = if risk >= 200 {
                CognitiveRing::Learn
            } else {
                decide_system(familiarity, complexity, risk, uncertainty)
            };
            ctrl.set_ring(next);
            return; // 手动设置后直接 return（不走默认 next）
        }
        CognitiveRing::Learn => {
            // 知识缺失 → 触发即时学习（transfer.tick 调 hebbian + threshold + chunking）
            let mut m = take(&mut ctrl.transfer);
            m.tick(ctrl);
            ctrl.transfer = m;
        }
        CognitiveRing::React => {
            let mut m = take(&mut ctrl.emotion);
            m.tick(ctrl);
            ctrl.emotion = m;
        }
        CognitiveRing::Reason => {
            let mut m = take(&mut ctrl.reason);
            m.tick(ctrl);
            ctrl.reason = m;
        }
        CognitiveRing::Plan => {
            {
                let mut m = take(&mut ctrl.goal_manager);
                m.tick(ctrl);
                ctrl.goal_manager = m;
            }
            {
                let mut m = take(&mut ctrl.plan);
                m.tick(ctrl);
                ctrl.plan = m;
            }
        }
        CognitiveRing::Execute => {
            {
                let mut m = take(&mut ctrl.attention);
                m.tick(ctrl);
                ctrl.attention = m;
            }
            {
                let mut m = take(&mut ctrl.execute);
                m.tick(ctrl);
                ctrl.execute = m;
            }
        }
        CognitiveRing::Feedback => {
            let mut m = take(&mut ctrl.feedback);
            m.tick(ctrl);
            ctrl.feedback = m;
        }
        CognitiveRing::Transfer => {
            let mut m = take(&mut ctrl.transfer);
            m.tick(ctrl);
            ctrl.transfer = m;
        }
    }
    ctrl.set_ring(current.next());
}

// ============================================================================
// 单元测试 —— 全部按 §10.4 uint8 标度重写
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_next_order() {
        assert_eq!(CognitiveRing::Perceive.next(), CognitiveRing::Understand);
        assert_eq!(CognitiveRing::React.next(), CognitiveRing::Plan);
        assert_eq!(CognitiveRing::Reason.next(), CognitiveRing::Plan);
        assert_eq!(CognitiveRing::Plan.next(), CognitiveRing::Execute);
        assert_eq!(CognitiveRing::Execute.next(), CognitiveRing::Feedback);
        assert_eq!(CognitiveRing::Feedback.next(), CognitiveRing::Transfer);
        assert_eq!(CognitiveRing::Transfer.next(), CognitiveRing::Perceive);
    }

    #[test]
    fn test_decide_system_all_familiar_low_complexity() {
        // 设计文档 §10.4 标准门槛全满足 → System 1 React
        let next = decide_system(220, 40, 20, 30);
        assert_eq!(next, CognitiveRing::React);
    }

    #[test]
    fn test_decide_system_familiarity_too_low() {
        // 熟悉度不够（< 200）→ System 2
        let next = decide_system(150, 40, 20, 30);
        assert_eq!(next, CognitiveRing::Reason);
    }

    #[test]
    fn test_decide_system_complexity_too_high() {
        // 复杂度超标（≥ 80）→ System 2
        let next = decide_system(220, 100, 20, 30);
        assert_eq!(next, CognitiveRing::Reason);
    }

    #[test]
    fn test_decide_system_risk_too_high() {
        // 风险超标（≥ 50）→ System 2
        let next = decide_system(220, 40, 60, 30);
        assert_eq!(next, CognitiveRing::Reason);
    }

    #[test]
    fn test_decide_system_uncertainty_too_high() {
        // 不确定度超标（≥ 60）→ System 2
        let next = decide_system(220, 40, 20, 80);
        assert_eq!(next, CognitiveRing::Reason);
    }

    #[test]
    fn test_decide_system_boundary_values() {
        // 精确边界：刚好等于阈值
        assert_eq!(
            decide_system(FAMILIAR_THRESHOLD, COMPLEXITY_THRESHOLD, RISK_THRESHOLD, UNCERTAINTY_THRESHOLD),
            CognitiveRing::Reason, // complexity==80 不满足 <80
        );
        // 四阈值同时刚好满足边界条件
        assert_eq!(
            decide_system(200, 79, 49, 59),
            CognitiveRing::React,
        );
    }
}
