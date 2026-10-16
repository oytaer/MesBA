// ============================================================================
// gws.rs —— L1 GlobalWorkspace（三级记忆的工作核心）
// ============================================================================
// 设计文档 §7 GlobalWorkspace：
//   - 聚合 PerceptualBuffer（L0 感知缓冲区）
//   - 聚合 PulseBuffer（工作记忆变量表）
//   - 聚合 GoalStack（子目标栈）
//   - 聚合 SituationModel（当前情境表示，简化版）
//   - 实现 MemoryBackend trait（mesba_engine::PulsePropagator 泛型参数）
//
// GlobalWorkspace 是 mesba_memory crate 对外暴露的主结构体，
// mesba_runtime::InferenceRuntime 通过 PulsePropagator<GlobalWorkspace> 组合全系统。
// ============================================================================

use std::collections::HashMap;

use mesba_core::MemoryBackend;

use crate::episodic::EpisodicMemory;
use crate::goal_stack::{GoalStack, SubGoal};
use crate::perceptual_buffer::{Modality, PerceptualBuffer};
use crate::pulse_buffer::PulseBuffer;

// ============================================================================
// SituationModel —— §3.1 / §7 情境模型
// ============================================================================
// 当前问题的综合理解表示：
//   - classification: 情境分类标签（如 "math.arithmetic.addition"）
//   - known_vars:      已知变量及其值（从 PulseBuffer 快照）
//   - knowledge_gaps:  知识缺口（理解环节 mark_gap 标记的）
//   - confidence:      当前理解的置信度 0~255
//   - input_summary:   输入的简要文本描述
//
// 被 Understand 环节更新 → 被 Reason 环节读取
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct SituationModel {
    /// 情境分类标签（如 "math.arithmetic.addition"）
    pub classification: String,
    /// 已知变量快照（从 PulseBuffer var_map 复制）
    pub known_vars: HashMap<String, i32>,
    /// 知识缺口标记（理解环节发现的缺失变量/知识）
    pub knowledge_gaps: Vec<String>,
    /// 当前理解置信度 0~255（越高越确定）
    pub confidence: u8,
    /// 输入简要描述（如 "a=3 b=5"）
    pub input_summary: String,
}

impl SituationModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 PerceptualBuffer + 初始 PulseBuffer 更新
    /// Perceive 环节结束后调用：把输入信号转成情境表示
    pub fn update_from_perceptual(
        &mut self,
        var_map: &HashMap<String, i32>,
        summary_prefix: &str,
    ) {
        self.known_vars = var_map.clone();
        // 生成 input_summary：把非零变量拼成 "name=value" 对
        let parts: Vec<String> = var_map
            .iter()
            .filter(|(_, v)| **v != 0) // &v → **v 新 Rust 规则
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        self.input_summary = if parts.is_empty() {
            summary_prefix.to_string()
        } else {
            format!("{} {}", summary_prefix, parts.join(" "))
        };
        // 输入存在 → 初始置信度 128（中性偏上）
        self.confidence = if parts.is_empty() { 0 } else { 128 };
    }

    /// 从 Understand 环节更新：分类 + 置信度 + 清除已填补的缺口
    pub fn update_from_understanding(
        &mut self,
        classification: &str,
        new_confidence: u8,
    ) {
        self.classification = classification.to_string();
        self.confidence = new_confidence;
        // 分类明确 + 置信度足够 → 清除缺口（后续推理可以填补）
        if new_confidence >= 128 {
            self.knowledge_gaps.clear();
        }
    }

    /// 标记一个知识缺口（理解环节发现某变量/知识缺失）
    pub fn mark_gap(&mut self, description: &str) {
        if !self.knowledge_gaps.iter().any(|g| g == description) {
            self.knowledge_gaps.push(description.to_string());
        }
        // 缺口 → 置信度下降
        self.confidence = self.confidence.saturating_sub(20);
    }

    /// 完全重置（新任务开始时）
    pub fn clear(&mut self) {
        self.classification.clear();
        self.known_vars.clear();
        self.knowledge_gaps.clear();
        self.confidence = 0;
        self.input_summary.clear();
    }
}

// ============================================================================
// GlobalWorkspace —— 主体
// ============================================================================
#[derive(Debug, Clone)]
pub struct GlobalWorkspace {
    /// L0 感知缓冲区（瞬时记忆，7±2 槽位）
    pub perceptual: PerceptualBuffer,

    /// L1 GWS PulseBuffer（工作记忆变量表，20 槽位）
    pub pulse_buffer: PulseBuffer,

    /// L1 GWS GoalStack（子目标栈，深度 ≤ 7）
    pub goal_stack: GoalStack,

    /// L1 GWS SituationModel（当前情境表示 — §3.1 / §7 核心）
    pub situation: SituationModel,

    /// L2 EpisodicMemory（长期情景记忆 — §7.1 三大子系统之一）
    pub episodic: EpisodicMemory,

    /// 本 step 的执行轨迹（供 mesba_free Feedback / 可观测性用）
    /// ★ 推理轨迹（phase-based，设计文档 §15.2）
    pub trace: Vec<mesba_core::TraceEntry>,

    /// 当前 step 编号
    step: u64,
}

impl GlobalWorkspace {
    pub fn new() -> Self {
        let mut gw = Self {
            perceptual: PerceptualBuffer::new(),
            pulse_buffer: PulseBuffer::new(),
            goal_stack: GoalStack::new(),
            situation: SituationModel::new(),
            episodic: EpisodicMemory::new(),
            trace: Vec::new(),
            step: 0,
        };
        // 预注册 MesBA MVP 的关键 Fixed slot（永不衰减淘汰）
        // 这些变量是两位加法推理链的核心中间变量
        for fixed_var in &[
            "a", "b",          // 输入操作数
            "carry",           // 进位标记
            "ones_sum",        // 个位和
            "tens_sum",        // 十位和（含进位）
            "final_result",    // 最终结果
            "last_result",     // Fact 查表 / Operation 执行的 fallback 结果
        ] {
            gw.pulse_buffer.register_fixed(fixed_var, 0);
        }
        gw
    }

    /// 推进 step（Propagator 每 step 结束时调用）
    /// 同时推进 PulseBuffer 衰减 + 清空本 step trace
    pub fn advance_step(&mut self, new_step: u64) {
        self.step = new_step;
        self.pulse_buffer.advance_step(new_step);
        self.trace.clear();
    }

    /// 把 PulseBuffer 当前变量表快照传给 GoalStack.push()
    pub fn snapshot_for_goal_push(&self) -> HashMap<String, i32> {
        self.pulse_buffer.snapshot()
    }

    /// 回滚 PulseBuffer 到 checkpoint 快照（GoalStack.rollback_to 恢复时用）
    pub fn restore_pulse_buffer(&mut self, snapshot: HashMap<String, i32>) {
        // 清空所有变量槽然后逐个写入快照（Fixed slot 保留）
        // 简化实现：直接逐个 set_var 覆盖
        for (key, value) in snapshot {
            self.pulse_buffer.set_var(&key, value);
        }
    }

    // --------- PerceptualBuffer 便捷代理 ---------

    pub fn write_perceptual(
        &mut self,
        modality: Modality,
        signal: Vec<i32>,
        strength: u8,
    ) {
        let ts = self.step * 1000; // step 作为时间戳（毫秒粒度）
        self.perceptual.write(modality, signal, strength, ts);
    }

    // --------- GoalStack 便捷代理 ---------

    pub fn push_goal(
        &mut self,
        name: String,
        plan_id: String,
        conditions: Vec<crate::goal_stack::Condition>,
        completion: crate::goal_stack::Completion,
    ) -> Result<(), String> {
        let snapshot = self.snapshot_for_goal_push();
        self.goal_stack.push(
            name,
            plan_id,
            conditions,
            completion,
            snapshot,
            self.step * 1000,
        )
    }

    pub fn pop_goal(&mut self) {
        let checkpoint = self.goal_stack.pop();
        // checkpoint 已由 GoalStack 移除，此处不做 PulseBuffer 恢复（由外部决定）
        let _ = checkpoint;
    }

    pub fn active_goal(&self) -> Option<&SubGoal> {
        self.goal_stack.top()
    }

    pub fn goal_depth(&self) -> usize {
        self.goal_stack.depth()
    }

    /// 栈空时自动创建一个根 Goal（completion = var 存在且正）
    pub fn push_root_goal_if_absent(&mut self, completion_var: &str) -> Result<(), String> {
        if !self.goal_stack.is_empty() {
            return Ok(());
        }
        self.push_goal(
            "root".to_string(),
            "system_auto_plan".to_string(),
            vec![],
            crate::goal_stack::Completion::VarPresent {
                variable: completion_var.to_string(),
            },
        )
    }

    /// 检查栈顶 completion 条件 → 满足则 pop（不恢复 checkpoint，让上层决定）
    pub fn check_and_pop_completed(&mut self) -> bool {
        if self.goal_stack.is_empty() {
            return false;
        }
        let snapshot = self.pulse_buffer.snapshot();
        if self.goal_stack.check_top_completion(&snapshot) {
            self.goal_stack.pop();
            true
        } else {
            false
        }
    }

    pub fn current_step(&self) -> u64 {
        self.step
    }

    /// 清空全部（新任务开始前重置工作记忆）
    pub fn reset(&mut self) {
        self.perceptual.clear();
        self.pulse_buffer.clear();
        self.goal_stack.clear();
        self.situation.clear();
        self.trace.clear();
        self.step = 0;
        // 重新注册 Fixed slot
        let fixed_vars = ["a", "b", "carry", "ones_sum", "tens_sum", "final_result", "last_result"];
        for v in fixed_vars {
            self.pulse_buffer.register_fixed(v, 0);
        }
    }
}

impl Default for GlobalWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// impl MemoryBackend for GlobalWorkspace
// ============================================================================
// PulsePropagator<GlobalWorkspace> 通过这些方法读写工作记忆。
// GlobalWorkspace 是设计文档 §7 L1 GWS 的完整实现。
impl MemoryBackend for GlobalWorkspace {
    // ---------- 变量表（代理到 PulseBuffer）----------

    fn get_var(&self, name: &str) -> Option<i32> {
        // PulseBuffer::get_var 需要 &mut self（更新 last_access_step），
        // MemoryBackend::get_var 签名是 &self。这里做只读查询（不更新 access）。
        // Fixed slot 永不衰减，Dynamic slot 的 last_access_step 影响 LRU。
        // MVP 简化：直接遍历 slot 只读匹配。
        for slot in self.pulse_buffer.slots_iter() {
            if slot.occupied && slot.name == name {
                return Some(slot.value);
            }
        }
        None
    }

    fn set_var(&mut self, name: impl Into<String>, value: i32) {
        self.pulse_buffer.set_var(&name.into(), value);
    }

    fn var_map(&self) -> HashMap<String, i32> {
        self.pulse_buffer.snapshot()
    }

    // ---------- 轨迹 ----------

    fn trace_push(&mut self, entry: mesba_core::TraceEntry) {
        self.trace.push(entry);
    }

    fn clear_trace(&mut self) {
        self.trace.clear();
    }

    fn trace(&self) -> &[mesba_core::TraceEntry] {
        &self.trace
    }

    // ---------- step ----------

    fn step(&self) -> u64 {
        self.step
    }

    fn set_step(&mut self, step: u64) {
        self.step = step;
        self.pulse_buffer.advance_step(step); // 同步推进 PulseBuffer 衰减
        self.trace.clear();
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{MemoryBackend, TraceEntry};

    #[test]
    fn test_global_workspace_fixed_slots_present() {
        let gw = GlobalWorkspace::new();
        assert_eq!(gw.get_var("a"), Some(0));
        assert_eq!(gw.get_var("carry"), Some(0));
        assert_eq!(gw.get_var("ones_sum"), Some(0));
    }

    #[test]
    fn test_var_map_snapshot() {
        let mut gw = GlobalWorkspace::new();
        gw.set_var("a", 23);
        gw.set_var("b", 45);
        let snap = gw.var_map();
        assert_eq!(snap.get("a").copied(), Some(23));
        assert_eq!(snap.get("b").copied(), Some(45));
    }

    #[test]
    fn test_trace_roundtrip() {
        let mut gw = GlobalWorkspace::new();
        let entry = TraceEntry {
            block_id: mesba_core::BlockId::from("test"),
            read_result: mesba_core::ReadResult::None,
        };
        gw.trace_push(entry);
        assert_eq!(gw.trace().len(), 1);
        gw.clear_trace();
        assert!(gw.trace().is_empty());
    }

    #[test]
    fn test_step_advances_pulse_buffer() {
        let mut gw = GlobalWorkspace::new();
        gw.set_var("temp", 100); // dynamic slot
        assert_eq!(gw.get_var("temp"), Some(100));
        // advance 150 step → dynamic slot strength 从 255 减到 255-300=饱和到 0
        // MIN_SIGNAL=5，0 < 5 → 被淘汰
        for step in 1..=150u64 {
            gw.set_step(step);
        }
        assert_eq!(gw.get_var("temp"), None); // 被衰减淘汰
        assert_eq!(gw.get_var("a"), Some(0));  // Fixed slot 保留
    }

    #[test]
    fn test_reset_clears_everything() {
        let mut gw = GlobalWorkspace::new();
        gw.set_var("temp_x", 999);
        gw.write_perceptual(Modality::Symbolic, vec![1, 2, 3], 100);
        gw.situation.update_from_understanding("math.addition", 200);
        gw.situation.mark_gap("missing k");
        assert!(gw.perceptual.filled() > 0);
        assert_eq!(gw.get_var("temp_x"), Some(999));

        gw.reset();
        assert_eq!(gw.perceptual.filled(), 0);
        assert_eq!(gw.get_var("temp_x"), None);
        assert!(gw.situation.classification.is_empty());
        assert_eq!(gw.situation.confidence, 0);
        assert!(gw.situation.knowledge_gaps.is_empty());
        // Fixed slot 重建
        assert_eq!(gw.get_var("a"), Some(0));
    }

    // ---------- SituationModel 单元测试 ----------

    #[test]
    fn test_situation_model_update_from_perceptual() {
        let mut sm = SituationModel::new();
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), 3);
        vars.insert("b".to_string(), 5);
        vars.insert("unused".to_string(), 0); // 值为 0 不进 input_summary

        sm.update_from_perceptual(&vars, "INIT");
        assert_eq!(sm.confidence, 128);
        assert_eq!(sm.known_vars.len(), 3);
        assert!(sm.input_summary.contains("a=3"));
        assert!(sm.input_summary.contains("b=5"));
        assert!(!sm.input_summary.contains("unused=0")); // 零值跳过
    }

    #[test]
    fn test_situation_model_update_from_understanding() {
        let mut sm = SituationModel::new();
        sm.mark_gap("missing carry logic");
        assert_eq!(sm.knowledge_gaps.len(), 1);

        sm.update_from_understanding("math.arithmetic.addition", 220);
        assert_eq!(sm.classification, "math.arithmetic.addition");
        assert_eq!(sm.confidence, 220);
        // confidence >= 128 → 缺口被清除
        assert!(sm.knowledge_gaps.is_empty());
    }

    #[test]
    fn test_situation_model_mark_gap_lowers_confidence() {
        let mut sm = SituationModel::new();
        sm.confidence = 100;
        sm.mark_gap("missing var x");
        assert_eq!(sm.confidence, 80); // saturating_sub(20)
        assert_eq!(sm.knowledge_gaps.len(), 1);

        // 重复标记同一项不重复添加
        sm.mark_gap("missing var x");
        assert_eq!(sm.knowledge_gaps.len(), 1);

        // confidence 为 0 时 saturating_sub 不会 panic
        sm.confidence = 0;
        sm.mark_gap("another gap");
        assert_eq!(sm.confidence, 0);
    }

    #[test]
    fn test_global_workspace_has_situation_model() {
        let gw = GlobalWorkspace::new();
        // situation 字段存在且默认为空
        assert_eq!(gw.situation.classification, "");
        assert_eq!(gw.situation.confidence, 0);
        assert!(gw.situation.knowledge_gaps.is_empty());
    }
}
