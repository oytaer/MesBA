// ============================================================================
// perceptual_buffer.rs —— L0 感知缓冲区（瞬时记忆）
// ============================================================================
// 设计文档 §7.1 L0 PerceptualBuffer：
//   - 容量 7±2 项（经典 Miller's Law，MesBA 固定 16 槽位）
//   - 多模态独立索引（visual / auditory / tactile / proprioceptive 等）
//   - 新感知覆盖旧感知（FIFO 语义）
//   - 每个槽位存原始信号 + 时间戳 + 模态标记
//
// PerceptualBuffer 是 MesBA 三级记忆的最底层：
//   L0 PerceptualBuffer（本文件）  →  L1 GWS（PulseBuffer+GoalStack）  →  L2 MBN（MeshBlockNetwork）
//
// 本模块独立实现，不依赖 mesba_engine 或 mesba_runtime。
// ============================================================================

use std::collections::HashMap;

/// 默认容量（7±2 取中位数 16）
pub const DEFAULT_CAPACITY: usize = 16;

// ============================================================================
// 感知模态枚举
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Modality {
    #[default]
    /// 视觉输入（像素、符号、文字）
    Visual,
    /// 听觉输入（语音、音调、噪音）
    Auditory,
    /// 触觉输入（压力、温度、振动）
    Tactile,
    /// 本体感觉（身体位置、运动姿态）
    Proprioceptive,
    /// 内部状态（饥饿、疼痛、疲劳）
    Interoceptive,
    /// 语言语义（自然语言 token / 符号序列）
    Linguistic,
    /// 抽象符号（数字、运算符、逻辑符号）
    Symbolic,
}

impl Modality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Modality::Visual => "visual",
            Modality::Auditory => "auditory",
            Modality::Tactile => "tactile",
            Modality::Proprioceptive => "proprioceptive",
            Modality::Interoceptive => "interoceptive",
            Modality::Linguistic => "linguistic",
            Modality::Symbolic => "symbolic",
        }
    }

    /// 所有模态列表（用于遍历）
    pub fn all() -> &'static [Modality] {
        &[
            Modality::Visual,
            Modality::Auditory,
            Modality::Tactile,
            Modality::Proprioceptive,
            Modality::Interoceptive,
            Modality::Linguistic,
            Modality::Symbolic,
        ]
    }
}

// ============================================================================
// PerceptualSlot —— 单个感知槽位
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct PerceptualSlot {
    /// 模态标记
    pub modality: Modality,
    /// 原始信号载荷（整数向量）
    pub signal: Vec<i32>,
    /// 信号强度（0-255，饱和整数）
    pub strength: u8,
    /// 时间戳（毫秒，单调递增）
    pub timestamp: u64,
    /// 槽位是否已填充
    pub occupied: bool,
}

// ============================================================================
// PerceptualBuffer —— L0 感知缓冲区主体
// ============================================================================
/// 容量固定的多模态 Ring Buffer。新感知覆盖最旧的槽位（各模态独立计数）。
#[derive(Debug, Clone)]
pub struct PerceptualBuffer {
    /// 所有槽位（长度 = capacity）
    slots: Vec<PerceptualSlot>,
    /// 各模态当前写入位置（环形索引）
    write_index: HashMap<Modality, usize>,
    /// 总容量
    capacity: usize,
    /// 当前总填充数
    filled: usize,
}

impl PerceptualBuffer {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: (0..capacity).map(|_| PerceptualSlot::default()).collect(),
            write_index: HashMap::new(),
            capacity,
            filled: 0,
        }
    }

    /// 写入一个感知信号。新感知覆盖该模态最旧的槽位。
    pub fn write(
        &mut self,
        modality: Modality,
        signal: Vec<i32>,
        strength: u8,
        timestamp: u64,
    ) {
        let idx = self.next_write_index(modality);
        let was_empty = !self.slots[idx].occupied;
        self.slots[idx] = PerceptualSlot {
            modality,
            signal,
            strength,
            timestamp,
            occupied: true,
        };
        if was_empty {
            self.filled = (self.filled + 1).min(self.capacity);
        }
    }

    /// 按模态读取全部已填充槽位（从旧到新）
    pub fn read_by_modality(&self, modality: Modality) -> Vec<&PerceptualSlot> {
        self.slots
            .iter()
            .filter(|s| s.occupied && s.modality == modality)
            .collect()
    }

    /// 读取最近 N 个感知（不限模态，从新到旧）
    pub fn read_recent(&self, n: usize) -> Vec<&PerceptualSlot> {
        let mut occupied: Vec<&PerceptualSlot> =
            self.slots.iter().filter(|s| s.occupied).collect();
        occupied.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
        occupied.truncate(n);
        occupied
    }

    /// 当前填充数
    pub fn filled(&self) -> usize {
        self.filled
    }

    /// 总容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 清空所有槽位（新任务开始时）
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.occupied = false;
        }
        self.write_index.clear();
        self.filled = 0;
    }

    // ---- 私有 ----

    fn next_write_index(&mut self, _modality: Modality) -> usize {
        // 简单全局环形索引（不分模态，所有写入覆盖最旧的槽位）
        // MVP 阶段简化实现，per-modality 独立环形可按需扩展
        let entry = self.write_index.entry(_modality).or_insert(0);
        let current = *entry;
        *entry = (current + 1) % self.capacity;
        current
    }
}

impl Default for PerceptualBuffer {
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
    fn test_write_and_read() {
        let mut buf = PerceptualBuffer::new();
        buf.write(Modality::Symbolic, vec![3, 5], 200, 1000);
        buf.write(Modality::Symbolic, vec![8], 150, 2000);
        assert_eq!(buf.filled(), 2);
        let slots = buf.read_by_modality(Modality::Symbolic);
        assert_eq!(slots.len(), 2);
    }

    #[test]
    fn test_capacity_overflow_evicts_old() {
        let mut buf = PerceptualBuffer::with_capacity(4);
        for i in 0i32..8 {
            buf.write(Modality::Symbolic, vec![i], 200, i as u64 * 1000);
        }
        assert_eq!(buf.filled(), 4); // 不超过 capacity
    }

    #[test]
    fn test_clear_resets() {
        let mut buf = PerceptualBuffer::with_capacity(8);
        for i in 0..5u64 {
            buf.write(Modality::Visual, vec![i as i32], 100, i);
        }
        buf.clear();
        assert_eq!(buf.filled(), 0);
        assert!(buf.read_by_modality(Modality::Visual).is_empty());
    }

    #[test]
    fn test_read_recent_order() {
        let mut buf = PerceptualBuffer::new();
        buf.write(Modality::Symbolic, vec![1], 100, 100);
        buf.write(Modality::Symbolic, vec![2], 100, 300);
        buf.write(Modality::Symbolic, vec![3], 100, 200);
        let recent = buf.read_recent(2);
        assert_eq!(recent.len(), 2);
        // 最新的 timestamp 最大
        assert!(recent[0].timestamp >= recent[1].timestamp);
    }
}