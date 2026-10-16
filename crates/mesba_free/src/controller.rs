// ============================================================================
// controller.rs —— FreeMechanismController 主体（十模块中枢 + mem::take workspace）
// ============================================================================
// 设计文档 §8 游离控制系统：
//   - 顶层 Controller 持有 MeshBlockNetwork + GlobalWorkspace
//   - 每 tick 从当前认知环分发到对应模块 tick() 方法
//   - mem::take 模式解决 workspace 所有权问题：
//     先 take workspace → PulsePropagator::with_memory → run N step
//     → 从 propagator 取回 workspace → Controller 恢复持有
//
// 十模块设计（每个独立 .rs 文件，一个 tick(&mut Controller) 方法）：
//   1. GoalManager       — 目标生命周期管理
//   2. Perceive          — 感知输入 → PulseBuffer 变量注入
//   3. AttentionRegulator — 注意力聚焦 → 增强关键 slot
//   4. Emotion           — 情感快速通道 → React 优先路由
//   5. Understand        — System 1 快速传播
//   6. Reason            — System 2 深度推理
//   7. Plan              — 子目标分解 → GoalStack push
//   8. Execute           — 执行脉冲注入 + 外部工具桥接
//   9. Feedback          — 结果比对 → Experience 区块写入
//  10. Transfer          — Hebbian 学习 + 连接塑形
// ============================================================================

use mesba_core::{BlockId, MemoryBackend, Pulse};
use mesba_engine::PulsePropagator;
use mesba_memory::GlobalWorkspace;
use mesba_network::MeshBlockNetwork;

use crate::state_machine::CognitiveRing;
use crate::{
    attention::AttentionRegulator, emotion::Emotion, execute::Executor,
    feedback::Feedback, goal_manager::GoalManager, understand::Understand,
    perceive::Perceive, plan::Planner, reason::Reason, transfer::Transfer,
};

// ============================================================================
// FreeMechanismController
// ============================================================================
pub struct FreeMechanismController {
    /// 网状区块网络（所有区块 + 连接）
    pub network: MeshBlockNetwork,
    /// 全局工作记忆（PerceptualBuffer + PulseBuffer + GoalStack + Trace）
    pub workspace: GlobalWorkspace,
    /// 当前认知环状态
    current_ring: CognitiveRing,
    /// 最近一次传播的统计（供 Understand→React/Reason 判定用）
    last_propagation: PropagationStats,
    /// 连续无阈值激活的 step 数（不确定性判定）
    consecutive_non_activations: u64,

    // ---- ★ F-19 防死循环字段 ----
    /// 每个认知环被**连续**访问次数（切换环时全部清零）
    state_visit_count: std::collections::HashMap<CognitiveRing, u32>,
    /// 全局 tick 计数（每个 tick +1，到达 max_session_steps 触发看门狗）
    total_ticks: u64,
    /// 全局最大步数（默认 System2 上限 200）
    max_session_steps: u64,
    /// 连续同环访问 ≥3 次 / 或 total_ticks ≥ max_session_steps → 置 true
    /// 外部 API 层检查 kill_flag 决定提前终止推理
    pub kill_flag: bool,
    /// 上次 kill 原因（调试用）
    pub kill_reason: Option<String>,

    // ---- ★ F-20 kill_flag → ErrorPulse 桥接 ----
    /// 待注入的 ErrorPulse（由 kill_flag 触发后，下次 run_propagation_* 时注入）
    pending_error_pulse: Option<Pulse>,

    // ---- 十模块（独立 .rs 文件实现，此处只是持有）----
    pub goal_manager: GoalManager,
    pub perceive: Perceive,
    pub attention: AttentionRegulator,
    pub emotion: Emotion,
    pub understand: Understand,
    pub reason: Reason,
    pub plan: Planner,
    pub execute: Executor,
    pub feedback: Feedback,
    pub transfer: Transfer,
}

/// 最近一次传播的统计快照
#[derive(Debug, Clone, Default)]
pub struct PropagationStats {
    pub pulses_processed: u64,
    pub blocks_activated: u64,
    pub pulses_forwarded: u64,
}

impl FreeMechanismController {
    // ========================================================================
    // 构造
    // ========================================================================

    pub fn new(network: MeshBlockNetwork) -> Self {
        Self::with_workspace(network, GlobalWorkspace::new())
    }

    pub fn with_workspace(network: MeshBlockNetwork, workspace: GlobalWorkspace) -> Self {
        Self {
            network,
            workspace,
            current_ring: CognitiveRing::Perceive,
            last_propagation: PropagationStats::default(),
            consecutive_non_activations: 0,
            // ---- ★ F-19 防死循环初始化 ----
            state_visit_count: std::collections::HashMap::new(),
            total_ticks: 0,
            max_session_steps: 200, // System 2 上限
            kill_flag: false,
            kill_reason: None,
            // ---- ★ F-20 pending_error_pulse 初始为空 ----
            pending_error_pulse: None,
            // ---- 十模块 ----
            goal_manager: GoalManager::new(),
            perceive: Perceive::new(),
            attention: AttentionRegulator::new(),
            emotion: Emotion::new(),
            understand: Understand::new(),
            reason: Reason::new(),
            plan: Planner::new(),
            execute: Executor::new(),
            feedback: Feedback::new(),
            transfer: Transfer::new(),
        }
    }

    // ========================================================================
    // 认知八环状态机对外接口
    // ========================================================================

    pub fn current_ring(&self) -> CognitiveRing {
        self.current_ring
    }

    /// 对外暴露总 tick 计数（F-15 Checkpoint 需要）
    pub fn total_ticks(&self) -> u64 {
        self.total_ticks
    }

    pub fn set_ring(&mut self, ring: CognitiveRing) {
        // ★ F-19：切换环 → 所有连续访问计数清零（换环即断"连续"）
        if ring != self.current_ring {
            self.state_visit_count.clear();
        }
        self.current_ring = ring;
    }

    // ========================================================================
    // ★ F-19 防死循环看门狗
    // ========================================================================

    /// 每次 tick 调用：推进 total_ticks + 累加当前环连续访问次数 + 检查看门狗
    /// 返回 true = kill（外部停止 tick 执行）
    pub(crate) fn advance_tick_watchdog(&mut self) -> bool {
        // 上一次已 kill → 直接返回
        if self.kill_flag {
            return true;
        }

        self.total_ticks += 1;
        let current_ring = self.current_ring; // 提前存，避免后续 borrow 冲突
        let cnt = self
            .state_visit_count
            .entry(current_ring)
            .or_insert(0);
        *cnt += 1;

        // 看门狗 1：连续同环 ≥3（状态循环）
        if *cnt >= 3 {
            let reason = format!(
                "防死循环：CognitiveRing::{:?} 连续访问 {} 次",
                current_ring, *cnt
            );
            self.kill_flag = true;
            self.kill_reason = Some(reason.clone());
            // ★ 桥接 2: kill_flag → ErrorPulse 注入
            self.pending_error_pulse = Some(Pulse::new(
                mesba_core::PulseId(self.total_ticks + 1000),
                mesba_core::PulseType::Error,
                BlockId::from("kill_watchdog"),
                BlockId::from("GLOBAL"),
                255,
                Pulse::payload_from_string(&reason),
                mesba_core::value::DEFAULT_TTL,
                self.total_ticks,
            ));
            return true;
        }
        // 看门狗 2：全局步数上限
        if self.total_ticks >= self.max_session_steps {
            let reason = format!(
                "防死循环：超过最大步数 {}（已执行 {}）",
                self.max_session_steps, self.total_ticks
            );
            self.kill_flag = true;
            self.kill_reason = Some(reason.clone());
            // ★ 桥接 2: kill_flag → ErrorPulse 注入
            self.pending_error_pulse = Some(Pulse::new(
                mesba_core::PulseId(self.total_ticks + 2000),
                mesba_core::PulseType::Error,
                BlockId::from("kill_watchdog"),
                BlockId::from("GLOBAL"),
                255,
                Pulse::payload_from_string(&reason),
                mesba_core::value::DEFAULT_TTL,
                self.total_ticks,
            ));
            return true;
        }
        false
    }

    // ========================================================================
    // ★ F-20 kill_flag → ErrorPulse 桥接
    // ========================================================================

    /// 供外部（advance_tick_watchdog 或 runtime）注入待处理的 ErrorPulse
    /// 下一次 run_propagation_system1/system2 时自动注入 Propagator
    pub fn inject_pending_error_pulse(&mut self, p: Pulse) {
        self.pending_error_pulse = Some(p);
    }

    pub fn last_propagation_blocks_activated(&self) -> u64 {
        self.last_propagation.blocks_activated
    }

    pub fn consecutive_non_activations(&self) -> u64 {
        self.consecutive_non_activations
    }

    pub fn set_last_propagation(&mut self, stats: PropagationStats) {
        self.last_propagation = stats.clone();
        if stats.blocks_activated == 0 {
            self.consecutive_non_activations += 1;
        } else {
            self.consecutive_non_activations = 0;
        }
    }

    pub fn reset_consecutive_non_activations(&mut self) {
        self.consecutive_non_activations = 0;
    }

    // ========================================================================
    // mem::take workspace —— 解决 PulsePropagator 所有权冲突
    // ========================================================================
    // Advisor 核心建议：Controller 持有 GlobalWorkspace，
    // PulsePropagator::with_memory 需要 M: MemoryBackend（GlobalWorkspace）。
    // std::mem::take 把 workspace 移给 propagator，propagator drop 后移回。
    // ========================================================================

    /// 取出 workspace（mem::take 模式）
    pub fn take_workspace(&mut self) -> GlobalWorkspace {
        std::mem::take(&mut self.workspace)
    }

    /// 放回 workspace（或用新的覆盖）
    pub fn put_workspace(&mut self, ws: GlobalWorkspace) {
        self.workspace = ws;
    }

    // ========================================================================
    // 高层：System 1 / System 2 传播
    // ========================================================================

    /// System 1 快速传播（最多 SYSTEM1_MAX_STEPS=5 step）
    pub fn run_propagation_system1(&mut self) -> PropagationStats {
        let workspace = self.take_workspace();
        let mut prop = PulsePropagator::with_memory(&mut self.network, workspace);

        // ★ 桥接：先注入 pending_error_pulse（如果有）
        if let Some(err_pulse) = self.pending_error_pulse.take() {
            prop.inject(err_pulse);
        }

        let max_steps = mesba_core::SYSTEM1_MAX_STEPS as u64;
        let mut total_activated = 0u64;
        let mut total_forwarded = 0u64;
        let mut total_processed = 0u64;

        for _ in 0..max_steps {
            if !prop.scheduler().has_current_pulses() {
                break;
            }
            let res = prop.run_step();
            total_processed += res.pulses_processed;
            total_activated += res.blocks_activated;
            total_forwarded += res.pulses_forwarded;
        }

        // 把 workspace 取回来放回 Controller
        let ws = prop.retake_memory();
        self.put_workspace(ws);

        let stats = PropagationStats {
            pulses_processed: total_processed,
            blocks_activated: total_activated,
            pulses_forwarded: total_forwarded,
        };
        self.set_last_propagation(stats.clone());
        stats
    }

    /// System 2 深度传播（最多 SYSTEM2_MAX_STEPS=200 step）
    pub fn run_propagation_system2(&mut self) -> PropagationStats {
        let workspace = self.take_workspace();
        let mut prop = PulsePropagator::with_memory(&mut self.network, workspace);

        // ★ 桥接：先注入 pending_error_pulse（如果有）
        if let Some(err_pulse) = self.pending_error_pulse.take() {
            prop.inject(err_pulse);
        }

        let max_steps = mesba_core::SYSTEM2_MAX_STEPS as u64;
        let mut total_activated = 0u64;
        let mut total_forwarded = 0u64;
        let mut total_processed = 0u64;

        for _ in 0..max_steps {
            if !prop.scheduler().has_current_pulses() {
                break;
            }
            let res = prop.run_step();
            total_processed += res.pulses_processed;
            total_activated += res.blocks_activated;
            total_forwarded += res.pulses_forwarded;
        }

        let ws = prop.retake_memory();
        self.put_workspace(ws);

        let stats = PropagationStats {
            pulses_processed: total_processed,
            blocks_activated: total_activated,
            pulses_forwarded: total_forwarded,
        };
        self.set_last_propagation(stats.clone());
        stats
    }

    // ========================================================================
    // 高层：单次 tick 驱动完整状态机
    // ========================================================================
    pub fn run_single_tick(&mut self) {
        crate::state_machine::tick(self);
    }

    /// 连续 run_tick 直到 Transfer 结束 + 循环 Perceive（最多上限防死循环）
    pub fn run_agent_loop(&mut self, max_ticks: u64) -> u64 {
        let mut ticks = 0u64;
        while ticks < max_ticks {
            let was_transfer = self.current_ring == CognitiveRing::Transfer;
            crate::state_machine::tick(self);
            ticks += 1;
            // Transfer 后 → Perceive → 如果没有新感知输入，说明任务结束
            if was_transfer
                && self.current_ring == CognitiveRing::Perceive
                && self.workspace.goal_stack.is_empty()
            {
                break;
            }
        }
        ticks
    }

    // ========================================================================
    // 辅助：查询 workspace 变量（供各模块 tick 使用）
    // ========================================================================
    pub fn get_var(&self, name: &str) -> Option<i32> {
        self.workspace.pulse_buffer.slots_iter()
            .find(|s| s.occupied && s.name == name)
            .map(|s| s.value)
    }

    pub fn set_var(&mut self, name: impl Into<String>, value: i32) {
        self.workspace.pulse_buffer.set_var(&name.into(), value);
    }

    /// 辅助：向 PulseScheduler 注入激活脉冲（各模块通用）
    pub fn inject_activate(&mut self, target: BlockId, strength: u8) {
        // 我们不直接调 scheduler，而是用 System 1 传播的入口：
        // Perceive 模块（state=Perceive tick）会从 PerceptualBuffer 把目标转成
        // 变量注入到 PulseBuffer，然后 run_propagation_system1 自然触发。
        // MVP 简化：直接 set_var 占位，让 Understand 模块的 propagate 读取
        let _ = target; // 目标由模块逻辑决定
        let _ = strength;
        // 实际脉冲注入由各模块 tick() 调用 run_propagation_* 完成
    }

    // ========================================================================
    // 辅助：Network 类型计数（供 state_machine decide_system 用）
    // ========================================================================
    pub fn experience_count(&self) -> usize {
        self.network.find_by_type(mesba_core::BlockType::Experience).len()
    }

    // ========================================================================
    // §10.4 四个 uint8 标度计算函数（严格按设计文档）
    // 全部返回 0~255 的整数，直接给 decide_system() 用
    // ========================================================================

    /// compute_familiarity_u8 —— 熟悉度 0~255
    /// 算法：Experience 区块数 / 总区块数 × 255 + 0~255 偏置（PulseBuffer 中有反馈记录则加成）
    /// MVP 简化：Experience 区块数 线性映射到 0~255，最多 20 个 → 255
    pub fn compute_familiarity_u8(&self) -> u8 {
        let exp_count = self.experience_count() as u16;
        // 每 2 个 Experience = 25 熟悉度，20 个 = 250，饱和
        let base = (exp_count * 25 / 2).min(250);
        // 有 Experience 时再加 5（偏置），至少保证 5+
        (base + 5).min(255) as u8
    }

    /// compute_complexity_u8 —— 复杂度 0~255
    /// 算法：本次推理激活的区块数 / 网络总区块数 × 255
    pub fn compute_complexity_u8(&self) -> u8 {
        let activated = self.last_propagation.blocks_activated as u16;
        let total = self.network.block_count().max(1) as u16;
        // 百分比 × 2.55 → uint8 标度
        let percent = (activated * 100 / total).min(100);
        (percent * 255 / 100) as u8
    }

    /// compute_risk_u8 —— 风险 0~255
    /// 算法：PulseBuffer 中的 error 值 × 20（饱和 255）
    /// 设计文档 §10.4 risk 来自 "错误历史 + 当前 confidence"
    pub fn compute_risk_u8(&self) -> u8 {
        let error = self.workspace.get_var("error").unwrap_or(0);
        let confidence = self.workspace.get_var("confidence").unwrap_or(255);
        // error 直接 → risk，每 1 个 error = 20 risk，最多 255
        // 用 saturating_abs 避免 cast_abs_to_unsigned clippy 警告
        let from_error = (error.saturating_abs() as u16 * 20).min(255);
        // 低 confidence → 额外风险
        let from_confidence = ((255 - confidence) as u16 / 2).min(127);
        (from_error + from_confidence).min(255) as u8
    }

    /// compute_uncertainty_u8 —— 不确定度 0~255
    /// 算法：连续未激活步数 × 25（饱和 255）
    /// 2 步未激活 = 50, 6 步 = 150, 11 步以上 = 255
    pub fn compute_uncertainty_u8(&self) -> u8 {
        let steps = self.consecutive_non_activations as u16;
        (steps * 25).min(255) as u8
    }
}

// ============================================================================
// 扩展：PulsePropagator 的 retake_memory 方法
// ============================================================================
// Controller 用 mem::take 取出 workspace，Propagator 用完后需能取出来
// 放回 Controller。给 PulsePropagator<GlobalWorkspace> 加一个 retake_memory。
// 因为 Propagator 是泛型的，我们用一个 trait bound 让它能 retake。
// 但我们已经在 Controller 里用 take_workspace/put_workspace 替代了：
//   实际上 run_propagation_system1 里用的是 workspace: GlobalWorkspace，
//   我们手动构造 PulsePropagator::with_memory(network, workspace)，
//   workspace 被 move 进去，但 propag 自己无法吐出来（它是 M 泛型）。
//
// 所以我们需要 PulsePropagator 持有 M 并能返回它。给 PulsePropagator<M> 加：
//   pub fn retake_memory(self) -> M { self.ctx }
// 这里我们在 Controller 里通过 retake_memory 获取。
// ============================================================================
