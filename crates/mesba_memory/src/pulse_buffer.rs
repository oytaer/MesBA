// ============================================================================
// pulse_buffer.rs —— L1 GWS PulseBuffer（工作记忆变量表）
// ============================================================================
// 设计文档 §7.2 PulseBuffer：
//   - 容量 10~20 槽位（MVP 固定 20）
//   - fixed slot（预注册变量，永不自动淘汰）+ dynamic slot（运行时产生，衰减淘汰）
//   - 每个 step 衰减所有 dynamic slot 的 strength，低于 MIN_SIGNAL 则淘汰
//   - LRU 策略：满时淘汰最久未访问的 dynamic slot
//
// PulseBuffer 实现 MemoryBackend trait 的 var_map() 核心：
//   var_map() → 遍历所有激活槽位收集 (String, i32) 映射
// ============================================================================

use std::collections::HashMap;

use mesba_core::MIN_SIGNAL;

/// PulseBuffer 默认容量（设计文档 §7.2：10~20，取上限）
pub const DEFAULT_CAPACITY: usize = 20;

// ============================================================================
// SlotType —— 槽位类型
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    /// 固定槽位：预注册的关键变量（如 "ones_sum", "carry", "result"），永不自动淘汰
    Fixed,
    /// 动态槽位：运行时产生的临时变量，衰减到 MIN_SIGNAL 以下或 LRU 满时淘汰
    Dynamic,
}

// ============================================================================
// PulseSlot —— 单个变量槽位
// ============================================================================
#[derive(Debug, Clone)]
pub struct PulseSlot {
    /// 变量名（key）
    pub name: String,
    /// 整数值（value）
    pub value: i32,
    /// 槽位类型（Fixed / Dynamic）
    pub slot_type: SlotType,
    /// 信号强度（0-255，Dynamic slot 衰减用）
    pub strength: u8,
    /// 最后访问时间戳（step 编号，LRU 淘汰用）
    pub last_access_step: u64,
    /// 是否已填充
    pub occupied: bool,
    /// 来源区块 ID（设计文档 §7.3：记录该变量由哪个区块写入，
    /// 便于溯源追踪和调试；为空字符串表示未记录来源）
    pub source: String,
}

impl PulseSlot {
    /// 创建新槽位（完整参数，含来源区块 ID）
    /// source：来源区块 ID；为空字符串表示未记录来源（默认情况）
    pub fn new(
        name: impl Into<String>,
        value: i32,
        slot_type: SlotType,
        step: u64,
        source: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            slot_type,
            // Fixed slot 初始 strength=255（永不衰减淘汰）
            // Dynamic slot 初始 strength=255（每次被读/写重置）
            strength: 255,
            last_access_step: step,
            occupied: true,
            source: source.into(),
        }
    }
}

// ============================================================================
// PulseBuffer —— GWS 脉冲缓存主体
// ============================================================================
#[derive(Debug, Clone)]
pub struct PulseBuffer {
    slots: Vec<PulseSlot>,
    /// 总容量
    capacity: usize,
    /// 当前 step 编号（从外部注入）
    current_step: u64,
    /// 衰减率（每个 step 减 2，与 ACTIVATION_DECAY 一致）
    decay_rate: u8,
}

impl PulseBuffer {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: (0..capacity)
                .map(|_| PulseSlot {
                    name: String::new(),
                    value: 0,
                    slot_type: SlotType::Dynamic,
                    strength: 0,
                    last_access_step: 0,
                    occupied: false,
                    source: String::new(),
                })
                .collect(),
            capacity,
            current_step: 0,
            decay_rate: 2,
        }
    }

    // ========================================================================
    // 读写接口
    // ========================================================================

    /// 写入变量（存在则更新，不存在则分配 dynamic slot）
    /// source 默认为空字符串（未记录来源）。需要记录来源时用 set_var_with_source。
    pub fn set_var(&mut self, name: &str, value: i32) {
        // 直接内联实现（与 set_var_with_source 逻辑一致，保留原始代码结构避免增量编译干扰）
        self.set_var_with_source(name, value, "");
    }

    /// 写入变量并记录来源区块 ID（设计文档 §7.3：溯源追踪）
    pub fn set_var_with_source(&mut self, name: &str, value: i32, source: impl Into<String>) {
        let source_str = source.into();
        // 先查已存在的槽位（更新）
        if let Some(existing) = self.slots.iter_mut().find(|s| s.occupied && s.name == name) {
            existing.value = value;
            existing.strength = 255; // 写入重置 strength
            existing.last_access_step = self.current_step;
            existing.source = source_str;
            return;
        }
        // 不存在 → 尝试找空槽
        if let Some(empty) = self.slots.iter_mut().find(|s| !s.occupied) {
            *empty = PulseSlot::new(name, value, SlotType::Dynamic, self.current_step, source_str);
            return;
        }
        // 没空槽 → LRU 淘汰最久未访问的 dynamic slot
        let lru_idx = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.occupied && s.slot_type == SlotType::Dynamic)
            .min_by_key(|(_, s)| s.last_access_step)
            .map(|(i, _)| i);

        if let Some(idx) = lru_idx {
            self.slots[idx] = PulseSlot::new(name, value, SlotType::Dynamic, self.current_step, source_str);
        }
        // 全是 Fixed slot → 静默丢弃（Fixed slot 有 20 个容量上限保护）
    }

    /// 预注册 Fixed slot（启动时声明关键变量）
    /// Fixed slot 的来源一般为系统启动，source 默认为空字符串
    pub fn register_fixed(&mut self, name: &str, initial_value: i32) {
        if self.slots.iter().any(|s| s.occupied && s.name == name) {
            return; // 已存在
        }
        // Fixed slot 不参与容量限制（预注册的关键变量）
        self.slots.push(PulseSlot::new(name, initial_value, SlotType::Fixed, self.current_step, ""));
    }

    /// 读取变量
    pub fn get_var(&mut self, name: &str) -> Option<i32> {
        let slot = self.slots.iter_mut().find(|s| s.occupied && s.name == name)?;
        slot.last_access_step = self.current_step;
        Some(slot.value)
    }

    /// 快照整个变量表（供 MemoryBackend::var_map() 实现）
    pub fn snapshot(&self) -> HashMap<String, i32> {
        let mut map = HashMap::new();
        for slot in self.slots.iter() {
            if slot.occupied {
                map.insert(slot.name.clone(), slot.value);
            }
        }
        map
    }

    // ========================================================================
    // step 管理（Propagator 每 step 调用）
    // ========================================================================

    /// 推进 step（同时衰减 + 淘汰）
    pub fn advance_step(&mut self, new_step: u64) {
        self.current_step = new_step;
        self.decay_tick();
    }

    /// Dynamic slot 衰减 + 淘汰（step 结束时调用）
    /// - Dynamic slot strength 每 step 减 decay_rate
    /// - Fixed slot 永不衰减
    /// - strength < MIN_SIGNAL 的 dynamic slot 被淘汰
    fn decay_tick(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.slot_type == SlotType::Dynamic {
                slot.strength = slot.strength.saturating_sub(self.decay_rate);
                if slot.strength < MIN_SIGNAL {
                    slot.occupied = false;
                }
            }
        }
    }

    pub fn current_step(&self) -> u64 {
        self.current_step
    }

    // ========================================================================
    // 容量 / 调试
    // ========================================================================

    pub fn filled_count(&self) -> usize {
        self.slots.iter().filter(|s| s.occupied).count()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 只读迭代所有槽位（GlobalWorkspace::get_var 用）
    pub fn slots_iter(&self) -> impl Iterator<Item = &PulseSlot> {
        self.slots.iter()
    }

    /// ★ 设计文档 §7.3 exists(name) → bool
    /// 检查变量是否存在（被 set_var 写入过）
    pub fn exists(&self, name: &str) -> bool {
        self.slots.iter().any(|s| s.occupied && s.name == name)
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.current_step = 0;
    }
}

impl Default for PulseBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut buf = PulseBuffer::new();
        buf.set_var("a", 23);
        assert_eq!(buf.get_var("a"), Some(23));
    }

    #[test]
    fn test_update_existing() {
        let mut buf = PulseBuffer::new();
        buf.set_var("x", 10);
        buf.set_var("x", 20);
        assert_eq!(buf.filled_count(), 1);
        assert_eq!(buf.get_var("x"), Some(20));
    }

    #[test]
    fn test_decay_evicts_dynamic() {
        let mut buf = PulseBuffer::new();
        buf.set_var("temp", 100);
        // 推进足够多 step 让 strength < MIN_SIGNAL (5)
        // 初始 255，每 step -2，需要 (255-5)/2 = 125 step
        for step in 1..=130u64 {
            buf.advance_step(step);
        }
        // Dynamic slot 被淘汰
        assert_eq!(buf.get_var("temp"), None);
    }

    #[test]
    fn test_fixed_slot_never_decays() {
        let mut buf = PulseBuffer::new();
        buf.register_fixed("carry", 0);
        for step in 1..=200u64 {
            buf.advance_step(step);
        }
        assert_eq!(buf.get_var("carry"), Some(0));
    }

    #[test]
    fn test_lru_eviction() {
        let mut buf = PulseBuffer::with_capacity(3);
        for i in 0..5 {
            buf.set_var(&format!("v{}", i), i);
        }
        // 容量 3，LRU 淘汰了前 2 个
        assert!(buf.filled_count() <= 3);
        assert_eq!(buf.get_var("v4"), Some(4)); // 最新的在
    }

    #[test]
    fn test_snapshot() {
        let mut buf = PulseBuffer::new();
        buf.set_var("a", 1);
        buf.set_var("b", 2);
        let snap = buf.snapshot();
        assert_eq!(snap.get("a").copied(), Some(1));
        assert_eq!(snap.get("b").copied(), Some(2));
    }
}
