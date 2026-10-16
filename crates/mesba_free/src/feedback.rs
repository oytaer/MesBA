// ============================================================================
// feedback.rs —— 反馈模块（环 7：FEEDBACK）— 完整实现
// ============================================================================
// 设计文档 §9 环节 7 + §14.1 明确要求：
//
// 三层校验：
//   ① 内部一致性  — 结果是否自洽？有无矛盾？
//   ② 外部结果匹配 — 执行结果是否符合预期？（goal 里的 expected_result）
//   ③ 目标达成度  — 当前子目标完成了吗？顶层目标完成了吗？
//
// 8-way 错误归因（设计文档 §14.1 完整错误分类表）：
//   感知错误   → 回 PERCEIVE
//   理解错误   → 回 UNDERSTAND
//   知识缺失   → 回 LEARN
//   推理错误   → 回 REASON
//   规划错误   → 回 PLAN
//   执行错误   → 回 EXECUTE 重试
//   过度泛化   → 回 TRANSFER（抑制错误连接）
//   脉冲异常   → 触发 TTL/不应期/全局终止
//
// 成功 → 路由到 TRANSFER（迁移巩固）
// 失败 → 发出 error Pulse + 路由到对应错误环节回退重试
// ============================================================================

use crate::controller::FreeMechanismController;
use crate::state_machine::CognitiveRing;
use mesba_core::MemoryBackend;

/// ============================================================================
/// ErrorType —— 8 种错误归因（严格对齐设计文档 §14.1 完整错误分类表）
/// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ErrorType {
    /// 感知错误：输入模糊/注意力遗漏/歧义 → 回 PERCEIVE 重新感知
    SensoryError,
    /// 理解错误：分类偏差/意图误解/情境模型偏差 → 回 UNDERSTAND
    Misunderstand,
    /// 知识缺失：相关区块不存在/连接太弱 → 回 LEARN
    MissingKnowledge,
    /// 推理错误：走了错误路径/逻辑矛盾 → 回 REASON
    ReasonError,
    /// 规划错误：步骤遗漏/顺序错误/缺 B 计划 → 回 PLAN
    BadPlan,
    /// 执行错误：工具失败/超时/参数错 → 回 EXECUTE 重试
    ExecutionError,
    /// 过度泛化：多次反馈后斜向连接过多/抽象区块不准确 → 回 TRANSFER
    OverGeneralization,
    /// 脉冲异常：传播过程中爆炸/消亡/死循环 → 触发 TTL/不应期/全局终止
    PulseAbnormal,
}

impl ErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorType::SensoryError => "SENSORY_ERROR",
            ErrorType::Misunderstand => "MISUNDERSTAND",
            ErrorType::MissingKnowledge => "MISSING_KNOWLEDGE",
            ErrorType::ReasonError => "REASON_ERROR",
            ErrorType::BadPlan => "BAD_PLAN",
            ErrorType::ExecutionError => "EXECUTION_ERROR",
            ErrorType::OverGeneralization => "OVER_GENERALIZATION",
            ErrorType::PulseAbnormal => "PULSE_ABNORMAL",
        }
    }

    /// 把错误类型映射到状态机下一环（设计文档 §8.3 + §14.1 处理策略列）
    pub fn route_to(&self) -> CognitiveRing {
        match self {
            ErrorType::SensoryError => CognitiveRing::Perceive,
            ErrorType::Misunderstand => CognitiveRing::Understand,
            ErrorType::MissingKnowledge => CognitiveRing::Learn,
            ErrorType::ReasonError => CognitiveRing::Reason,
            ErrorType::BadPlan => CognitiveRing::Plan,
            ErrorType::ExecutionError => CognitiveRing::Execute,
            // 过度泛化 → 抑制错误连接 → 迁移环节处理
            ErrorType::OverGeneralization => CognitiveRing::Transfer,
            // 脉冲异常 → 强制终止 → 下一轮从 Perceive 重开
            ErrorType::PulseAbnormal => CognitiveRing::Perceive,
        }
    }
}

/// ============================================================================
/// VerificationResult —— 三层校验的完整结果
/// ============================================================================
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct VerificationResult {
    /// 内部一致性校验结果
    pub internal_consistency: bool,
    /// 外部结果匹配校验结果（是否等于期望）
    pub external_match: bool,
    /// 子目标达成度
    pub subgoal_achieved: bool,
    /// 顶层目标达成度
    pub topgoal_achieved: bool,
    /// 错误归因（仅当有错误时）
    pub error_type: Option<ErrorType>,
    /// 是否整体成功
    pub overall_success: bool,
    /// 校验详情（人类可读）
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Feedback {
    /// 上次校验结果（可序列化）
    pub last_result: Option<VerificationResult>,
}

impl Feedback {
    pub fn new() -> Self {
        Self { last_result: None }
    }

    // ========================================================================
    // tick —— 主入口（被状态机 Feedback 环调用）
    // ========================================================================
    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 1. 跑三层校验
        let verification = self.verify_all(ctrl);

        // 2. 把 error 值写入 PulseBuffer（让后续 Learning / Transfer 感知）
        let error_val: i32 = if verification.overall_success { 0 } else { 1 };
        ctrl.set_var("error", error_val);
        ctrl.set_var("feedback_value", verification.external_match as i32);

        // 3. 写最终校验详情
        self.last_result = Some(verification.clone());

        // 4. 路由下一环
        let next_ring = if verification.overall_success {
            CognitiveRing::Transfer
        } else {
            match verification.error_type {
                Some(et) => et.route_to(),
                None => CognitiveRing::Transfer, // 无明确归因 → 默认走迁移收尾
            }
        };
        ctrl.set_ring(next_ring);
    }

    // ========================================================================
    // 三层校验（设计文档 §9 环节 7）
    // ========================================================================

    /// 全部三层校验入口
    pub fn verify_all(&self, ctrl: &FreeMechanismController) -> VerificationResult {
        let mut result = VerificationResult::default();

        let final_result = ctrl.get_var("final_result").unwrap_or(0);

        // ── ① 内部一致性 ──
        // 检查：last_result 是否等于 final_result（中间变量不矛盾）
        let last_result = ctrl.get_var("last_result").unwrap_or(0);
        if final_result != 0 && last_result != 0 && final_result != last_result {
            result.internal_consistency = false;
            result.details.push(format!(
                "内部矛盾：final_result={final_result} != last_result={last_result}"
            ));
        } else {
            result.internal_consistency = true;
            result.details.push("内部一致性：通过".into());
        }

        // ── ② 外部结果匹配 ──
        // 检查：PulseBuffer 中的 expected_result vs final_result
        let expected = ctrl.get_var("expected_result").unwrap_or(0);
        if expected != 0 {
            // 有明确预期
            result.external_match = final_result == expected;
            if !result.external_match {
                result.details.push(format!(
                    "外部不匹配：expected={expected} got={final_result}"
                ));
            } else {
                result.details.push(format!(
                    "外部匹配：expected={expected} == final_result={final_result}"
                ));
            }
        } else {
            // 无明确预期 → 只要 final_result != 0 就算"非空输出"通过
            result.external_match = final_result != 0;
            if !result.external_match {
                result.details.push("外部无输出：final_result == 0".into());
            } else {
                result.details.push(format!("外部输出存在：final_result={final_result}"));
            }
        }

        // ── ③ 目标达成度 ──
        // 子目标：只要推理产生了非零结果就算达成
        result.subgoal_achieved = final_result != 0 && result.external_match;
        result.topgoal_achieved = result.subgoal_achieved && result.internal_consistency;
        if result.topgoal_achieved {
            result.details.push("目标达成：通过".into());
        } else {
            result.details.push("目标达成：未通过".into());
        }

        // ── 综合判定 ──
        result.overall_success =
            result.internal_consistency && result.external_match && result.topgoal_achieved;

        // ── 错误归因（如果失败） ──
        if !result.overall_success {
            result.error_type = Some(self.classify_error(ctrl, &result));
        }

        result
    }

    // ========================================================================
    // classify_error —— 8-way 错误归因（设计文档 §14.1 完整错误分类）
    // ========================================================================
    /// 综合多个信号定位错误来源：
    ///   - 感知层检测 → SensoryError（输入异常/注意力为零）
    ///   - kill_flag=true → PulseAbnormal（脉冲被看门狗终止）
    ///   - 内部一致性失败 → ReasonError（推理逻辑矛盾）
    ///   - final_result == 0 → MissingKnowledge 或 ReasonError
    ///   - external_match 失败 + 小差异 → ExecutionError
    ///   - external_match 失败 + 大差异 → Misunderstand
    ///   - OverGeneralization → 保留为 Feedback 的上层模块注入（Transfer 后检测）
    pub fn classify_error(
        &self,
        ctrl: &FreeMechanismController,
        result: &VerificationResult,
    ) -> ErrorType {
        let final_result = ctrl.get_var("final_result").unwrap_or(0);
        let trace_len = ctrl.workspace.trace().len();
        let blocks_activated = ctrl.last_propagation_blocks_activated();

        // 0) 看门狗已终止 → 脉冲异常（设计文档 §14.1 第 8 行）
        if ctrl.kill_flag {
            return ErrorType::PulseAbnormal;
        }

        // 0b) 感知层输入异常 → 感知错误（设计文档 §14.1 第 1 行）
        //     当 a 和 b 都 == 0 或缺失时，属于输入模糊/注意力遗漏
        let a_val = ctrl.get_var("a").unwrap_or(0);
        let b_val = ctrl.get_var("b").unwrap_or(0);
        if a_val == 0 && b_val == 0 && ctrl.get_var("a").is_none() {
            return ErrorType::SensoryError;
        }

        // 1) 内部一致性失败 → 推理逻辑矛盾 → ReasonError
        if !result.internal_consistency {
            return ErrorType::ReasonError;
        }

        // 2) final_result == 0：分两种情况
        if final_result == 0 {
            if trace_len <= 1 || blocks_activated <= 1 {
                // 几乎没有区块被激活 = 知识根本没连上 → MissingKnowledge
                return ErrorType::MissingKnowledge;
            }
            // 有区块激活但没结果 → 推理路径中断或走反 → ReasonError
            return ErrorType::ReasonError;
        }

        // 3) 执行结果 != 预期，但内部一致 → ExecutionError 或 Misunderstand
        if !result.external_match {
            let expected = ctrl.get_var("expected_result").unwrap_or(0);
            let diff = (final_result - expected).abs();
            if diff <= 9 {
                // 小差异（个位/十位进位级）→ 执行器算法细节问题
                return ErrorType::ExecutionError;
            }
            // 大差异 → 可能理解就错了
            return ErrorType::Misunderstand;
        }

        // 4) 子目标未达成但 final_result != 0 → 方案没覆盖到位
        if !result.subgoal_achieved {
            return ErrorType::BadPlan;
        }

        // 5) 默认兜底
        ErrorType::BadPlan
    }

    // ========================================================================
    // route_after_verification —— 根据校验结果决定状态机下一环
    // ========================================================================
    pub fn route_after_verification(&self, _ctrl: &FreeMechanismController) -> CognitiveRing {
        match &self.last_result {
            Some(v) if v.overall_success => CognitiveRing::Transfer,
            Some(v) => v.error_type.as_ref().map(|e| e.route_to()).unwrap_or(CognitiveRing::Transfer),
            None => CognitiveRing::Transfer,
        }
    }
}

// ============================================================================
// 单元测试 —— 覆盖三层校验 + 5-way 归因 + 回退路由
// 注意：测试不能用 mesba_runtime（循环依赖），直接用 Controller + seed
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::FreeMechanismController;
    use mesba_engine::seed::build_addition_table;

    fn make_ctrl() -> FreeMechanismController {
        let network = build_addition_table();
        FreeMechanismController::new(network)
    }

    #[test]
    fn test_feedback_success_three_plus_five() {
        let mut ctrl = make_ctrl();
        // 设置输入变量（模拟 perceiver 产出）
        ctrl.set_var("a", 3);
        ctrl.set_var("b", 5);
        // 手动设置 expected_result
        ctrl.set_var("expected_result", 8);
        // 运行推理 loop
        ctrl.run_agent_loop(50);
        // 切到 Feedback 环节
        ctrl.set_ring(CognitiveRing::Feedback);
        let mut fb = Feedback::new();
        fb.tick(&mut ctrl);
        let r = fb.last_result.clone().unwrap();
        // MVP 种子网络应该能算出 8
        assert_eq!(ctrl.get_var("final_result"), Some(8), "种子网络 3+5 应 = 8");
        assert_eq!(ctrl.get_var("error"), Some(0));
        println!("校验详情: {:?}", r.details);
    }

    #[test]
    fn test_feedback_error_when_no_input() {
        let mut ctrl = make_ctrl();
        // 不设置任何输入 → final_result=None → error=1
        ctrl.run_agent_loop(50);
        ctrl.set_ring(CognitiveRing::Feedback);
        let mut fb = Feedback::new();
        fb.tick(&mut ctrl);
        assert_eq!(ctrl.get_var("error"), Some(1), "无输入时 error 应为 1");
    }

    #[test]
    fn test_error_type_route_to() {
        // 8 种 ErrorType 各自路由回正确环节（严格按设计文档 §14.1 处理策略列）
        assert_eq!(ErrorType::SensoryError.route_to(), CognitiveRing::Perceive);
        assert_eq!(ErrorType::Misunderstand.route_to(), CognitiveRing::Understand);
        assert_eq!(ErrorType::MissingKnowledge.route_to(), CognitiveRing::Learn);
        assert_eq!(ErrorType::ReasonError.route_to(), CognitiveRing::Reason);
        assert_eq!(ErrorType::BadPlan.route_to(), CognitiveRing::Plan);
        assert_eq!(ErrorType::ExecutionError.route_to(), CognitiveRing::Execute);
        assert_eq!(ErrorType::OverGeneralization.route_to(), CognitiveRing::Transfer);
        assert_eq!(ErrorType::PulseAbnormal.route_to(), CognitiveRing::Perceive);
    }

    #[test]
    fn test_classify_error_no_result_missing_knowledge() {
        let mut ctrl = make_ctrl();
        ctrl.run_agent_loop(50); // 无输入
        let fb = Feedback::new();
        let res = VerificationResult {
            internal_consistency: true,
            external_match: false,
            subgoal_achieved: false,
            topgoal_achieved: false,
            error_type: None,
            overall_success: false,
            details: vec![],
        };
        let et = fb.classify_error(&ctrl, &res);
        // 无输入 → final_result=0 + trace 极短 → MissingKnowledge
        assert_eq!(et, ErrorType::MissingKnowledge);
    }

    #[test]
    fn test_error_type_as_str() {
        assert_eq!(ErrorType::SensoryError.as_str(), "SENSORY_ERROR");
        assert_eq!(ErrorType::Misunderstand.as_str(), "MISUNDERSTAND");
        assert_eq!(ErrorType::MissingKnowledge.as_str(), "MISSING_KNOWLEDGE");
        assert_eq!(ErrorType::ReasonError.as_str(), "REASON_ERROR");
        assert_eq!(ErrorType::BadPlan.as_str(), "BAD_PLAN");
        assert_eq!(ErrorType::ExecutionError.as_str(), "EXECUTION_ERROR");
        assert_eq!(ErrorType::OverGeneralization.as_str(), "OVER_GENERALIZATION");
        assert_eq!(ErrorType::PulseAbnormal.as_str(), "PULSE_ABNORMAL");
    }

    #[test]
    fn test_internal_consistency_check() {
        let mut ctrl = make_ctrl();
        ctrl.set_var("a", 3);
        ctrl.set_var("b", 5);
        ctrl.set_var("expected_result", 8);
        ctrl.run_agent_loop(50);
        // 手动制造矛盾
        ctrl.set_var("last_result", 100);
        ctrl.set_ring(CognitiveRing::Feedback);
        let mut fb = Feedback::new();
        fb.tick(&mut ctrl);
        let r = fb.last_result.clone().unwrap();
        assert!(
            !r.internal_consistency,
            "矛盾数据应触发 internal_consistency=false"
        );
        // 矛盾 → ReasonError 归因
        assert_eq!(r.error_type, Some(ErrorType::ReasonError));
    }

    #[test]
    fn test_classify_error_pulse_abnormal_when_killed() {
        let mut ctrl = make_ctrl();
        ctrl.kill_flag = true; // 模拟看门狗终止
        ctrl.set_var("final_result", 5);
        let fb = Feedback::new();
        let res = VerificationResult {
            internal_consistency: true,
            external_match: true,
            subgoal_achieved: true,
            topgoal_achieved: false,
            error_type: None,
            overall_success: false,
            details: vec![],
        };
        let et = fb.classify_error(&ctrl, &res);
        assert_eq!(et, ErrorType::PulseAbnormal);
    }

    #[test]
    fn test_all_8_error_variants_exist() {
        // 穷举 8 种变体（防止未来误删）
        let variants: &[ErrorType] = &[
            ErrorType::SensoryError,
            ErrorType::Misunderstand,
            ErrorType::MissingKnowledge,
            ErrorType::ReasonError,
            ErrorType::BadPlan,
            ErrorType::ExecutionError,
            ErrorType::OverGeneralization,
            ErrorType::PulseAbnormal,
        ];
        assert_eq!(variants.len(), 8, "设计文档 §14.1 要求 8 种错误类型");
        // 所有变体都能序列化为字符串（无 panic）
        for v in variants {
            let _ = v.as_str();
            let _ = v.route_to();
        }
    }
}
