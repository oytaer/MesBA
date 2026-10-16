// ============================================================================
// buffer.rs —— 脉冲缓存（Pulse Buffer / 工作记忆）
// ============================================================================
// 功能：MesBA 推理过程中的中间结果暂存——工作记忆核心
// 设计原则：
//   1. 固定槽位数（默认 16 个），不做动态分配
//   2. 每个槽位有名称、整数值、激活强度、时间戳、来源
//   3. 支持衰减（自然遗忘）和 checkpoint（快照/回退）
//   4. 与设计文档中"三级记忆系统"的工作记忆层对应
//
// 对应设计文档：
//   - 网状区块架构 v2 §7.3 脉冲缓存详细设计
//   - 网状区块模型 v2 §10 工作记忆详细规范
// ============================================================================

use serde::{Deserialize, Serialize};

use crate::integer_ops::sat8_sub;
use crate::value::ACTIVATION_DECAY;

// ============================================================================
// BufferSlot —— 单个脉冲缓存槽位
// ============================================================================
// 每个槽位存储一个命名的中间变量，如 "个位结果"、"进位"、"最终结果"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferSlot {
    /// 变量名（如 "operand1", "carry", "digit_result"）
    pub name: String,

    /// 槽位值（int32，支持负数/零/大整数）
    pub value: i32,

    /// 槽位激活强度（uint8 0~255，衰减用）
    pub strength: u8,

    /// 写入时的计步器时间戳（用于衰减淘汰）
    pub timestamp: u64,

    /// 值来源的区块 ID（追踪哪个区块写入了这个中间结果）
    pub source_block: String,
}

impl BufferSlot {
    pub fn new(name: impl Into<String>, value: i32, timestamp: u64, source_block: String) -> Self {
        Self {
            name: name.into(),
            value,
            strength: 200,  // 初始激活强度（中等）
            timestamp,
            source_block,
        }
    }

    /// 衰减一步
    pub fn decay(&mut self) {
        self.strength = sat8_sub(self.strength, ACTIVATION_DECAY);
    }

    /// 槽位是否已衰减到"消亡"状态（可被淘汰）
    pub fn is_dead(&self) -> bool {
        self.strength == 0
    }
}

// ============================================================================
// Buffer —— 脉冲缓存（整个工作记忆）
// ============================================================================
// 设计文档参数（MeshBlock v2 §5.2 + §10）：
//   - 最大槽位数：16（Miller 7±2 法则翻倍，支持复杂推理）
//   - 槽位类型：固定槽 + 动态槽
//   - 衰减模式：每步 strength -= ACTIVATION_DECAY
//   - 淘汰策略：强度最低 + 最久未用（类 LRU + 衰减）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Buffer {
    /// 槽位数组（固定容量）
    slots: Vec<Option<BufferSlot>>,

    /// 最大容量
    capacity: usize,

    /// 全局计步器时间戳（每次推理递增）
    step_counter: u64,
}

impl Buffer {
    // -----------------------------------------------------------------------
    // 构造函数
    // -----------------------------------------------------------------------

    /// 创建默认 Buffer（16 槽位）
    pub fn new_default() -> Self {
        Self::with_capacity(16)
    }

    /// 创建指定容量的 Buffer
    pub fn with_capacity(capacity: usize) -> Self {
        let slots = vec![None; capacity];
        Self {
            slots,
            capacity,
            step_counter: 0,
        }
    }

    // -----------------------------------------------------------------------
    // 基本操作
    // -----------------------------------------------------------------------

    /// 写入一个命名槽位（同名覆盖 / 新名分配）
    /// 返回：(slot_index, 是否新建)
    pub fn write(
        &mut self,
        name: impl Into<String>,
        value: i32,
        source_block: impl Into<String>,
    ) -> (usize, bool) {
        let name = name.into();
        let source = source_block.into();
        self.step_counter += 1;

        // 先查找是否已存在同名槽位 → 覆盖
        for (idx, slot_opt) in self.slots.iter_mut().enumerate() {
            if let Some(slot) = slot_opt
                && slot.name == name {
                    slot.value = value;
                    slot.strength = 200;  // 重写恢复激活
                    slot.timestamp = self.step_counter;
                    slot.source_block = source;
                    return (idx, false);
                }
        }

        // 不存在 → 找空槽位
        for (idx, slot_opt) in self.slots.iter_mut().enumerate() {
            if slot_opt.is_none() {
                *slot_opt = Some(BufferSlot::new(name, value, self.step_counter, source));
                return (idx, true);
            }
        }

        // 槽位全满 → 淘汰最弱槽位后写入
        let weakest_idx = self.find_weakest_slot();
        if let Some(idx) = weakest_idx {
            self.slots[idx] = Some(BufferSlot::new(name, value, self.step_counter, source));
            return (idx, true);
        }

        // 不应到达此处（find_weakest_slot 在满槽时一定返回 Some）
        unreachable!("Buffer slots full but find_weakest_slot returned None");
    }

    /// 读取命名槽位的 int32 值
    pub fn read(&self, name: &str) -> Option<i32> {
        self.slots
            .iter()
            .find_map(|slot_opt| slot_opt.as_ref().and_then(|s| {
                if s.name == name { Some(s.value) } else { None }
            }))
    }

    /// 检查槽位是否存在
    pub fn exists(&self, name: &str) -> bool {
        self.slots.iter().any(|slot_opt| {
            slot_opt.as_ref().map(|s| s.name == name).unwrap_or(false)
        })
    }

    /// 获取槽位完整内容（含 strength/source/timestamp）
    pub fn get_slot(&self, name: &str) -> Option<&BufferSlot> {
        self.slots
            .iter()
            .find_map(|slot_opt| slot_opt.as_ref().filter(|&s| s.name == name))
    }

    // -----------------------------------------------------------------------
    // 衰减 / 淘汰
    // -----------------------------------------------------------------------

    /// 对所有槽位执行一次衰减（每轮脉冲传播后调用）
    /// 衰减后自动淘汰消亡槽位
    pub fn decay_all(&mut self) {
        self.step_counter += 1;
        for slot_opt in self.slots.iter_mut() {
            if let Some(slot) = slot_opt {
                slot.decay();
                if slot.is_dead() {
                    *slot_opt = None;
                }
            }
        }
    }

    /// 找出最弱槽位（strength 最低；若并列则取 timestamp 最旧）
    /// 用于槽位满时淘汰
    fn find_weakest_slot(&self) -> Option<usize> {
        let mut best: Option<(usize, u8, u64)> = None;
        for (idx, slot_opt) in self.slots.iter().enumerate() {
            if let Some(slot) = slot_opt {
                match best {
                    None => best = Some((idx, slot.strength, slot.timestamp)),
                    Some((_best_idx, best_str, best_ts)) => {
                        if slot.strength < best_str
                            || (slot.strength == best_str && slot.timestamp < best_ts)
                        {
                            best = Some((idx, slot.strength, slot.timestamp));
                        }
                    }
                }
            }
        }
        best.map(|(idx, _, _)| idx)
    }

    // -----------------------------------------------------------------------
    // Clear / Reset
    // -----------------------------------------------------------------------

    /// 清空所有槽位（推理结束时调用）
    pub fn clear(&mut self) {
        for slot_opt in self.slots.iter_mut() {
            *slot_opt = None;
        }
    }

    /// 重置 Buffer（清空 + 重置时间戳）
    pub fn reset(&mut self) {
        self.clear();
        self.step_counter = 0;
    }

    /// 创建快照（用于 checkpoint/rollback 机制）
    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            slots: self.slots.clone(),
            step_counter: self.step_counter,
        }
    }

    /// 从快照恢复（回退操作）
    pub fn restore(&mut self, snapshot: &BufferSnapshot) {
        self.slots = snapshot.slots.clone();
        self.step_counter = snapshot.step_counter;
    }

    // -----------------------------------------------------------------------
    // 状态查询
    // -----------------------------------------------------------------------

    /// 当前已使用槽位数
    pub fn used_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// 剩余空槽位数
    pub fn free_slots(&self) -> usize {
        self.capacity - self.used_slots()
    }

    /// 总容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前步骤计数器值
    pub fn current_step(&self) -> u64 {
        self.step_counter
    }
}

// ============================================================================
// BufferSnapshot —— 脉冲缓存快照（用于 checkpoint / rollback）
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferSnapshot {
    /// 快照时的槽位数组（深拷贝）
    pub slots: Vec<Option<BufferSlot>>,
    /// 快照时的步骤计数器
    pub step_counter: u64,
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_write_read() {
        let mut buf = Buffer::new_default();
        let (idx, is_new) = buf.write("operand1", 99, "Perceive.DigitRecognizer");
        assert!(is_new);
        assert_eq!(buf.used_slots(), 1);
        assert_eq!(buf.read("operand1"), Some(99));

        // 同名覆盖
        let (idx2, is_new2) = buf.write("operand1", 42, "Op.AlignDigits");
        assert!(!is_new2);  // 不是新建
        assert_eq!(idx, idx2);  // 同一个槽位
        assert_eq!(buf.read("operand1"), Some(42));
        assert_eq!(buf.used_slots(), 1);
    }

    #[test]
    fn test_slot_capacity_and_eviction() {
        let mut buf = Buffer::with_capacity(4);
        // 填满 4 个槽位
        buf.write("a", 1, "src");
        buf.write("b", 2, "src");
        buf.write("c", 3, "src");
        buf.write("d", 4, "src");
        assert_eq!(buf.used_slots(), 4);

        // 衰减：让前三个槽位强度降低
        // 衰减一次后 strength = 200 - 2 = 198
        for _ in 0..99 {
            buf.decay_all();  // 每个槽位 strength 减 2，直到归零
        }
        // 现在前 3 个槽位已经衰减到 ~2，新写入会覆盖最弱的
        buf.write("e", 5, "src");
        assert_eq!(buf.used_slots(), 4);
        assert_eq!(buf.read("e"), Some(5));
    }

    #[test]
    fn test_decay_and_life_cycle() {
        let mut buf = Buffer::new_default();
        buf.write("carry", 1, "Judge.CarryNeeded");
        assert!(buf.exists("carry"));

        // 衰减到消亡
        for _ in 0..120 {
            buf.decay_all();
        }
        assert!(!buf.exists("carry"));  // 已被淘汰
        assert_eq!(buf.used_slots(), 0);
    }

    #[test]
    fn test_snapshot_rollback() {
        let mut buf = Buffer::new_default();
        buf.write("operand1", 99, "src");
        buf.write("operand2", 47, "src");

        let snap = buf.snapshot();

        // 推理中间状态
        buf.write("result", 52, "src");
        buf.write("carry", 0, "src");
        assert_eq!(buf.used_slots(), 4);
        assert_eq!(buf.read("result"), Some(52));

        // 回退！
        buf.restore(&snap);
        assert_eq!(buf.used_slots(), 2);
        assert!(buf.read("result").is_none());
        assert_eq!(buf.read("operand1"), Some(99));
        assert_eq!(buf.read("operand2"), Some(47));
    }

    #[test]
    fn test_clear_and_reset() {
        let mut buf = Buffer::new_default();
        buf.write("a", 1, "src");
        buf.write("b", 2, "src");
        buf.write("c", 3, "src");

        buf.clear();
        assert_eq!(buf.used_slots(), 0);
        // clear 后 step_counter 保留
        assert!(buf.current_step() > 0);

        buf.reset();
        assert_eq!(buf.current_step(), 0);
    }

    #[test]
    fn test_slot_info() {
        let mut buf = Buffer::new_default();
        buf.write("carry", 1, "Judge.CarryNeeded");

        let slot = buf.get_slot("carry").unwrap();
        assert_eq!(slot.name, "carry");
        assert_eq!(slot.value, 1);
        assert_eq!(slot.source_block, "Judge.CarryNeeded");
        assert!(slot.strength > 0);
    }
}
