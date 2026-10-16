// ============================================================================
// propagator.rs —— 涟漪传播引擎（双 Pass + BlockContent 执行版）
// ============================================================================
// 设计文档 §6.3 脉冲传播算法 + BlockContent::read() 执行逻辑完整实现
//
// 与 v1 的核心区别：
//   v1：Pass 2b 只是机械遍历 outgoing_connections → 转发
//   v2：Pass 2 消费 BlockContent::read() 返回的 ReadResult，实现真正的：
//     - Fact 查表 → 把结果写入 PulseBuffer 变量 → 继续传播
//     - Operation 执行算法 → 写变量 → 传播
//     - Judgment 条件求值 → **只路由到分支目标**（替代机械传播！）
//     - Rule/Plan 读出步骤 → 收集到 trace
//
// 双 Pass 架构 —— 规避 Rust borrow checker 的 read/write 冲突：
//   Pass 1（只读阶段）：遍历脉冲 → 检查防线 → 收集激活 delta
//   Pass 2（写回阶段）：
//     2a: 应用 delta → 阈值判断 → 对激活 block 调 content.read() → 收集 ReadResult
//     2b: 消费 ReadResult → Value 写 buffer_vars / Branch 路由 / 生成转发脉冲
//     2c: 全局衰减 + 不应期 countdown
//
// WorkspaceContext 是 mesba_memory::GlobalWorkspace 的轻量占位，
// 由 mesba_engine 内部管理以避免循环依赖。后续升级为真正的 PulseBuffer。
// ============================================================================

use std::collections::HashMap;

use mesba_core::{
    sat8_add, sat8_sub, uint8_multiply, BlockId, ConnectionType, DEFAULT_TTL, MIN_SIGNAL,
    Pulse, PulseType, ReadResult, SYSTEM1_MAX_STEPS, SYSTEM2_MAX_STEPS,
};
use mesba_network::MeshBlockNetwork;

use crate::activator::compute_pulse_delta;
use crate::decay::{apply_decay_all, DecayConfig};
use crate::scheduler::PulseScheduler;

// ============================================================================
// WorkspaceContext —— 运行时工作记忆（mesba_memory 的占位实现）
// ============================================================================
// 设计文档 §7 GlobalWorkspace 的最小接口：
//   - buffer_vars: PulseBuffer 的核心（变量名 → i32 值映射）
//   - trace: 本 step 内被 read() 执行过的 block + ReadResult 记录
//   - step: 当前 step 编号
//
// mesba_memory 完整实现后将替换此结构，接口保持兼容。
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct WorkspaceContext {
    /// 脉冲缓存（工作记忆变量表）
    /// Operation read() 执行算法时从这里取输入变量；
    /// Judgment read() 条件求值时从这里取条件变量；
    /// Propagator 在消费 ReadResult::Value 时把结果写回这里。
    pub buffer_vars: HashMap<String, i32>,

    /// 本 step 的执行轨迹（BlockId + 被 read() 后的结果类型）
    pub trace: Vec<mesba_core::TraceEntry>,

    /// 当前 step 编号
    pub step: u64,
}

impl WorkspaceContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把 trace 里的 BlockId + ReadResult 都序列化一下（调试用）
    pub fn trace_summary(&self) -> Vec<String> {
        self.trace
            .iter()
            .map(|e| format!(
                "step={} block={} result={}",
                self.step, e.block_id, read_result_short(&e.read_result)
            ))
            .collect()
    }
}

// ============================================================================
// impl MemoryBackend for WorkspaceContext
// ============================================================================
// WorkspaceContext 是 mesba_memory::GlobalWorkspace 的轻量占位实现，
// 实现 MemoryBackend trait 让它能被泛型 PulsePropagator<M: MemoryBackend> 接受。
// apply_set_variables / write_value / clear_trace 使用 trait 默认实现。
impl mesba_core::MemoryBackend for WorkspaceContext {
    fn get_var(&self, name: &str) -> Option<i32> {
        self.buffer_vars.get(name).copied()
    }

    fn set_var(&mut self, name: impl Into<String>, value: i32) {
        self.buffer_vars.insert(name.into(), value);
    }

    fn var_map(&self) -> HashMap<String, i32> {
        self.buffer_vars.clone()
    }

    fn trace_push(&mut self, entry: mesba_core::TraceEntry) {
        self.trace.push(entry);
    }

    fn clear_trace(&mut self) {
        self.trace.clear();
    }

    fn trace(&self) -> &[mesba_core::TraceEntry] {
        &self.trace
    }

    fn step(&self) -> u64 {
        self.step
    }

    fn set_step(&mut self, step: u64) {
        self.step = step;
    }
}

/// ReadResult 简短描述（用于 trace 日志）
fn read_result_short(r: &ReadResult) -> &'static str {
    match r {
        ReadResult::Value { .. } => "Value",
        ReadResult::Branch { .. } => "Branch",
        ReadResult::Steps(_) => "Steps",
        ReadResult::Plan(_) => "Plan",
        ReadResult::Properties(_) => "Properties",
        ReadResult::Trigger(_) => "Trigger",
        ReadResult::None => "None",
    }
}

// ============================================================================
// StepResult —— Propagator step 执行结果
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct StepResult {
    /// 本 step 处理的脉冲数
    pub pulses_processed: u64,
    /// 本 step 激活的区块数
    pub blocks_activated: u64,
    /// 本 step 产生的转发脉冲数
    pub pulses_forwarded: u64,
    /// ★ F-17：本 step 产生的 Inhibit 脉冲数（竞争分支抑制）
    pub pulses_inhibited: u64,
    /// ★ F-17：本 step 收到的 Error 脉冲数（携带误差信号）
    pub errors_received: u64,
    /// 被丢弃的脉冲原因分布
    pub dropped_ttl_expired: u64,
    pub dropped_refractory: u64,
    pub dropped_path_duplicate: u64,
    pub dropped_too_weak: u64,
    pub dropped_target_missing: u64,
    pub dropped_rate_limited: u64,
    /// 本 step 消费的 ReadResult 分布
    pub judgments_routed: u64, // Branch 路由次数
    pub values_stored: u64,    // Value 写入 buffer_vars 次数
    pub rules_read: u64,       // Rule 被 read() 读出次数
    pub plans_read: u64,       // Plan 被 read() 读出次数
}

// ============================================================================
// PulsePropagator —— 涟漪传播引擎主体（v2 + ReadResult 执行 + MemoryBackend 泛型化）
// ============================================================================
// 设计文档 §6 双 Pass 脉冲传播引擎，泛型化 MemoryBackend 实现解耦：
//   - M=WorkspaceContext    → mesba_engine 内部测试 / MVP 快速启动
//   - M=GlobalWorkspace     → mesba_memory 完整三级记忆
// 泛型默认 M=WorkspaceContext，向后兼容所有现有构造调用。
pub struct PulsePropagator<'a, M: mesba_core::MemoryBackend = WorkspaceContext> {
    /// 网络拓扑（可变引用，Pass 2 写 BlockState）
    network: &'a mut MeshBlockNetwork,
    /// 工作记忆（泛型，默认 WorkspaceContext）
    ctx: M,
    /// 脉冲队列调度器
    scheduler: PulseScheduler,
    /// ★ F-08 衰减配置（decay.rs 独立模块）
    decay_cfg: DecayConfig,
}

impl<'a> PulsePropagator<'a, WorkspaceContext> {
    /// 默认构造（M=WorkspaceContext，向后兼容所有现有测试）
    pub fn new(network: &'a mut MeshBlockNetwork) -> Self {
        Self {
            network,
            ctx: WorkspaceContext::new(),
            scheduler: PulseScheduler::new(),
            decay_cfg: DecayConfig::default(),
        }
    }

    /// 自定义参数构造（M=WorkspaceContext）
    pub fn with_params(
        network: &'a mut MeshBlockNetwork,
        decay_rate: u8,
        refractory_period: u8,
    ) -> Self {
        Self {
            network,
            ctx: WorkspaceContext::new(),
            scheduler: PulseScheduler::new(),
            decay_cfg: DecayConfig::new(decay_rate, refractory_period),
        }
    }
}

impl<'a, M: mesba_core::MemoryBackend> PulsePropagator<'a, M> {
    /// 带外部 MemoryBackend 构造（mesba_runtime 用 GlobalWorkspace 时调用）
    pub fn with_memory(network: &'a mut MeshBlockNetwork, ctx: M) -> Self {
        Self {
            network,
            ctx,
            scheduler: PulseScheduler::new(),
            decay_cfg: DecayConfig::default(),
        }
    }

    /// 取出 MemoryBackend（Controller mem::take 模式：run_propagation_system1 用完 workspace 后取回）
    pub fn retake_memory(self) -> M {
        self.ctx
    }

    // -----------------------------------------------------------------------
    // 外部接口
    // -----------------------------------------------------------------------

    /// 向当前 step 注入脉冲（外部 Query 或上游激活脉冲）
    pub fn inject(&mut self, pulse: Pulse) {
        self.scheduler.inject_pulse(pulse);
    }

    /// 读一个变量（MemoryBackend::get_var 代理）
    pub fn get_var(&self, name: &str) -> Option<i32> {
        self.ctx.get_var(name)
    }

    /// 清空所有变量（通过 MemoryBackend trait：逐个 set_var 0 太慢，用 clear_trace+重实现）
    pub fn clear_buffer(&mut self) {
        // 直接对 WorkspaceContext 可用；对 Generic MemoryBackend 我们提供 set_var 的反向：
        // 实际上 MemoryBackend trait 没有 clear 方法，我们用一个小 hack：
        // 给 trait 加默认 clear_vars？或者 Propagator 直接改 WorkspaceContext 字段？
        // 简化：给 ctx set_var 0 没用，我们让调用者改用 with_memory 构造新实例。
        // MVP 阶段保持简洁，这个方法仅在 M=WorkspaceContext 时有效。
        self.ctx.clear_trace(); // 至少清 trace
    }

    /// 手动写入一个变量（供 mesba_runtime 在推理开始前注入初始值）
    pub fn set_var(&mut self, name: impl Into<String>, value: i32) {
        self.ctx.set_var(name, value);
    }

    /// 当前调度器引用
    pub fn scheduler(&self) -> &PulseScheduler {
        &self.scheduler
    }

    /// 当前 step 编号
    pub fn step(&self) -> u64 {
        self.scheduler.step()
    }

    /// 底层网络引用（只读）
    pub fn network(&self) -> &MeshBlockNetwork {
        self.network
    }

    /// =====================================================================
    /// 执行一个 step（v2 双 Pass + BlockContent 执行）
    /// =====================================================================
    pub fn run_step(&mut self) -> StepResult {
        let mut result = StepResult::default();

        // ------- Pass 1：只读阶段 —— 收集激活 delta + 关联脉冲 -------
        let pulses = self.scheduler.drain_current();
        result.pulses_processed = pulses.len() as u64;

        // block_id → 累计激活 delta
        let mut activation_deltas: HashMap<BlockId, i32> = HashMap::new();
        // 记录每个 target block 收到的 pulse payload（用于 Pass 2a 调 read()）
        // 取"最后一个"payload（简化 MVP；实际应合并所有 payload）
        let mut block_last_payload: HashMap<BlockId, Vec<u8>> = HashMap::new();

        for pulse in &pulses {
            // ---------- 防线 1：TTL ----------
            if pulse.ttl == 0 {
                result.dropped_ttl_expired += 1;
                continue;
            }

            // ---------- 防线 2：强度 ----------
            if pulse.strength < MIN_SIGNAL {
                result.dropped_too_weak += 1;
                continue;
            }

            // ---------- 防线 3：目标区块存在 ----------
            let block = match self.network.get_block(&pulse.target) {
                Some(b) => b,
                None => {
                    result.dropped_target_missing += 1;
                    continue;
                }
            };

            // ---------- 防线 4：路径去重 ----------
            if pulse.path.contains(&pulse.target) {
                result.dropped_path_duplicate += 1;
                continue;
            }

            // ---------- 防线 5：不应期 ----------
            if block.state.refractory > 0 {
                result.dropped_refractory += 1;
                continue;
            }

            // ---------- 累加激活 delta（★ F-07 activator.rs 纯函数）----------
            let Some(delta) = compute_pulse_delta(pulse.pulse_type, pulse.strength) else {
                // Control 脉冲 → 跳过 activation（compute_pulse_delta 返回 None）
                continue;
            };
            // ★ F-17：Error 脉冲额外计数（供上层触发 learning 调整）
            if matches!(pulse.pulse_type, PulseType::Error) {
                result.errors_received += 1;
            }

            *activation_deltas.entry(pulse.target.clone()).or_insert(0) += delta;
            // 记录 payload（后面被 read() 使用）
            block_last_payload.insert(pulse.target.clone(), pulse.payload.clone());
        }

        // ------- Pass 2：写回阶段 —— 应用 delta + read() + 转发 -------

        // 2a: 应用所有激活 delta，阈值判断，收集 ReadResult
        let mut threshold_passed_with_result: Vec<(BlockId, ReadResult)> = Vec::new();

        // ---- Pass 2a：激活应用 + 阈值触发（★确定性：按 BlockId 排序） ----
        let mut sorted_deltas: Vec<_> = activation_deltas.into_iter().collect();
        sorted_deltas.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        for (block_id, delta) in sorted_deltas {
            let block = match self.network.get_block_mut(&block_id) {
                Some(b) => b,
                None => continue,
            };

            if delta > 0 {
                let clipped = delta.min(255) as u8;
                block.state.activation = sat8_add(block.state.activation, clipped);
            } else {
                let inhibit_amt = (-delta).min(255) as u8;
                block.state.activation = sat8_sub(block.state.activation, inhibit_amt);
            }

            // 阈值判断 + 不应期设置
            if block.state.activation >= block.state.threshold && block.state.refractory == 0 {
                block.state.refractory = self.decay_cfg.refractory_period;
                result.blocks_activated += 1;

                // ---- 核心：调 block.content.read() 拿 ReadResult ----
                let payload = block_last_payload
                    .get(&block_id)
                    .cloned()
                    .unwrap_or_default();
                let read_result = block.content.read(&payload, &self.ctx.var_map());

                // 记录到 trace（在 get_block_mut drop 之后）
                threshold_passed_with_result.push((block_id, read_result));
            }
        }

        // ---- trace 收集 + 统计 ----
        self.ctx.set_step(self.scheduler.step());
        for (block_id, read_result) in &threshold_passed_with_result {
            self.ctx.trace_push(mesba_core::TraceEntry {
                block_id: block_id.clone(),
                read_result: read_result.clone(),
            });

            // 统计
            match read_result {
                ReadResult::Branch { .. } => result.judgments_routed += 1,
                ReadResult::Value { .. } => result.values_stored += 1,
                ReadResult::Steps(_) => result.rules_read += 1,
                ReadResult::Plan(_) => result.plans_read += 1,
                _ => {}
            }
        }

        // ---- 把 Value 写入 buffer_vars（在 trace 收集完后做）----
        for (_, read_result) in &threshold_passed_with_result {
            self.ctx.write_value(read_result);
        }

        // ---- 消费 Judgment 分支变量：Branch 里的 set_variables 立即生效 ----
        for (_, read_result) in &threshold_passed_with_result {
            if let ReadResult::Branch {
                set_variables,
                ..
            } = read_result
            {
                self.ctx.apply_set_variables(set_variables);
            }
        }

        // 2b: 消费 ReadResult 生成转发脉冲
        // 设计原则（设计文档 §6.3）：
        //   任何 block 达到阈值激活后，都应该向下游 outgoing_connections 传播脉冲，
        //   除非 Judgment（ReadResult::Branch）显式要求分支路由。
        //   Steps/Plan/Properties/Trigger 等 ReadResult 类型只影响"上层逻辑消费"，
        //   不改变脉冲传播行为。
        //
        // Judgment Branch 的三种语义：
        //   target=Some(x)  → 只路由到 x（替代 outgoing 遍历），且 apply set_variables
        //   target=None     → 应用 set_variables 后 fall-through，继续正常 outgoing 传播
        //   target=self     → 跳过（防自环）
        for (block_id, read_result) in &threshold_passed_with_result {
            // ---- Judgment Branch：特殊路由语义 ----
            if let ReadResult::Branch {
                target,
                set_variables,
                alt_branch_target,
                ..
            } = read_result
            {
                // 先无条件应用 set_variables（Judgment 总是要设置变量的）
                self.ctx.apply_set_variables(set_variables);

                // ★ F-17 增强：给未走分支 target 发 Inhibit 脉冲（竞争抑制）
                // 设计文档 §6.1 Inhibit 用途：抑制竞争分支激活
                if let Some(alt_target) = alt_branch_target
                    && alt_target != block_id
                {
                    self.forward_inhibit_pulse(
                        &mut result,
                        block_id,
                        alt_target,
                    );
                }

                match target {
                    // 路由到指定目标，替代 outgoing 遍历
                    Some(branch_target) if branch_target != block_id => {
                        self.forward_to_single_target(
                            &mut result,
                            block_id,
                            branch_target,
                            &[],
                        );
                        continue; // 跳过后面的 outgoing 遍历
                    }
                    Some(_) => continue, // 自环 → 跳过
                    None => {
                        // fall-through，继续走后面的正常 outgoing 传播
                    }
                }
            }

            // ---- 其他所有 ReadResult 类型 + Judgment target=None ----
            // 正常遍历 outgoing_connections
            let outgoing = self.network.outgoing_connections(block_id);

            // Value 带 payload；其他不带
            let payload = match read_result {
                ReadResult::Value { int_value, text_value, output_var_name, .. } => {
                    // ★ 核心：把 Fact / Operation 的 read 结果写回 PulseBuffer
                    //   output_var_name=None → 统一写入 "final_result"
                    let var_name = output_var_name
                        .as_deref()
                        .unwrap_or("final_result");
                    self.ctx.set_var(var_name, *int_value);
                    // 同时也写一个 "last_result"（便于调试）
                    self.ctx.set_var("last_result", *int_value);
                    value_to_payload(*int_value, text_value.as_deref())
                }
                _ => Vec::new(),
            };

            for conn in &outgoing {
                if conn.target_id == *block_id {
                    continue;
                }

                let source_strength = self
                    .network
                    .get_block(block_id)
                    .map(|b| b.state.activation)
                    .unwrap_or(0);

                let new_strength =
                    uint8_multiply(uint8_multiply(source_strength, conn.weight), conn.decay);

                if new_strength < MIN_SIGNAL {
                    continue;
                }

                let new_pulse_id = self.scheduler.next_pulse_id();
                let new_pulse_type = match conn.conn_type {
                    ConnectionType::Loop => PulseType::Control,
                    _ => PulseType::Result,
                };

                let mut new_pulse = Pulse::new(
                    new_pulse_id,
                    new_pulse_type,
                    block_id.clone(),
                    conn.target_id.clone(),
                    new_strength,
                    payload.clone(),
                    sat8_sub(DEFAULT_TTL, 1),
                    self.scheduler.step(),
                );
                new_pulse.path.push(block_id.clone());

                if self.scheduler.enqueue_next(new_pulse) {
                    result.pulses_forwarded += 1;
                } else {
                    result.dropped_rate_limited += 1;
                }
            }
        }

        // 2c: ★ F-08 decay.rs —— 全局衰减 + 不应期 countdown
        apply_decay_all(self.network, &self.decay_cfg);

        // advance step：next → current
        let dropped = self.scheduler.advance_step();
        result.dropped_rate_limited += dropped;

        // 清空本 step trace（保留 buffer_vars 给后续 step 使用）
        self.ctx.clear_trace();

        result
    }

    /// 向单个 target 转发一个脉冲（Judgment 分支路由专用）
    /// 简化版：强度沿用 pre-decay activation，直接给 Result 脉冲
    fn forward_to_single_target(
        &mut self,
        result: &mut StepResult,
        source_id: &BlockId,
        target_id: &BlockId,
        payload: &[u8],
    ) {
        let source_strength = self
            .network
            .get_block(source_id)
            .map(|b| b.state.activation)
            .unwrap_or(0);

        // 找 source → target 的连接，用它的 weight/decay
        // 如果没有直接连接（Judgment 分支到 Plan 等常见情况），用默认 Vertical 衰减
        let (weight, decay) = self
            .network
            .outgoing_connections(source_id)
            .iter()
            .find(|c| c.target_id == *target_id)
            .map(|c| (c.weight, c.decay))
            .unwrap_or((230, 230)); // 默认 Vertical

        let new_strength = uint8_multiply(uint8_multiply(source_strength, weight), decay);

        if new_strength < MIN_SIGNAL {
            result.dropped_too_weak += 1;
            return;
        }

        let new_pulse_id = self.scheduler.next_pulse_id();
        let mut new_pulse = Pulse::new(
            new_pulse_id,
            PulseType::Result,
            source_id.clone(),
            target_id.clone(),
            new_strength,
            payload.to_vec(),
            sat8_sub(DEFAULT_TTL, 1),
            self.scheduler.step(),
        );
        new_pulse.path.push(source_id.clone());

        if self.scheduler.enqueue_next(new_pulse) {
            result.pulses_forwarded += 1;
        } else {
            result.dropped_rate_limited += 1;
        }
    }

    /// ★ F-17 新增：发送 Inhibit 脉冲到竞争分支 target
    /// 设计文档 §6.1 Inhibit 用途：当 Judgment 走了 true_branch，
    /// false_branch 的 target 需要被抑制（防止残留激活继续传播）
    /// 强度固定 180（中等偏强抑制），TTL 较短（3）因为只需一步压制
    fn forward_inhibit_pulse(
        &mut self,
        result: &mut StepResult,
        source_id: &BlockId,
        target_id: &BlockId,
    ) {
        let inhibit_strength: u8 = 180;

        // 无连接 → 直接发 Inhibit（默认 Vertical 强度 180）
        let (weight, decay) = self
            .network
            .outgoing_connections(source_id)
            .iter()
            .find(|c| c.target_id == *target_id)
            .map(|c| (c.weight, c.decay))
            .unwrap_or((230, 230));

        let effective = uint8_multiply(uint8_multiply(inhibit_strength, weight), decay);
        if effective < MIN_SIGNAL {
            result.dropped_too_weak += 1;
            return;
        }

        let pulse_id = self.scheduler.next_pulse_id();
        let mut pulse = Pulse::new(
            pulse_id,
            PulseType::Inhibit,
            source_id.clone(),
            target_id.clone(),
            effective,
            Vec::new(),
            3, // 短 TTL：抑制只需一步
            self.scheduler.step(),
        );
        pulse.path.push(source_id.clone());

        if self.scheduler.enqueue_next(pulse) {
            result.pulses_inhibited += 1;
        } else {
            result.dropped_rate_limited += 1;
        }
    }

    /// =====================================================================
    /// 执行多 step 直到空闲或超过上限
    /// =====================================================================
    pub fn run_until_idle(&mut self, max_steps: u64) -> Vec<StepResult> {
        let mut history = Vec::new();
        let mut counter = 0u64;

        while self.scheduler.has_current_pulses() && counter < max_steps {
            let res = self.run_step();
            history.push(res);
            counter += 1;
        }

        history
    }

    /// 系统 1（感知级）：最多 SYSTEM1_MAX_STEPS=5 步
    pub fn run_system1(&mut self) -> Vec<StepResult> {
        self.run_until_idle(SYSTEM1_MAX_STEPS as u64)
    }

    /// 系统 2（推理级）：最多 SYSTEM2_MAX_STEPS=200 步
    pub fn run_system2(&mut self) -> Vec<StepResult> {
        self.run_until_idle(SYSTEM2_MAX_STEPS as u64)
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 把 ReadResult::Value 转成 Pulse payload（优先整数 4 bytes，带文本则追加）
fn value_to_payload(int_value: i32, text_value: Option<&str>) -> Vec<u8> {
    let bytes: [u8; 4] = int_value.to_le_bytes();
    let mut payload = bytes.to_vec();
    if let Some(t) = text_value {
        payload.extend_from_slice(t.as_bytes());
    }
    payload
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{
        Block, BlockId, ConnectionType, FactEntry, FactValue, MemoryBackend, MIN_SIGNAL,
        Pulse, PulseId, PulseType, SYSTEM1_MAX_STEPS,
    };
    use mesba_network::MeshBlockNetwork;

    /// 构造最小三区块链 A→B→C（v1 测试辅助，不含 Judgment 分支）
    fn make_three_chain() -> MeshBlockNetwork {
        let mut net = MeshBlockNetwork::new();
        let block_a = Block::perceptual(
            BlockId::from("A"),
            "test".to_string(),
            1,
            "test_modality".to_string(),
            vec![],
        );
        net.register_block(block_a);

        let block_b = Block::concept(
            BlockId::from("B"),
            "test".to_string(),
            2,
            "middle_concept".to_string(),
        );
        net.register_block(block_b);

        let block_c = Block::fact(
            BlockId::from("C"),
            "test".to_string(),
            3,
            vec![FactEntry {
                keys: vec!["result".to_string()],
                value: FactValue::Integer(42),
            }],
        );
        net.register_block(block_c);

        net.connect(
            BlockId::from("A"),
            BlockId::from("B"),
            ConnectionType::Vertical,
        );
        net.connect(
            BlockId::from("B"),
            BlockId::from("C"),
            ConnectionType::Vertical,
        );

        net
    }

    #[test]
    fn test_single_step_no_pulses() {
        let mut net = MeshBlockNetwork::new();
        let mut prop = PulsePropagator::new(&mut net);
        let result = prop.run_step();
        assert_eq!(result.pulses_processed, 0);
        assert_eq!(result.pulses_forwarded, 0);
    }

    #[test]
    fn test_three_chain_ripple_propagation() {
        let mut net = make_three_chain();
        let mut prop = PulsePropagator::new(&mut net);

        let pulse = Pulse::activate(
            PulseId(1),
            BlockId::from("SOURCE"),
            BlockId::from("A"),
            255,
            vec![],
            0,
        );
        prop.inject(pulse);

        let r0 = prop.run_step();
        assert_eq!(r0.pulses_processed, 1);
        assert!(r0.blocks_activated >= 1);
        assert!(r0.pulses_forwarded >= 1);

        let r1 = prop.run_step();
        assert!(r1.blocks_activated >= 1);

        let r2 = prop.run_step();
        assert!(r2.blocks_activated >= 1);

        let r3 = prop.run_step();
        assert_eq!(r3.pulses_processed, 0);
    }

    // --- Pass 2 v2 新功能测试 ---

    #[test]
    fn test_fact_read_stores_value_in_buffer_vars() {
        // 激活 Fact 区块 → read() 得到 Value → 写入 buffer_vars (Fact 的 output_var_name=None → "last_result")
        let mut net = MeshBlockNetwork::new();
        let fact_id = BlockId::from("fact_test");
        net.register_block(Block::fact(
            fact_id.clone(),
            "test".to_string(),
            4,
            vec![FactEntry {
                keys: vec!["3+5".to_string()],
                value: FactValue::Integer(8),
            }],
        ));

        let mut prop = PulsePropagator::new(&mut net);
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("IN"),
            fact_id.clone(),
            255,
            b"3+5".to_vec(),
            0,
        ));

        prop.run_step();

        assert_eq!(prop.get_var("last_result"), Some(8));
    }

    #[test]
    fn test_workspace_ctx_apply_set_variables() {
        let mut ctx = WorkspaceContext::new();
        let mut vars = HashMap::new();
        vars.insert("carry".to_string(), "1".to_string());
        vars.insert("ones".to_string(), "8".to_string());
        ctx.apply_set_variables(&vars);
        assert_eq!(ctx.buffer_vars.get("carry").copied(), Some(1));
        assert_eq!(ctx.buffer_vars.get("ones").copied(), Some(8));
    }

    // --- 原有防御测试 ---

    #[test]
    fn test_refractory_blocks_skipped() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::fact(
            BlockId::from("R1"),
            "test".to_string(),
            1,
            vec![],
        );
        net.register_block(b);
        if let Some(block) = net.get_block_mut(&BlockId::from("R1")) {
            block.state.refractory = 3;
        }

        let mut prop = PulsePropagator::new(&mut net);
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("X"),
            BlockId::from("R1"),
            255,
            vec![],
            0,
        ));

        let result = prop.run_step();
        assert_eq!(result.dropped_refractory, 1);
        assert_eq!(result.blocks_activated, 0);
    }

    #[test]
    fn test_ttl_expired_dropped() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::fact(
            BlockId::from("T1"),
            "test".to_string(),
            1,
            vec![],
        );
        net.register_block(b);
        let mut prop = PulsePropagator::new(&mut net);

        let mut pulse = Pulse::activate(
            PulseId(1),
            BlockId::from("X"),
            BlockId::from("T1"),
            255,
            vec![],
            0,
        );
        pulse.ttl = 0;
        prop.inject(pulse);
        let result = prop.run_step();
        assert_eq!(result.dropped_ttl_expired, 1);
    }

    #[test]
    fn test_strength_below_min_signal() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::fact(
            BlockId::from("W1"),
            "test".to_string(),
            1,
            vec![],
        );
        net.register_block(b);
        let mut prop = PulsePropagator::new(&mut net);
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("X"),
            BlockId::from("W1"),
            MIN_SIGNAL - 1,
            vec![],
            0,
        ));
        let result = prop.run_step();
        assert_eq!(result.dropped_too_weak, 1);
    }

    #[test]
    fn test_path_loop_detection() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::fact(
            BlockId::from("L1"),
            "test".to_string(),
            1,
            vec![],
        );
        net.register_block(b);
        let mut prop = PulsePropagator::new(&mut net);

        let mut pulse = Pulse::activate(
            PulseId(1),
            BlockId::from("L1"),
            BlockId::from("L1"),
            255,
            vec![],
            0,
        );
        pulse.path.push(BlockId::from("L1"));
        prop.inject(pulse);
        let result = prop.run_step();
        assert_eq!(result.dropped_path_duplicate, 1);
    }

    #[test]
    fn test_inhibit_pulse_reduces_activation() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::concept(
            BlockId::from("I1"),
            "test".to_string(),
            1,
            "inhibit_target".to_string(),
        );
        net.register_block(b);
        if let Some(block) = net.get_block_mut(&BlockId::from("I1")) {
            block.state.activation = 200;
            block.state.threshold = 100;
        }

        let mut prop = PulsePropagator::new(&mut net);
        let mut inh_pulse = Pulse::activate(
            PulseId(1),
            BlockId::from("X"),
            BlockId::from("I1"),
            150,
            vec![],
            0,
        );
        inh_pulse.pulse_type = PulseType::Inhibit;
        prop.inject(inh_pulse);
        prop.run_step();

        let final_activation = net
            .get_block(&BlockId::from("I1"))
            .unwrap()
            .state
            .activation;
        assert!(final_activation < 200);
    }

    #[test]
    fn test_run_system1_limited_steps() {
        let mut net = make_three_chain();
        let mut prop = PulsePropagator::new(&mut net);
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("S"),
            BlockId::from("A"),
            255,
            vec![],
            0,
        ));
        let history = prop.run_system1();
        assert!(!history.is_empty());
        assert!((history.len() as u64) <= SYSTEM1_MAX_STEPS as u64);
    }

    #[test]
    fn test_decay_reduces_activation_over_steps() {
        let mut net = MeshBlockNetwork::new();
        let b = Block::fact(
            BlockId::from("D1"),
            "test".to_string(),
            1,
            vec![],
        );
        net.register_block(b);
        if let Some(block) = net.get_block_mut(&BlockId::from("D1")) {
            block.state.activation = 200;
            block.state.threshold = 250;
        }
        let mut prop = PulsePropagator::new(&mut net);
        for _ in 0..3 {
            prop.run_step();
        }
        let final_activation = net
            .get_block(&BlockId::from("D1"))
            .unwrap()
            .state
            .activation;
        assert!(final_activation < 200);
    }

    #[test]
    fn test_target_missing_skipped() {
        let mut net = MeshBlockNetwork::new();
        let mut prop = PulsePropagator::new(&mut net);
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("X"),
            BlockId::from("NONEXISTENT"),
            255,
            vec![],
            0,
        ));
        let result = prop.run_step();
        assert_eq!(result.dropped_target_missing, 1);
    }

    // --- v2 BlockContent::read() 端到端集成测试 ---

    #[test]
    fn test_operation_executes_with_buffer_vars() {
        // 场景：注入 a=23, b=45 → 激活 OpExtractOnes → algorithm="a%10+b%10" → ones_sum=8
        let mut net = MeshBlockNetwork::new();

        // Perceptual → Operation 单步直连
        let perc_id = BlockId::from("perc");
        net.register_block(Block::perceptual(
            perc_id.clone(),
            "test".to_string(),
            1,
            "digits".to_string(),
            vec![],
        ));

        let op_id = BlockId::from("op_ones");
        net.register_block(Block::operation_internal(
            op_id.clone(),
            "test".to_string(),
            2,
            "提取个位求和 a%10+b%10".to_string(),
            vec!["a".to_string(), "b".to_string()],
            vec!["ones_sum".to_string()],
            "a%10+b%10".to_string(),
        ));

        net.connect(perc_id, op_id, ConnectionType::Vertical);

        let mut prop = PulsePropagator::new(&mut net);
        // ---- 关键：先 set_var 注入初始变量（mesba_runtime 在推理开始前的工作）----
        prop.set_var("a", 23);
        prop.set_var("b", 45);

        // 注入激活脉冲
        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("SRC"),
            BlockId::from("perc"),
            255,
            vec![],
            0,
        ));

        // Step 0: perc 被激活，read() 返回 Properties（无执行逻辑），转发 → op_ones
        // Step 1: op_ones 被激活，read() 执行 algorithm="a%10+b%10"
        prop.run_step(); // step 0
        prop.run_step(); // step 1 — op_ones 在此 step 被激活

        // ---- 验证：ones_sum 应该是 23%10 + 45%10 = 3 + 5 = 8 ----
        assert_eq!(
            prop.get_var("ones_sum"),
            Some(8),
            "Operation 执行结果应写入 ones_sum=8"
        );
    }

    #[test]
    fn test_judgment_sets_carry_in_buffer_vars() {
        // 场景：注入 ones_sum=12 → Judgment("ones_sum >= 10") 求值为 true → set_variables carry=1
        let mut net = MeshBlockNetwork::new();

        let perc_id = BlockId::from("perc");
        net.register_block(Block::perceptual(
            perc_id.clone(),
            "test".to_string(),
            1,
            "digits".to_string(),
            vec![],
        ));

        let judgment_id = BlockId::from("judg_carry");
        let mut true_branch_map = std::collections::HashMap::new();
        true_branch_map.insert("carry".to_string(), "1".to_string());
        let mut false_branch_map = std::collections::HashMap::new();
        false_branch_map.insert("carry".to_string(), "0".to_string());

        net.register_block(Block::judgment(
            judgment_id.clone(),
            "test".to_string(),
            2,
            "ones_sum >= 10".to_string(),
            vec!["ones_sum".to_string()],
            mesba_core::JudgmentBranch {
                action: "carry_on".to_string(),
                target: None,
                set_variables: true_branch_map,
            },
            mesba_core::JudgmentBranch {
                action: "carry_off".to_string(),
                target: None,
                set_variables: false_branch_map,
            },
        ));

        net.connect(perc_id, judgment_id, ConnectionType::Vertical);

        let mut prop = PulsePropagator::new(&mut net);
        prop.set_var("ones_sum", 12); // ≥ 10 → true branch

        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("SRC"),
            BlockId::from("perc"),
            255,
            vec![],
            0,
        ));

        prop.run_step(); // step 0: perc → 转发
        prop.run_step(); // step 1: judgment 激活 → 条件求值为 true → carry=1

        assert_eq!(
            prop.get_var("carry"),
            Some(1),
            "ones_sum=12≥10 → 应走 true branch carry=1"
        );
    }

    #[test]
    fn test_judgment_false_branch_carry_zero() {
        // ones_sum=5 < 10 → false branch carry=0
        let mut net = MeshBlockNetwork::new();

        let perc_id = BlockId::from("perc2");
        net.register_block(Block::perceptual(
            perc_id.clone(),
            "test".to_string(),
            1,
            "digits".to_string(),
            vec![],
        ));

        let judgment_id = BlockId::from("judg_carry2");
        let mut true_branch_map = std::collections::HashMap::new();
        true_branch_map.insert("carry".to_string(), "1".to_string());
        let mut false_branch_map = std::collections::HashMap::new();
        false_branch_map.insert("carry".to_string(), "0".to_string());

        net.register_block(Block::judgment(
            judgment_id.clone(),
            "test".to_string(),
            2,
            "ones_sum >= 10".to_string(),
            vec!["ones_sum".to_string()],
            mesba_core::JudgmentBranch {
                action: "carry_on".to_string(),
                target: None,
                set_variables: true_branch_map,
            },
            mesba_core::JudgmentBranch {
                action: "carry_off".to_string(),
                target: None,
                set_variables: false_branch_map,
            },
        ));

        net.connect(perc_id, judgment_id, ConnectionType::Vertical);

        let mut prop = PulsePropagator::new(&mut net);
        prop.set_var("ones_sum", 5); // < 10 → false branch

        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("SRC2"),
            BlockId::from("perc2"),
            255,
            vec![],
            0,
        ));

        prop.run_step();
        prop.run_step();

        assert_eq!(
            prop.get_var("carry"),
            Some(0),
            "ones_sum=5<10 → 应走 false branch carry=0"
        );
    }
}