// ============================================================================
// memory_backend.rs —— 记忆后端抽象 trait（mesba_memory 与 mesba_engine 的解耦层）
// ============================================================================
// 设计文档 §7 三级记忆系统 —— 定义所有记忆模块必须实现的统一接口。
//
// 为什么 trait 放在 mesba_core（而不是 mesba_memory / mesba_engine）？
//   - mesba_engine → mesba_core（Engine 要能调用 MemoryBackend 方法）
//   - mesba_memory → mesba_core（Memory 要能 impl MemoryBackend trait）
//   - mesba_runtime 组合两者
//   把 trait 放在 mesba_core 避免了循环依赖。
//
// 谁来实现？
//   - mesba_engine::WorkspaceContext    —— 轻量占位实现（Engine 内部测试用）
//   - mesba_memory::GlobalWorkspace     —— 完整实现（PerceptualBuffer + PulseBuffer + GoalStack + SituationModel 聚合）
//
// 谁来消费？
//   - mesba_engine::PulsePropagator<M: MemoryBackend> 泛型化
//   - mesba_free::FreeMechanismController 泛型化
//   - mesba_runtime::InferenceRuntime<W: MemoryBackend>
//
// ============================================================================

use std::collections::HashMap;

use crate::ReadResult;

// ============================================================================
// TraceEntry —— 执行轨迹的一条记录
// ============================================================================
// 记录"哪个区块被 read() 激活后返回了什么"，用于：
//   - mesba_free 认知八环反馈系统（Feedback / Transfer 模块）
//   - mesba_runtime 可观测性（reasoning_trace JSON 输出）
//   - mesba_memory 情景记忆写入
#[derive(Debug, Clone)]
pub struct TraceEntry {
    /// 被 read() 的区块
    pub block_id: crate::BlockId,
    /// read() 返回的结构化结果
    pub read_result: ReadResult,
}

// ============================================================================
// MemoryBackend trait
// ============================================================================
// 设计文档 §7 GlobalWorkspace 接口最小集：
//   L0 PerceptualBuffer —— 感知缓冲区（暂不 trait 化，独立实现）
//   L1 GWS（本 trait） —— PulseBuffer 变量表 + GoalStack + SituationModel + Trace
//   L2 MBN —— 网状区块网络（MeshBlockNetwork，已独立实现）
pub trait MemoryBackend {
    // ---------- 变量表（PulseBuffer 的核心接口）----------

    /// 读取一个命名变量（Operation 执行时查输入 / Judgment 条件求值时查条件）
    fn get_var(&self, name: &str) -> Option<i32>;

    /// 写入一个命名变量（Operation 执行后写结果 / Runtime 注入初始值）
    fn set_var(&mut self, name: impl Into<String>, value: i32);

    /// 快照整个变量表（供 block.content.read() 使用，read 需要完整 HashMap 做条件求值）
    /// 默认返回空 HashMap — WorkspaceContext 直接 clone buffer_vars，PulseBuffer 需遍历 slot
    fn var_map(&self) -> HashMap<String, i32> {
        HashMap::new()
    }

    /// Judgment 分支执行时注入 set_variables（HashMap<String, String> → i32）
    /// 规则：能 parse::<i32>() 的算数值，否则提取最后的连续数字段或 0
    fn apply_set_variables(&mut self, vars: &HashMap<String, String>) {
        for (key, val_str) in vars {
            let parsed = val_str.parse::<i32>().unwrap_or_else(|_| {
                // 提取最后一个连续数字段（"carry=1" → 1, "hello" → 0）
                let bytes = val_str.as_bytes();
                let mut end = bytes.len();
                while end > 0 && (bytes[end - 1] as char).is_ascii_digit() {
                    end -= 1;
                }
                if end < bytes.len() {
                    let mut start = end;
                    while start < bytes.len() && (bytes[start] as char).is_ascii_digit() {
                        start += 1;
                    }
                    val_str[end..start].parse::<i32>().unwrap_or(0)
                } else {
                    0
                }
            });
            self.set_var(key.clone(), parsed);
        }
    }

    /// 把 ReadResult::Value 写入变量表
    /// 如果 Value 声明了 output_var_name，写到那个变量；否则写到 "last_result"
    fn write_value(&mut self, result: &ReadResult) {
        if let ReadResult::Value {
            int_value,
            output_var_name,
            ..
        } = result
        {
            let var_name = output_var_name.as_deref().unwrap_or("last_result");
            self.set_var(var_name.to_string(), *int_value);
        }
    }

    // ---------- 执行轨迹（供上层消费）----------

    /// 添加一条轨迹（Propagator Pass 2a 调 content.read() 后调用）
    fn trace_push(&mut self, entry: TraceEntry);

    /// 清空本 step 的轨迹（每 step 结束时调用）
    fn clear_trace(&mut self);

    /// 读取全部轨迹（mesba_free Feedback 模块 / 可观测性用）
    fn trace(&self) -> &[TraceEntry];

    // ---------- step 计数 ----------

    /// 当前 step 编号（与 Propagator 保持同步）
    fn step(&self) -> u64;
    fn set_step(&mut self, step: u64);
}

// ============================================================================
// 默认实现（方便测试构造空 MemoryBackend）
// ============================================================================
// 直接给每个方法一个空实现，测试可以用它来快速跳过 MemoryBackend 的完整构造。
// 但推荐实现 WorkspaceContext（engine 内部）或 GlobalWorkspace（memory 完整）。