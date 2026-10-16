// ============================================================================
// attention.rs —— 注意力调节器（环 6 前置：聚焦执行焦点）
// ============================================================================
// 职责：对 PulseBuffer 中激活最强的 Dynamic slot 做增强（strength += 预设值）
// 让后续执行聚焦在最关键的变量上
// ============================================================================

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct AttentionRegulator;

impl AttentionRegulator {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 找出 strength 最高的 Dynamic slot（非 Fixed）并增强
        let best = ctrl
            .workspace
            .pulse_buffer
            .slots_iter()
            .filter(|s| {
                s.occupied
                    && s.slot_type == mesba_memory::pulse_buffer::SlotType::Dynamic
            })
            .max_by_key(|s| s.strength);

        if let Some(slot) = best {
            let best_name = slot.name.clone();
            // 读取并写回让 strength 提升（写操作自动重置 strength=255）
            if let Some(val) = ctrl.get_var(&best_name) {
                ctrl.set_var(&best_name, val); // 自动提升 strength 到 255
            }
        }
    }
}
