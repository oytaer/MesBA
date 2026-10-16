// ============================================================================
// runtime.rs —— InferenceRuntime 推理运行时封装
// ============================================================================
// 设计文档 §13-14：薄编排器层，把 mesba_free::FreeMechanismController 包起来
// 对 mesba_cli / HTTP 服务暴露 JSON API
//
// 核心入口：
//   run_query(input: HashMap<String, i32>) -> ApiResponse<InferenceResult>
//     1) input → PerceptualBuffer（write_perceptual Symbolic, signal=[a, b]）
//     2) controller.run_agent_loop(max_ticks)
//     3) workspace 提取 final_result / variables / trace / ticks_used
//     4) 封装成 ApiResponse 返回
//
//   run_query_raw(input) -> InferenceResult（内部调用，不经过 JSON）
// ============================================================================

use std::collections::HashMap;

use mesba_core::MemoryBackend;
use mesba_engine::seed;
use mesba_free::FreeMechanismController;
use mesba_memory::{GlobalWorkspace, Modality};
use mesba_network::MeshBlockNetwork;

use crate::api::{
    ApiResponse, InferenceResult, ReasoningTrace, ReasoningTracePhase, StepRecord, TraceEntryView,
    now_ms,
};

// ============================================================================
// InferenceRuntime
// ============================================================================
pub struct InferenceRuntime {
    /// 内部 Controller（network + workspace + 十模块 + 状态机）
    pub controller: FreeMechanismController,
    /// 单次推理允许的最大 tick 数（防死循环）
    max_ticks_per_query: u64,
    /// ★ F-06 SecurityGate —— 安全门控配置（桥接 Propagator Pass 1 前检查）
    pub security_gate: crate::safety::SecurityGate,
}

impl InferenceRuntime {
    // ========================================================================
    // 构造
    // ========================================================================

    /// 用已有 network 构造（mesba_cli 传入持久化 network）
    pub fn new(network: MeshBlockNetwork) -> Self {
        Self {
            controller: FreeMechanismController::new(network),
            max_ticks_per_query: 200,
            security_gate: crate::safety::SecurityGate::default(),
        }
    }

    /// ★ F-15 Observability 需要暴露可变 network（Learning consolidate 改网络）
    pub fn network_mut(&mut self) -> &mut MeshBlockNetwork {
        &mut self.controller.network
    }

    /// 用已有 network + workspace 构造（测试 / 状态恢复）
    pub fn with_workspace(network: MeshBlockNetwork, workspace: GlobalWorkspace) -> Self {
        Self {
            controller: FreeMechanismController::with_workspace(network, workspace),
            max_ticks_per_query: 200,
            security_gate: crate::safety::SecurityGate::default(),
        }
    }

    /// 用 seed 构造完整 100 个 Fact 加法表网络（集成测试 / demo 默认入口）
    ///   0+0 ~ 9+9 全覆盖 → 任意单位数加法直接查表
    pub fn default_seed_addition() -> Self {
        let network = seed::build_addition_table();
        Self::new(network)
    }

    /// 自定义最大 tick 数
    pub fn set_max_ticks(&mut self, max: u64) {
        self.max_ticks_per_query = max;
    }

    // ========================================================================
    // 高层 API：JSON 封装
    // ========================================================================

    /// 外部调用主入口：输入 HashMap 变量 → ApiResponse<InferenceResult>
    /// 输入约定：{"a": 3, "b": 5} 或任何变量名 → 自动映射到 PerceptualBuffer
    pub fn run_query(
        &mut self,
        input: HashMap<String, i32>,
    ) -> ApiResponse<InferenceResult> {
        let started = now_ms();
        let result = self.run_query_raw(input);
        let elapsed_ms = now_ms() - started;

        // ★ 桥接 3：kill_flag 判定（看门狗 / SecurityGate 触发）
        // kill_flag → CODE_ERROR + kill_reason 写入 msg
        // 若 kill_reason 以 "[SECURITY]" 开头 → CODE_UNAUTHORIZED (401)
        if self.controller.kill_flag {
            let reason = self
                .controller
                .kill_reason
                .clone()
                .unwrap_or_else(|| "kill_flag=true 但无原因".into());
            let code = if reason.starts_with("[SECURITY]") {
                crate::api::CODE_UNAUTHORIZED
            } else {
                crate::api::CODE_ERROR
            };
            return ApiResponse {
                code,
                msg: format!("inference_killed: {} elapsed={elapsed_ms}ms", reason),
                data: result,
                time: now_ms(),
            };
        }

        // 根据 final_result 判定成功 / 失败
        let msg = match result.final_result {
            Some(v) if v != 0 => format!("inference_ok value={v} elapsed={elapsed_ms}ms"),
            _ => format!("inference_no_result elapsed={elapsed_ms}ms"),
        };
        let code = if result.final_result.is_some() && result.final_result != Some(0) {
            crate::api::CODE_OK
        } else {
            crate::api::CODE_ERROR
        };

        ApiResponse {
            code,
            msg,
            data: result,
            time: now_ms(),
        }
    }

    // ========================================================================
    // 底层 API：返回原始 InferenceResult（供 mesba_cli 内部调用）
    // ========================================================================

    /// 原始推理（无 JSON 封装）
    pub fn run_query_raw(
        &mut self,
        input: HashMap<String, i32>,
    ) -> InferenceResult {
        // 1) 重置 workspace（新任务 → 清 GoalStack + PulseBuffer + Trace）
        self.controller.workspace.reset();

        // 2) 输入 → PerceptualBuffer + PulseBuffer
        // ★ 确定性排序：按变量名排序避免 HashMap 随机迭代导致 a/b 交换
        let mut sorted_input: Vec<(String, i32)> =
            input.iter().map(|(k, v)| (k.clone(), *v)).collect();
        sorted_input.sort_by(|a, b| a.0.cmp(&b.0));
        let signal: Vec<i32> = sorted_input.iter().map(|(_, v)| *v).collect();
        if !signal.is_empty() {
            self.controller
                .workspace
                .write_perceptual(Modality::Symbolic, signal.clone(), 255);
            // 直接 set_var 到 PulseBuffer（双保险：Perceive 模块也会从 PerceptualBuffer 提取）
            for (name, value) in &sorted_input {
                self.controller.set_var(name.clone(), *value);
            }
        }

        // 3) 重置状态机到 PERCEIVE（新 query 从感知环开始）
        self.controller.set_ring(mesba_free::CognitiveRing::Perceive);
        self.controller.reset_consecutive_non_activations();

        // ★ 桥接 1：SecurityGate → Propagator 前检查
        // 在 agent loop 前检查一次，避免无效执行
        // 若违规 → 设 kill_flag + kill_reason（加 [SECURITY] 前缀），让 run_query 识别并返回 401
        if let Err(violation) = self.security_gate.check_before_propagate(
            &self.controller.network,
            &self.controller.workspace,
            self.controller.total_ticks(),
        ) {
            self.controller.kill_reason =
                Some(format!("[SECURITY] {}", violation.short_msg()));
            self.controller.kill_flag = true;
            // 返回空 InferenceResult（run_query 的 kill_flag 路径会接管）
            return InferenceResult {
                final_result: None,
                variables: vec![],
                reasoning_trace: crate::api::ReasoningTrace::default(),
                trace: vec![],
                ticks_used: 0,
                final_ring: format!("{:?}", self.controller.current_ring()),
            };
        }

        // 4) 跑 agent loop（最多 max_ticks_per_query）
        let ticks_used = self.controller.run_agent_loop(self.max_ticks_per_query);

        // 5) 提取最终结果
        let final_result = self.controller.get_var("final_result");

        // variables: PulseBuffer snapshot（HashMap<String,i32> → Vec<(String,i32)>）
        let vars_map = self.controller.workspace.pulse_buffer.snapshot();
        let variables: Vec<(String, i32)> = vars_map.into_iter().collect();

        // trace: MemoryBackend trait 方法
        let trace_entries: &[mesba_core::TraceEntry] = self.controller.workspace.trace();
        let trace = trace_entries
            .iter()
            .map(|e| TraceEntryView {
                block_id: e.block_id.to_string(),
                read_result: format!("{:?}", e.read_result),
            })
            .collect();

        let final_ring = format!("{:?}", self.controller.current_ring());

        // ★ F-18 phase-based 推理轨迹：把所有 trace 包成一个 phase（MVP 简化）
        // 后续升级：让 Propagator 每次 run_step 产生独立 phase
        let steps: Vec<StepRecord> = trace_entries
            .iter()
            .enumerate()
            .map(|(i, e)| StepRecord {
                step_index: i as u32,
                block_id: e.block_id.to_string(),
                read_result: format!("{:?}", e.read_result),
                activation_delta: 0, // MVP 简化：TraceEntry 不带 delta，留 0
            })
            .collect();
        let reasoning_trace = ReasoningTrace {
            phases: vec![ReasoningTracePhase {
                phase_name: format!("Full Inference → {}", final_ring),
                started_at_ms: now_ms(),
                steps,
                verification: None, // Feedback 模块填入（MVP 简化：先 None）
            }],
            total_elapsed_ms: 0, // MVP 简化
        };

        InferenceResult {
            final_result,
            variables,
            reasoning_trace,
            trace,
            ticks_used,
            final_ring,
        }
    }

    // ========================================================================
    // 便捷：直接传两个数字（加法 demo 入口）
    // ========================================================================

    /// 单数字加法便捷接口（demo / 集成测试用）
    pub fn run_add(&mut self, a: i32, b: i32) -> ApiResponse<InferenceResult> {
        let mut input = HashMap::new();
        input.insert("a".to_string(), a);
        input.insert("b".to_string(), b);
        self.run_query(input)
    }

    // ========================================================================
    // 辅助：底层访问
    // ========================================================================

    pub fn workspace(&self) -> &GlobalWorkspace {
        &self.controller.workspace
    }

    pub fn network(&self) -> &MeshBlockNetwork {
        &self.controller.network
    }

    pub fn controller(&self) -> &FreeMechanismController {
        &self.controller
    }
}

// ============================================================================
// 集成测试：单数字加法 a=3, b=5 → final_result=8
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addition_3_5() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let resp = rt.run_add(3, 5);
        assert_eq!(resp.code, crate::api::CODE_OK, "msg={}", resp.msg);
        assert_eq!(resp.data.final_result, Some(8), "expected 3+5=8");
    }

    #[test]
    fn test_addition_9_9() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let resp = rt.run_add(9, 9);
        assert!(
            resp.data.final_result.is_some(),
            "should produce a final_result, got ticks_used={}",
            resp.data.ticks_used
        );
    }

    #[test]
    fn test_query_map_input() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let mut input = HashMap::new();
        input.insert("a".to_string(), 7);
        input.insert("b".to_string(), 2);
        let resp = rt.run_query(input);
        assert!(resp.is_ok());
        assert_eq!(resp.data.final_result, Some(9));
    }

    #[test]
    fn test_empty_input_yields_error() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let input = HashMap::new();
        let resp = rt.run_query(input);
        // 无输入 → Perceive 无事发生 → final_result=None → 应是 100 错误
        assert_eq!(resp.code, crate::api::CODE_ERROR);
    }

    #[test]
    fn test_multiple_queries_reset_workspace() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let r1 = rt.run_add(2, 3);
        let r2 = rt.run_add(4, 6);
        assert_eq!(r1.data.final_result, Some(5));
        assert_eq!(r2.data.final_result, Some(10)); // 互不污染
    }

    #[test]
    fn test_response_json_format() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let resp = rt.run_add(1, 2);
        let json = resp.to_json();
        // 按用户规则格式：{code, msg, data(内含 final_result 等), time}
        assert!(json.contains("\"code\":200"));
        assert!(json.contains("\"final_result\":3"));
        assert!(json.contains("\"time\":"));
    }

    // ---------- 端到端集成测试 ----------

    #[test]
    fn test_e2e_submit_3_5_returns_8() {
        // 端到端: submit({a:3, b:5}) → Propagator 激活 Fact 3+5 → ReadResult 读值 8 → 写 final_result
        let mut rt = InferenceRuntime::default_seed_addition();
        let input: std::collections::HashMap<String, i32> = [
            ("a".to_string(), 3),
            ("b".to_string(), 5),
        ].into();
        let resp = rt.run_query(input);
        assert_eq!(
            resp.code, 200,
            "应返回 200，实际 {} msg={}",
            resp.code, resp.msg
        );
        let result = resp.data;

        // ★ 强断言：3+5 必须等于 8（不能只 is_some）
        assert_eq!(
            result.final_result, Some(8),
            "端到端 final_result 应为 8，实际 {:?}",
            result.final_result
        );

        // Fact block 也必须存在
        let fact_exists = mesba_engine::seed::get_fact_value(rt.network(), 3, 5).is_some();
        assert!(fact_exists, "3+5 Fact 必须存在于网络中");

        // PulseBuffer 应有至少 a, b, final_result 三个变量
        let has_a = result.variables.iter().any(|(k, v)| k == "a" && *v == 3);
        let has_b = result.variables.iter().any(|(k, v)| k == "b" && *v == 5);
        let has_final = result.variables.iter().any(|(k, v)| k == "final_result" && *v == 8);
        assert!(has_a, "PulseBuffer 应有 a=3");
        assert!(has_b, "PulseBuffer 应有 b=5");
        assert!(has_final, "PulseBuffer 应有 final_result=8");
    }

    #[test]
    fn test_e2e_consolidate_after_query() {
        // 端到端: query → consolidate → LearningReport 非空
        let mut rt = InferenceRuntime::default_seed_addition();
        let _resp = rt.run_add(3, 5);

        // 用 Learning trait: consolidate
        use crate::interfaces::Learning;
        let report = <InferenceRuntime as Learning>::consolidate(&mut rt);
        // consolidate 应产生有效 connections_modified / blocks_added
        // （不强制 learn_rate，LearningReport 没 learn_rate 字段）
        assert!(
            report.blocks_added > 0 || report.connections_modified > 0 || report.prunings > 0,
            "consolidate 应产生有效学习行为"
        );
    }

    #[test]
    fn test_e2e_checkpoint_roundtrip() {
        // 端到端: query → checkpoint → snapshot 有 pulse_buffer 数据
        let mut rt = InferenceRuntime::default_seed_addition();
        let _resp = rt.run_add(3, 5);

        use crate::interfaces::Observability;
        let cp = <InferenceRuntime as Observability>::checkpoint(&rt, "REASON");
        assert!(
            !cp.pulse_buffer.is_empty(),
            "checkpoint 应捕获 PulseBuffer 变量"
        );
        // snapshot_id 当前实现硬编码为 0 —— 只要不是 None 就好
        assert!(cp.current_ring == "REASON");
    }
}
