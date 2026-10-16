// ============================================================================
// lut.rs —— 查找表（LUT）模块 —— 256 项预计算
// ============================================================================
// 功能：为 sigmoid / exp / softmax 等复杂函数提供 O(1) 整数查表实现
// 设计文档 §12.3-§12.4 明确要求：
//   "所有复杂函数预先计算256项查找表，存储在只读内存中，推理时O(1)查表访问"
//
// 原则：全系统零浮点 —— 预计算仅在初始化时进行一次，运行时只有整数查表
// ============================================================================

use std::sync::OnceLock;

// ============================================================================
// 预计算函数（运行时初始化，结果存入 static OnceLock）
// ============================================================================

/// 构建 256 项 Sigmoid 查找表
/// 输入 uint8 → 映射到 [-8, +8] → sigmoid → 输出 uint8 (0..=255)
fn build_sigmoid_lut() -> [u8; 256] {
    let mut table = [0u8; 256];
    for i in 0u16..256 {
        let idx = i as usize;
        let x = (i as f32 / 255.0) * 16.0 - 8.0;
        let sig = 1.0 / (1.0 + (-x).exp());
        table[idx] = (sig * 255.0 + 0.5) as u8;
    }
    table
}

/// 构建 256 项 Exp(-x) 查找表
/// 输入 uint8 → 映射到 [0, 4.0] → exp(-x) → 输出 uint8 (0..=255)
fn build_exp_neg_lut() -> [u8; 256] {
    let mut table = [0u8; 256];
    for i in 0u16..256 {
        let idx = i as usize;
        let x = (i as f32 / 255.0) * 4.0;
        let val = (-x).exp();
        table[idx] = (val * 255.0 + 0.5) as u8;
    }
    table
}

// ============================================================================
// 全局 OnceLock —— 首次访问时预计算一次，之后只读
// ============================================================================

/// Sigmoid 256 项查找表（首次调用 sigmoid_lookup 时构建）
static SIGMOID_LUT: OnceLock<[u8; 256]> = OnceLock::new();

/// Exp(-x) 256 项查找表（首次调用 exp_neg_lookup 时构建）
static EXP_NEG_LUT: OnceLock<[u8; 256]> = OnceLock::new();

// ============================================================================
// 查表函数 —— O(1) 访问，零浮点
// ============================================================================

/// Sigmoid 查表函数
/// # 参数
/// - `input`: uint8（0~255）→ 映射到 sigmoid 输入 [-8, +8]
/// # 返回
/// - uint8（0~255）代表 sigmoid(x)（0=0.0, 255=1.0）
#[inline]
pub fn sigmoid_lookup(input: u8) -> u8 {
    let lut = SIGMOID_LUT.get_or_init(build_sigmoid_lut);
    lut[input as usize]
}

/// Exp(-x) 查表函数
/// # 参数
/// - `input`: uint8（0~255）→ 映射到 [0, 4.0]
/// # 返回
/// - uint8（0~255）代表 exp(-x)（255=1.0, 0≈0.018）
#[inline]
pub fn exp_neg_lookup(input: u8) -> u8 {
    let lut = EXP_NEG_LUT.get_or_init(build_exp_neg_lut);
    lut[input as usize]
}

// ============================================================================
// softmax_integer —— 整数版 Softmax
// ============================================================================
/// 设计文档 §12.3 要求：
///   softmax | 整数版本：减最大值→查表exp→累加→除法（整数除法用移位近似）
pub fn softmax_integer(scores: &[u8]) -> Vec<u8> {
    if scores.is_empty() {
        return Vec::new();
    }

    let max_val = *scores.iter().max().unwrap_or(&0);

    let mut exp_values: Vec<u16> = Vec::with_capacity(scores.len());
    for &s in scores {
        let offset = max_val.saturating_sub(s);
        exp_values.push(exp_neg_lookup(offset) as u16);
    }

    let total: u16 = exp_values.iter().sum();

    exp_values
        .into_iter()
        .map(|e| {
            if total == 0 {
                0
            } else {
                ((e as u32 * 255 + total as u32 / 2) / total as u32) as u8
            }
        })
        .collect()
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::integer_ops::sat8_sub;

    #[test]
    fn test_sigmoid_table_shape() {
        let mid = sigmoid_lookup(128);
        assert!((120..=140).contains(&mid), "sigmoid(0) should be ~128, got {}", mid);

        for i in 1u16..256 {
            let ii = i as u8;
            assert!(
                sigmoid_lookup(ii) >= sigmoid_lookup((i - 1) as u8),
                "sigmoid not monotonic at index {}",
                ii
            );
        }

        assert!(sigmoid_lookup(0) <= 10);
        assert!(sigmoid_lookup(255) >= 245);
    }

    #[test]
    fn test_exp_table_shape() {
        assert_eq!(exp_neg_lookup(0), 255);

        for i in 1u16..256 {
            let ii = i as u8;
            assert!(
                exp_neg_lookup(ii) <= exp_neg_lookup((i - 1) as u8),
                "exp not monotonic at index {}",
                ii
            );
        }

        assert!(exp_neg_lookup(255) <= 20);
    }

    #[test]
    fn test_softmax_integer_equal_scores() {
        let scores = vec![100u8, 100, 100, 100];
        let probs = softmax_integer(&scores);
        assert_eq!(probs.len(), 4);
        for p in &probs {
            assert!(*p >= 55 && *p <= 75, "uniform softmax should be ~64, got {}", p);
        }
    }

    #[test]
    fn test_softmax_integer_max_score() {
        let scores = vec![200u8, 50, 50, 50];
        let probs = softmax_integer(&scores);
        assert_eq!(probs.len(), 4);
        assert!(probs[0] > probs[1] * 3);
    }

    #[test]
    fn test_softmax_integer_single() {
        let scores = vec![42u8];
        let probs = softmax_integer(&scores);
        assert_eq!(probs, vec![255]);
    }

    #[test]
    fn test_sat8_sub_works() {
        assert_eq!(sat8_sub(10, 5), 5);
        assert_eq!(sat8_sub(5, 10), 0);
    }

    // ====== softmax_integer 高价值补充测试 ======

    #[test]
    fn test_softmax_integer_probs_sum_to_255() {
        // 软概率之和应接近 255（整数化后）
        let scores = vec![100u8, 150, 200, 50, 80];
        let probs = softmax_integer(&scores);
        let sum: u32 = probs.iter().map(|&p| p as u32).sum();
        // 整数除法可能导致轻微偏差，但应该在 255±4 内
        assert!(
            (251..=259).contains(&sum),
            "softmax probabilities should sum ~255, got {}",
            sum
        );
    }

    #[test]
    fn test_softmax_integer_empty_input() {
        // 空输入 → 空输出（不能 panic）
        let probs = softmax_integer(&[]);
        assert!(probs.is_empty());
    }

    #[test]
    fn test_softmax_integer_two_close_scores() {
        // 两个接近的分数 → 概率接近
        let scores = vec![100u8, 110];
        let probs = softmax_integer(&scores);
        let diff = (probs[0] as i32 - probs[1] as i32).unsigned_abs();
        assert!(diff <= 30, "close scores should yield close probabilities, got diff={}", diff);
    }

    #[test]
    fn test_softmax_integer_extreme_dominance() {
        // 一个极大值 → 概率 255（几乎全部权重）
        let scores = vec![255u8, 0, 0, 0];
        let probs = softmax_integer(&scores);
        assert!(probs[0] >= 240, "dominant score should get ~255, got {}", probs[0]);
    }

    #[test]
    fn test_softmax_integer_order_preserved() {
        // 分数大小顺序必须和概率顺序一致
        let scores = vec![30u8, 80, 200, 150];
        let probs = softmax_integer(&scores);
        assert!(probs[2] > probs[3]); // 200 > 150
        assert!(probs[3] > probs[1]); // 150 > 80
        assert!(probs[1] > probs[0]); // 80 > 30
    }

    #[test]
    fn test_softmax_integer_all_zeros() {
        // 全零输入 → 均匀分布
        let scores = vec![0u8, 0, 0, 0];
        let probs = softmax_integer(&scores);
        for p in &probs {
            assert!(*p >= 55 && *p <= 75, "all-zero softmax should be ~64 each, got {}", p);
        }
    }
}
