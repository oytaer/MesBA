// ============================================================================
// scheduler.rs —— 事件队列调度器（双队列 Buffer + Step 计数 + 限流）
// ============================================================================
// 设计文档 §6.3 脉冲传播算法 + §6.4 防爆炸六道防线
//
// 核心设计：
//   - 每个 step 使用两个 VecDeque<Pulse>：current_step + next_step
//   - Propagator 处理 current_step，新产生的脉冲入 next_step
//   - step 结束后交换队列，MAX_PULSES_PER_STEP=1024 限流
//   - step 计数器：SYSTEM1_MAX_STEPS=5 / SYSTEM2_MAX_STEPS=200
//
// 双队列设计好处：
//   1. 明确 step 边界 —— 防止新脉冲在当前 step 被递归处理
//   2. 限流简单 —— 交换时直接 truncate
//   3. borrow checker 友好 —— 避免同时 read+write 同一队列
// ============================================================================

use std::collections::VecDeque;

use mesba_core::{MAX_PULSES_PER_STEP, Pulse, PulseId};

/// =========================================================================
/// PulseScheduler —— 脉冲事件队列调度器
/// =========================================================================
#[derive(Debug, Default)]
pub struct PulseScheduler {
    /// 当前 step 待处理脉冲队列
    current: VecDeque<Pulse>,
    /// 下一个 step 的脉冲队列
    next: VecDeque<Pulse>,
    /// 全局脉冲 ID 计数器（保证唯一性）
    pulse_id_counter: u64,
    /// 当前 step 编号（从 0 开始）
    step: u64,
}

impl PulseScheduler {
    pub fn new() -> Self {
        let cap = MAX_PULSES_PER_STEP as usize;
        Self {
            current: VecDeque::with_capacity(cap),
            next: VecDeque::with_capacity(cap),
            pulse_id_counter: 0,
            step: 0,
        }
    }

    // -----------------------------------------------------------------------
    // 基础查询
    // -----------------------------------------------------------------------

    /// 当前 step 编号
    pub fn step(&self) -> u64 {
        self.step
    }

    /// 当前 step 待处理脉冲数
    pub fn current_len(&self) -> usize {
        self.current.len()
    }

    /// 下一个 step 已入队脉冲数
    pub fn next_len(&self) -> usize {
        self.next.len()
    }

    /// 当前 step 是否有脉冲待处理
    pub fn has_current_pulses(&self) -> bool {
        !self.current.is_empty()
    }

    // -----------------------------------------------------------------------
    // 脉冲入队
    // -----------------------------------------------------------------------

    /// 向当前 step 队列注入脉冲（外部调用如 injector 初始激活）
    pub fn inject_pulse(&mut self, pulse: Pulse) {
        self.current.push_back(pulse);
    }

    /// 向下一个 step 队列追加脉冲（Propagator 转发时调用）
    /// 超过 MAX_PULSES_PER_STEP 时丢弃（防爆炸第 6 道防线）
    pub fn enqueue_next(&mut self, pulse: Pulse) -> bool {
        let max = MAX_PULSES_PER_STEP as usize;
        if self.next.len() >= max {
            return false;
        }
        self.next.push_back(pulse);
        true
    }

    // -----------------------------------------------------------------------
    // 脉冲生成辅助（给 Propagator 转发用）
    // -----------------------------------------------------------------------

    /// 分配新脉冲 ID（线程内唯一）
    pub fn next_pulse_id(&mut self) -> PulseId {
        self.pulse_id_counter += 1;
        PulseId(self.pulse_id_counter)
    }

    // -----------------------------------------------------------------------
    // Step 生命周期
    // -----------------------------------------------------------------------

    /// 取出当前队列全部脉冲（供 Propagator Pass 1 遍历）
    pub fn drain_current(&mut self) -> Vec<Pulse> {
        self.current.drain(..).collect()
    }

    /// 推进到下一个 step：next → current，清空 next，step++
    /// 返回本 step 被丢弃的脉冲数（超过限流的）
    pub fn advance_step(&mut self) -> u64 {
        self.step += 1;

        let max = MAX_PULSES_PER_STEP as usize;
        // 交换队列
        let dropped = if self.next.len() > max {
            let excess = self.next.len() - max;
            self.next.truncate(max);
            excess as u64
        } else {
            0
        };

        self.current = std::mem::take(&mut self.next);

        dropped
    }

    /// 重置调度器（清空所有队列，step 归零）
    pub fn reset(&mut self) {
        self.current.clear();
        self.next.clear();
        self.pulse_id_counter = 0;
        self.step = 0;
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{BlockId, Pulse, PulseType};

    fn make_test_pulse(target: &str) -> Pulse {
        Pulse::new(
            PulseId(0),
            PulseType::Activate,
            BlockId::from("A"),
            BlockId::from(target),
            200,
            vec![],
            5,
            0,
        )
    }

    #[test]
    fn test_scheduler_basic_lifecycle() {
        let mut sched = PulseScheduler::new();

        // 初始为空
        assert_eq!(sched.step(), 0);
        assert_eq!(sched.current_len(), 0);
        assert!(!sched.has_current_pulses());

        // 注入
        sched.inject_pulse(make_test_pulse("B"));
        assert_eq!(sched.current_len(), 1);
        assert!(sched.has_current_pulses());

        // drain + advance
        let drained = sched.drain_current();
        assert_eq!(drained.len(), 1);
        assert_eq!(sched.current_len(), 0);

        sched.advance_step();
        assert_eq!(sched.step(), 1);
        assert_eq!(sched.current_len(), 0); // next 也是空的
    }

    #[test]
    fn test_advance_with_next_pulses() {
        let mut sched = PulseScheduler::new();

        sched.inject_pulse(make_test_pulse("B"));

        // 模拟 Propagator 处理后产生 next pulse
        sched.enqueue_next(make_test_pulse("C"));
        assert_eq!(sched.next_len(), 1);

        let _ = sched.drain_current();
        sched.advance_step();

        assert_eq!(sched.step(), 1);
        assert_eq!(sched.current_len(), 1);
        assert_eq!(sched.next_len(), 0);
    }

    #[test]
    fn test_max_pulses_per_step_limiting() {
        let mut sched = PulseScheduler::new();
        let max = MAX_PULSES_PER_STEP as usize;

        // 填满到 MAX
        for _ in 0..max {
            assert!(sched.enqueue_next(make_test_pulse("X")));
        }
        // 超过 MAX 应该返回 false
        assert!(!sched.enqueue_next(make_test_pulse("Y")));

        // advance 时应该 truncate
        sched.advance_step();
        assert_eq!(sched.current_len(), max);
    }

    #[test]
    fn test_pulse_id_monotonic() {
        let mut sched = PulseScheduler::new();
        let id1 = sched.next_pulse_id();
        let id2 = sched.next_pulse_id();
        let id3 = sched.next_pulse_id();
        assert!(id2.0 > id1.0);
        assert!(id3.0 > id2.0);
    }

    #[test]
    fn test_reset() {
        let mut sched = PulseScheduler::new();
        sched.inject_pulse(make_test_pulse("A"));
        sched.enqueue_next(make_test_pulse("B"));
        sched.advance_step();
        sched.reset();

        assert_eq!(sched.step(), 0);
        assert_eq!(sched.current_len(), 0);
        assert_eq!(sched.next_len(), 0);
    }
}
