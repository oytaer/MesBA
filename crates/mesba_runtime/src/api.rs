// ============================================================================
// api.rs —— MesBA 运行时公共 JSON API 格式（全局统一）
// ============================================================================
// 用户规则强制要求：所有服务端接口统一返回
//   {code:200正确 100错误 401未授权, msg:'xxxx', data:任何格式数据, time:时间戳}
// 本模块定义 ApiResponse<T: Serialize> + 预定义的 code 常量 + 常用构造器
// ============================================================================

use serde::{Deserialize, Serialize};

// ============================================================================
// 业务码常量
// ============================================================================

/// 成功
pub const CODE_OK: u16 = 200;
/// 通用错误
pub const CODE_ERROR: u16 = 100;
/// 未授权（预留）
pub const CODE_UNAUTHORIZED: u16 = 401;

// ============================================================================
// ApiResponse<T> —— 统一 JSON 响应
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T: Serialize> {
    /// 200=正确 / 100=错误 / 401=未授权
    pub code: u16,
    /// 人类可读消息
    pub msg: String,
    /// 业务数据（任意可序列化）
    #[serde(flatten)]
    pub data: T,
    /// 服务器生成时的 epoch 毫秒时间戳
    pub time: u64,
}

impl<T: Serialize> ApiResponse<T> {
    /// 构造成功响应
    pub fn ok(msg: impl Into<String>, data: T) -> Self {
        Self {
            code: CODE_OK,
            msg: msg.into(),
            data,
            time: now_ms(),
        }
    }

    /// 构造错误响应
    pub fn err(msg: impl Into<String>, data: T) -> Self {
        Self {
            code: CODE_ERROR,
            msg: msg.into(),
            data,
            time: now_ms(),
        }
    }

    /// 构造未授权响应
    pub fn unauthorized(msg: impl Into<String>, data: T) -> Self {
        Self {
            code: CODE_UNAUTHORIZED,
            msg: msg.into(),
            data,
            time: now_ms(),
        }
    }

    /// 快速判断 code 是否为 OK
    pub fn is_ok(&self) -> bool {
        self.code == CODE_OK
    }

    /// 序列化成 JSON 字符串
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            // 序列化失败时返回一个兜底 JSON（不 panic）
            format!(
                r#"{{"code":100,"msg":"serialize_error: {}","time":{}}}"#,
                e,
                now_ms()
            )
        })
    }
}

// ============================================================================
// ★ F-18 §15.2 phase-based 推理轨迹结构
// ============================================================================
// 设计文档 §15.2 要求推理轨迹分 phase 组织，每个 phase 包含：
//   Classification → Steps.action → Verification（三层校验）
// 与 v1 扁平 trace 的区别：
//   v1：trace: Vec<TraceEntryView> — 扁平列表
//   v2：reasoning_trace: ReasoningTrace — 按 phase 分组 + verification 三层校验
// ============================================================================

/// §15.2 phase-based 推理轨迹顶层容器
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReasoningTrace {
    /// 每个 phase（Propagator 一次 run_step / CognitiveRing 一环）
    pub phases: Vec<ReasoningTracePhase>,
    /// 总耗时（毫秒）
    pub total_elapsed_ms: u64,
}

/// 单个 phase（§15.2 "Classification → Steps → Verification" 三层）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningTracePhase {
    /// phase 名称（如 "Understand"、"Reason Step 3"、"Feedback"）
    pub phase_name: String,
    /// phase 开始时间戳（epoch ms）
    pub started_at_ms: u64,
    /// phase 内执行的 step 列表（ReadResult 消费记录）
    pub steps: Vec<StepRecord>,
    /// Feedback 三层校验结果（phase 结束后由 Feedback 模块填入）
    pub verification: Option<VerificationRecord>,
}

/// phase 内单个 step 的执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    /// phase 内第几个 step（0-based）
    pub step_index: u32,
    /// 被激活并执行的区块 ID
    pub block_id: String,
    /// BlockContent::read() 返回的 ReadResult 类型（"Value"/"Branch"/"Steps"/"Plan" 等）
    pub read_result: String,
    /// 本 step 对 activation 产生的 delta（正=激活, 负=抑制）
    pub activation_delta: i32,
}

/// Feedback 三层校验结果（§15.2 verification 字段）
/// 由 Feedback 模块在完成一次推理后填充
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationRecord {
    /// 内部一致性（0=完全一致, 255=严重不一致）
    pub internal_consistency: u8,
    /// 外部匹配（期望值 vs 实际值的偏差程度，0=完全匹配）
    pub external_match: u8,
    /// 目标达成度（0=失败, 255=完美达成）
    pub goal_achievement: u8,
    /// Feedback 归因的 ErrorType（如 MISUNDERSTANDING / REASON / BAD_PLAN）
    pub error_type: Option<String>,
}

// ============================================================================
// InferenceResult —— 一次推理的完整结果（data 字段内容）
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResult {
    /// 最终输出变量值（PulseBuffer "final_result"）
    pub final_result: Option<i32>,
    /// 所有 PulseBuffer 变量快照（key=变量名, value=int32）
    pub variables: Vec<(String, i32)>,
    /// ★ F-18：phase-based 推理轨迹（替代旧的扁平 trace）
    pub reasoning_trace: ReasoningTrace,
    /// 工作记忆 trace（TraceEntry JSON 序列化，保留兼容）
    pub trace: Vec<TraceEntryView>,
    /// 消耗 tick 数
    pub ticks_used: u64,
    /// 最终认知环（调试）
    pub final_ring: String,
}

/// TraceEntry 的 JSON 序列化视图（mesba_core::TraceEntry 可能不是 Serialize）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntryView {
    pub block_id: String,
    pub read_result: String,
}

// ============================================================================
// now_ms —— epoch 毫秒时间戳（无 chrono 依赖，供 runtime / 各模块使用）
// ============================================================================
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ok_response_json_roundtrip() {
        let resp = ApiResponse::ok("ok", InferenceResult {
            final_result: Some(8),
            variables: vec![("a".into(), 3), ("b".into(), 5)],
            reasoning_trace: ReasoningTrace::default(),
            trace: vec![],
            ticks_used: 42,
            final_ring: "PERCEIVE".into(),
        });
        let json = resp.to_json();
        assert!(json.contains("\"code\":200"));
        assert!(json.contains("\"msg\":\"ok\""));
        assert!(json.contains("\"final_result\":8"));
        assert!(json.contains("\"ticks_used\":42"));
        assert!(json.contains("\"time\":"));
    }

    #[test]
    fn test_err_response() {
        let resp = ApiResponse::err("bad", ());
        assert_eq!(resp.code, CODE_ERROR);
        assert_eq!(resp.msg, "bad");
    }

    #[test]
    fn test_unauthorized() {
        let resp = ApiResponse::unauthorized("denied", ());
        assert_eq!(resp.code, CODE_UNAUTHORIZED);
    }

    #[test]
    fn test_is_ok() {
        assert!(ApiResponse::ok("x", ()).is_ok());
        assert!(!ApiResponse::err("x", ()).is_ok());
    }
}
