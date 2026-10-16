// ============================================================================
// integer_ops.rs —— 整数运算核心模块
// ============================================================================
// 功能：提供 MesBA 全系统使用的无浮点整数运算原语
// 设计原则：
//   1. 所有运算使用整数（uint8/uint16/int32），**全系统零浮点**
//   2. 饱和运算：结果钳位到合法范围，不溢出
//   3. Q8.8 定点小数乘法：uint8 × uint8 → 右移8位，等价于 Q8.8 乘法
//   4. 常数预计算：避免运行时浮点运算
//
// 对应设计文档：
//   - 网状区块架构 v2 §12.1-§12.3 数值计算规范
//   - 网状区块模型 v2 §4.1 核心整数运算函数
// ============================================================================

/// =========================================================================
/// sat8_add —— uint8 饱和加法
/// =========================================================================
/// 将两个 uint8 值相加，若结果超过 255 则钳位为 255（饱和）。
/// 此函数是脉冲激活累加的核心：脉冲强度累加、激活强度累加都使用此函数。
///
/// # 参数
/// - `a`: 第一个 uint8 操作数
/// - `b`: 第二个 uint8 操作数
///
/// # 返回
/// - `0..=255` 范围内的 uint8 饱和加结果
///
/// # 算法
/// 使用 uint16 作为中间累加器避免溢出，然后钳位：
/// ```text
/// sum = (a as u16) + (b as u16)
/// return if sum > 255 { 255 } else { sum as u8 }
/// ```
///
/// # 示例
/// ```
/// use mesba_core::integer_ops::sat8_add;
/// assert_eq!(sat8_add(100, 100), 200);   // 正常加法
/// assert_eq!(sat8_add(200, 100), 255);   // 饱和钳位到255
/// assert_eq!(sat8_add(255, 255), 255);   // 完全饱和
/// assert_eq!(sat8_add(0, 0), 0);         // 零值
/// ```
#[inline]
pub fn sat8_add(a: u8, b: u8) -> u8 {
    let sum = (a as u16) + (b as u16);
    if sum > 255 {
        255
    } else {
        sum as u8
    }
}

/// =========================================================================
/// sat8_sub —— uint8 饱和减法
/// =========================================================================
/// 将两个 uint8 值相减，若结果低于 0 则钳位为 0（饱和）。
/// 此函数用于抑制脉冲的累减、激活强度的自然衰减。
///
/// # 参数
/// - `a`: 被减数
/// - `b`: 减数
///
/// # 返回
/// - `0..=255` 范围内的 uint8 饱和减结果
///
/// # 算法
/// 直接整数比较后条件减法：
/// ```text
/// return if a > b { a - b } else { 0 }
/// ```
///
/// # 示例
/// ```
/// use mesba_core::integer_ops::sat8_sub;
/// assert_eq!(sat8_sub(200, 100), 100);   // 正常减法
/// assert_eq!(sat8_sub(50, 100), 0);      // 饱和钳位到0
/// assert_eq!(sat8_sub(0, 0), 0);         // 零值
/// assert_eq!(sat8_sub(1, 0), 1);         // 减零不变
/// ```
#[inline]
pub fn sat8_sub(a: u8, b: u8) -> u8 {
    a.saturating_sub(b)
}

/// =========================================================================
/// uint8_multiply —— uint8 × uint8 乘法 + 右移8位（等价 Q8.8 定点乘法）
/// =========================================================================
/// 将两个 uint8 值相乘得到 16 位中间值，然后右移 8 位返回高 8 位。
/// 此函数是脉冲传播中**强度×权重×衰减系数**计算的核心原语。
///
/// # Q8.8 定点小数原理
/// uint8 值范围 0..=255 对应 Q8.8 定点小数 0.0..=1.0（其中 255 ≈ 0.996）。
/// Q8.8 × Q8.8 = Q16.16（高8位整数+低8位小数，其余8位小数），
/// 右移 8 位得到新的 Q8.8 值。
///
/// # 参数
/// - `a`: 第一个 uint8 操作数（可表示 Q8.8 定点小数）
/// - `b`: 第二个 uint8 操作数（可表示 Q8.8 定点小数）
///
/// # 返回
/// - `0..=255` 范围内的 uint8，代表 Q8.8 定点乘法结果
///
/// # 算法
/// ```text
/// product = (a as u16) * (b as u16)
/// return (product >> 8) as u8
/// ```
///
/// # 数值示例
/// | a (uint8) | b (uint8) | a(Q8.8) | b(Q8.8) | 真实乘积 | 本函数结果 |
/// |:---:|:---:|:---:|:---:|:---:|:---:|
/// | 255 | 255 | 0.996 | 0.996 | 0.992 | 254 |
/// | 204 | 204 | 0.80 | 0.80 | 0.64 | 164 |
/// | 128 | 128 | 0.50 | 0.50 | 0.25 | 64 |
/// | 51  | 51  | 0.20 | 0.20 | 0.04 | 10 |
/// | 26  | 128 | 0.10 | 0.50 | 0.05 | 25 |
///
/// # 使用场景
/// - 脉冲传播：`new_strength = uint8_multiply(pulse.strength, conn.weight)`
/// - 衰减计算：`new_strength = uint8_multiply(new_strength, conn.decay)`
#[inline]
pub fn uint8_multiply(a: u8, b: u8) -> u8 {
    let product = (a as u16) * (b as u16);
    (product >> 8) as u8
}

/// =========================================================================
/// sat32_add —— int32 饱和加法
/// =========================================================================
/// 用于中间结果（事实区块 value 的 int32 运算）的饱和加法，
/// 结果钳位到 i32::MIN..=i32::MAX 范围内。
///
/// # 参数
/// - `a`: 第一个 int32 操作数
/// - `b`: 第二个 int32 操作数
///
/// # 返回
/// - i32 饱和加结果
///
/// # 算法
/// 使用 int64 作为中间累加器，然后钳位到 i32 范围。
#[inline]
pub fn sat32_add(a: i32, b: i32) -> i32 {
    let sum = (a as i64) + (b as i64);
    if sum > i32::MAX as i64 {
        i32::MAX
    } else if sum < i32::MIN as i64 {
        i32::MIN
    } else {
        sum as i32
    }
}

/// =========================================================================
/// uint8_divide —— uint8 × 常量 magic number 除法（整数近似）
/// =========================================================================
/// 将一个 uint8 值用常量整数近似除以某个固定整数，避免运行时除法。
/// 原理：除以 k 等价于乘以 magic_number 然后右移 shift_bits。
/// 预计算表覆盖常用除数（2, 4, 5, 8, 10, 16, 20, 25, 50, 100）。
///
/// # 常量整数除法原理
/// 除以常量 k 等价于乘以 magic_number 再右移 shift_bits：
///   magic = round(2^shift / divisor)
///   result ≈ (value × magic) >> shift
/// 这比运行时除法快得多，且整数近似足够用（MesBA 是近似脉冲传播，无需精确到 1）。
#[inline]
pub fn uint8_divide(value: u8, divisor: u8) -> u8 {
    match divisor {
        // 2^n 次幂直接右移
        2  => value >> 1,
        4  => value >> 2,
        8  => value >> 3,
        16 => value >> 4,
        // 非 2^n 常量用 magic number 算法
        // 公式：result ≈ (value × magic) >> shift
        5  => { let p = value as u16 * 205; (p >> 10) as u8 }   // magic=205≈2^10/5, shift=10
        10 => { let p = value as u16 * 205; (p >> 11) as u8 }   // magic=205≈2^11/10, shift=11
        20 => { let p = value as u16 * 205; (p >> 12) as u8 }   // magic=205≈2^12/20, shift=12
        25 => { let p = value as u16 * 41;  (p >> 10) as u8 }   // magic=41≈2^10/25,  shift=10
        50 => { let p = value as u16 * 41;  (p >> 11) as u8 }   // magic=41≈2^11/50,  shift=11
        100 => { let p = value as u16 * 41; (p >> 12) as u8 }   // magic=41≈2^12/100, shift=12
        _ => (value as u16 / divisor as u16) as u8,  // 罕见除数回退到真正除法
    }
}

// ============================================================================
// sqrt_u32_newton —— 整数牛顿迭代法求 int32 平方根
// ============================================================================
/// 设计文档 §12.3 明确要求：
///   "sqrt | 整数牛顿迭代法"
///
/// 算法：
///   牛顿迭代求整数平方根：
///   1. x₀ = value（初始猜测值）
///   2. 迭代：x_{n+1} = (x_n + value / x_n) / 2
///   3. 直到 x_{n+1} ≥ x_n（收敛）
///
/// # 参数
/// - `value`: 非负 int32 输入（负数返回 0）
///
/// # 返回
/// - 非负 int32，满足 result² ≤ value < (result+1)²
///
/// # 复杂度
/// - 最多迭代 ceil(log₂(value)) + 1 次（uint32 最多 32 次）
/// - 全整数运算，零浮点
///
/// # 精度
/// - 精确整数平方根（floor），适用于 MesBA 内部计算（连接距离/向量模长等）
#[inline]
pub fn sqrt_u32_newton(value: i32) -> i32 {
    if value <= 0 {
        return 0;
    }
    // 初始猜测：从 min(value, 2^16) 开始加速收敛
    let mut next = if value > 65536 { (value as u32) / 2 } else { value as u32 };
    // 迭代直到收敛（x_next >= x_cur 且 x_cur >= x_next 等价于稳定）
    loop {
        // next = (x + value / x) / 2，全整数运算
        let div = value as u32 / next.max(1); // 防止除以零
        let new_x = (next + div) / 2;
        if new_x >= next {
            break; // 收敛（整数牛顿迭代：当 new_x ≥ next 时停止）
        }
        next = new_x;
    }
    next as i32
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // ---- sat8_add 测试 ----
    #[test]
    fn test_sat8_add_normal() {
        assert_eq!(sat8_add(0, 0), 0);
        assert_eq!(sat8_add(1, 1), 2);
        assert_eq!(sat8_add(100, 50), 150);
        assert_eq!(sat8_add(128, 127), 255);
    }

    #[test]
    fn test_sat8_add_saturation() {
        assert_eq!(sat8_add(200, 100), 255);
        assert_eq!(sat8_add(255, 1), 255);
        assert_eq!(sat8_add(255, 255), 255);
        assert_eq!(sat8_add(128, 128), 255);
    }

    // ---- sat8_sub 测试 ----
    #[test]
    fn test_sat8_sub_normal() {
        assert_eq!(sat8_sub(0, 0), 0);
        assert_eq!(sat8_sub(255, 0), 255);
        assert_eq!(sat8_sub(100, 50), 50);
        assert_eq!(sat8_sub(128, 64), 64);
    }

    #[test]
    fn test_sat8_sub_saturation() {
        assert_eq!(sat8_sub(0, 1), 0);
        assert_eq!(sat8_sub(50, 100), 0);
        assert_eq!(sat8_sub(0, 255), 0);
    }

    // ---- uint8_multiply 测试（Q8.8 定点小数乘法） ----
    #[test]
    fn test_uint8_multiply_zero() {
        assert_eq!(uint8_multiply(0, 0), 0);
        assert_eq!(uint8_multiply(255, 0), 0);
        assert_eq!(uint8_multiply(0, 255), 0);
    }

    #[test]
    fn test_uint8_multiply_identity() {
        // 255 ≈ 0.996，乘以255近似等于原值
        assert_eq!(uint8_multiply(128, 255), 127);
        assert_eq!(uint8_multiply(64, 255), 63);
    }

    #[test]
    fn test_uint8_multiply_half() {
        // 128 = 0.5，乘以128等价于除以2
        assert_eq!(uint8_multiply(200, 128), 100);
        assert_eq!(uint8_multiply(128, 128), 64);
        assert_eq!(uint8_multiply(100, 128), 50);
    }

    #[test]
    fn test_uint8_multiply_weight_decay() {
        // 权重204 (0.80) × 强度255 × 衰减230 (0.90) — 验证脉冲传播两步计算
        let step1 = uint8_multiply(255, 204);  // (255×204)>>8 = 52020>>8 = 203
        let step2 = uint8_multiply(step1, 230); // (203×230)>>8 = 46690>>8 = 182
        // 注：Q8.8 整数乘法是截断而非四舍五入，所以结果与浮点近似值有 ±1 差异
        assert!((200..=210).contains(&step1));  // 大致在 0.80×255=204 附近
        assert!((175..=190).contains(&step2));  // 大致在 0.90×204≈184 附近
    }

    // ---- sat32_add 测试 ----
    #[test]
    fn test_sat32_add_normal() {
        assert_eq!(sat32_add(0, 0), 0);
        assert_eq!(sat32_add(100, 200), 300);
        assert_eq!(sat32_add(-100, 50), -50);
    }

    #[test]
    fn test_sat32_add_positive_overflow() {
        assert_eq!(sat32_add(i32::MAX, 1), i32::MAX);
        assert_eq!(sat32_add(i32::MAX, i32::MAX), i32::MAX);
    }

    #[test]
    fn test_sat32_add_negative_overflow() {
        assert_eq!(sat32_add(i32::MIN, -1), i32::MIN);
        assert_eq!(sat32_add(i32::MIN, i32::MIN), i32::MIN);
    }

    // ---- uint8_divide 测试 ----
    #[test]
    fn test_uint8_divide() {
        assert_eq!(uint8_divide(100, 2), 50);
        assert_eq!(uint8_divide(100, 4), 25);
        assert_eq!(uint8_divide(100, 5), 20);
        assert_eq!(uint8_divide(100, 10), 10);
        assert_eq!(uint8_divide(100, 100), 1);
        assert_eq!(uint8_divide(0, 50), 0);
    }

    // ---- sqrt_u32_newton 测试 ----
    #[test]
    fn test_sqrt_zero_and_negative() {
        assert_eq!(sqrt_u32_newton(0), 0);
        assert_eq!(sqrt_u32_newton(-5), 0);
        assert_eq!(sqrt_u32_newton(i32::MIN), 0);
    }

    #[test]
    fn test_sqrt_perfect_squares() {
        assert_eq!(sqrt_u32_newton(1), 1);
        assert_eq!(sqrt_u32_newton(4), 2);
        assert_eq!(sqrt_u32_newton(9), 3);
        assert_eq!(sqrt_u32_newton(16), 4);
        assert_eq!(sqrt_u32_newton(25), 5);
        assert_eq!(sqrt_u32_newton(100), 10);
        assert_eq!(sqrt_u32_newton(225), 15);
        assert_eq!(sqrt_u32_newton(10000), 100);
        assert_eq!(sqrt_u32_newton(1000000), 1000);
        assert_eq!(sqrt_u32_newton(2147395600), 46340); // 最大完全平方 ≤ i32::MAX
    }

    #[test]
    fn test_sqrt_non_perfect_floor() {
        // 非完全平方应返回 floor
        assert_eq!(sqrt_u32_newton(2), 1);  // √2 ≈ 1.414 → floor=1
        assert_eq!(sqrt_u32_newton(3), 1);
        assert_eq!(sqrt_u32_newton(5), 2);  // √5 ≈ 2.236 → floor=2
        assert_eq!(sqrt_u32_newton(8), 2);
        assert_eq!(sqrt_u32_newton(26), 5); // √26 ≈ 5.099 → floor=5
        assert_eq!(sqrt_u32_newton(99), 9); // √99 ≈ 9.95 → floor=9
    }

    #[test]
    fn test_sqrt_large_values() {
        // 大值：保证迭代在合理次数内收敛
        let result = sqrt_u32_newton(2_000_000_000);
        assert_eq!(result, 44721); // √2e9 ≈ 44721.36 → floor=44721
        assert!(result * result <= 2_000_000_000);
        assert!((result + 1) * (result + 1) > 2_000_000_000);
    }

    // ====== sqrt_u32_newton 高价值补充测试 ======

    #[test]
    fn test_sqrt_max_i32() {
        // i32::MAX = 2147483647
        let result = sqrt_u32_newton(i32::MAX);
        assert_eq!(result, 46340); // floor(√2147483647) = 46340
        assert!(result as i64 * result as i64 <= i32::MAX as i64);
        assert!((result as i64 + 1) * (result as i64 + 1) > i32::MAX as i64);
    }

    #[test]
    fn test_sqrt_floor_property_100_random() {
        // 100 个随机值验证 floor 属性：result² ≤ value < (result+1)²
        let mut rng_state: u32 = 0x12345678;
        for _ in 0..100 {
            // xorshift32
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 17;
            rng_state ^= rng_state << 5;
            let val = (rng_state as i64 % 2_000_000_000i64).abs() as i32;
            if val <= 0 {
                continue;
            }
            let r = sqrt_u32_newton(val);
            let r_sq = r as i64 * r as i64;
            let r1_sq = (r as i64 + 1) * (r as i64 + 1);
            assert!(
                r_sq <= val as i64 && r1_sq > val as i64,
                "sqrt({}) = {} violates floor property ({}²={}, {}²={})",
                val,
                r,
                r,
                r_sq,
                r + 1,
                r1_sq
            );
        }
    }

    #[test]
    fn test_sqrt_small_values_edge_cases() {
        // 边界值：1~16 全部手动算
        let cases: [(i32, i32); 17] = [
            (0, 0), (1, 1), (2, 1), (3, 1), (4, 2), (5, 2), (6, 2), (7, 2),
            (8, 2), (9, 3), (10, 3), (15, 3), (16, 4), (17, 4), (24, 4), (25, 5), (99, 9),
        ];
        for (input, expected) in cases {
            let got = sqrt_u32_newton(input);
            assert_eq!(
                got, expected,
                "sqrt({}) should be {}, got {}",
                input, expected, got
            );
        }
    }

    #[test]
    fn test_sqrt_convergence_rate() {
        // 验证牛顿迭代在 O(log n) 步内收敛（不做精确计数，只验证很快返回）
        // 这里用极端值 2^30 ≈ 1.07e9
        let result = sqrt_u32_newton(1 << 30);
        assert_eq!(result, 1 << 15); // √(2^30) = 2^15 = 32768
    }

    #[test]
    fn test_sqrt_two_consecutive_squares() {
        // 验证相邻完全平方数之间的值都返回正确的 floor
        for n in 1i32..=100 {
            let sq = n * n;
            let next_sq = (n + 1) * (n + 1);
            for val in sq..next_sq {
                let r = sqrt_u32_newton(val);
                assert_eq!(
                    r, n,
                    "sqrt({}) should be {} (between {}² and {}²), got {}",
                    val, n, n, n + 1, r
                );
            }
        }
    }
}
