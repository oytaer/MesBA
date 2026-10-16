// ============================================================================
// activator.rs —— 脉冲激活计算（§6.3 伪代码 step 4）
// ============================================================================
// 选型文档要求：激活阈值判断拆为独立模块（与 propagator.rs 解耦）
//
// 核心职责：
//   给定 PulseType + strength → 计算对目标 block 的 activation delta
//   Activate / Result → +strength（激活）
//   Inhibit          → -strength（抑制）
//   Error            → -strength/2（半抑制，携带误差信号）
//   Control          → None（跳过，不影响 activation）
//
// 与 block 解耦：Activator 不持有 Block 引用，只做纯函数计算。
// Propagator 拿到 delta 后再决定怎么应用到 block.state.activation。
// ============================================================================

use mesba_core::PulseType;

/// 根据脉冲类型和强度计算对 activation 的 delta
/// - Activate/Result → +strength（正激活）
/// - Inhibit         → -strength（抑制竞争分支）
/// - Error           → -strength/2（半抑制 + 供上层触发 learning）
/// - Control         → None（跳过，不影响 activation）
///
/// 返回 Some(delta) 表示需要累加 activation_deltas；None 表示跳过此脉冲
pub fn compute_pulse_delta(pulse_type: PulseType, strength: u8) -> Option<i32> {
    match pulse_type {
        PulseType::Activate => Some(strength as i32),
        PulseType::Result => Some(strength as i32),
        PulseType::Inhibit => Some(-(strength as i32)),
        PulseType::Error => Some(-(strength as i32) / 2),
        PulseType::Control => None, // 流程门控脉冲不影响 activation
    }
}

/// 判断 block 是否应该执行 read()（达到阈值）
/// Propagator Pass 2a 调用来决定是否对激活 block 调用 content.read()
pub fn should_read(activation: u8, threshold: u8) -> bool {
    activation >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activate_plus() {
        assert_eq!(compute_pulse_delta(PulseType::Activate, 100), Some(100));
    }

    #[test]
    fn test_result_plus() {
        assert_eq!(compute_pulse_delta(PulseType::Result, 50), Some(50));
    }

    #[test]
    fn test_inhibit_minus() {
        assert_eq!(compute_pulse_delta(PulseType::Inhibit, 180), Some(-180));
    }

    #[test]
    fn test_error_half_negative() {
        assert_eq!(compute_pulse_delta(PulseType::Error, 100), Some(-50));
        assert_eq!(compute_pulse_delta(PulseType::Error, 99), Some(-49)); // 整数除法
    }

    #[test]
    fn test_control_skip() {
        assert_eq!(compute_pulse_delta(PulseType::Control, 255), None);
    }

    #[test]
    fn test_should_read() {
        assert!(should_read(200, 128));
        assert!(!should_read(100, 128));
        assert!(should_read(128, 128)); // 边界值含等于
    }
}
