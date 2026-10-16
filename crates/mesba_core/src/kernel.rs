// ============================================================================
// kernel.rs —— Layer 1 热点 Kernel（SIMD 加速）
// ============================================================================
// 编程语言选型 §4.1 明确要求：
//   "脉冲传播最内层循环是整个系统的热点，每次推理会被调用数亿次"
//   "这一层的代码量极小（预估不超过 2000 行），但占推理时间的 90% 以上"
//
// 本模块提供：
//   1. sat8_add_batch —— 批量 uint8 饱和加法（运行时自动选择 AVX-2 / 标量）
//   2. sat8_add_batch_inplace —— 同上的 in-place 版本
//   3. uint8_multiply_batch —— 批量 Q8.8 定点乘法（同上）
//   4. saturate_u8 —— 标量饱和钳位辅助函数
//
// 平台适配策略：运行时检测 CPU 特性 → dispatch 到最优实现
//   ┌──────────┬──────────────────────────┬──────────────────────────┐
//   │ 平台     │ 检测结果                  │ 启用实现                  │
//   ├──────────┼──────────────────────────┼──────────────────────────┤
//   │ x86-64   │ avx2=true               │ AVX-2 intrinsic           │
//   │ x86-64   │ avx2=false               │ 标量回退（4 字节分组）     │
//   │ ARM/其他 │ 始终 false               │ 标量回退                  │
//   └──────────┴──────────────────────────┴──────────────────────────┘
// ============================================================================

// ============================================================================
// x86-64 SIMD 版本（运行时检测 + 条件编译）
// ============================================================================
// SIMD 内部函数带 #[target_feature(enable = "avx2")] 注解 + 显式 unsafe 块，
// edition 2024 要求即使在 unsafe fn 内部调用 intrinsic 也需要 unsafe {} 包。
// ============================================================================

// ----------------------------------------------------------------------------
// AVX-2 内部实现（仅被 dispatcher 调用）
// ----------------------------------------------------------------------------
#[cfg(target_arch = "x86_64")]
mod x86_avx2 {
    use std::arch::x86_64::*;

    /// AVX-2 批量 sat8 饱和加法：一次处理 32 个 uint8
    /// 使用 vpaddusb 指令（AVX-2 内建的 u8 饱和加法）
    #[target_feature(enable = "avx2")]
    #[inline]
    pub unsafe fn sat8_add_batch_avx2(
        dst: &mut [u8],
        a: &[u8],
        b: &[u8],
    ) {
        let n = dst.len();
        let mut i = 0;
        while i + 32 <= n {
            unsafe {
                let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
                let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
                let result = _mm256_adds_epu8(va, vb);
                _mm256_storeu_si256(dst.as_mut_ptr().add(i) as *mut __m256i, result);
            }
            i += 32;
        }
        // 尾部 fallback
        let dst_tail = &mut dst[i..];
        let a_tail = &a[i..];
        let b_tail = &b[i..];
        for j in 0..dst_tail.len() {
            let sum = (a_tail[j] as u16) + (b_tail[j] as u16);
            dst_tail[j] = if sum > 255 { 255 } else { sum as u8 };
        }
    }

    /// AVX-2 批量 sat8 饱和加法（in-place 版本）
    #[target_feature(enable = "avx2")]
    #[inline]
    pub unsafe fn sat8_add_batch_inplace_avx2(dst: &mut [u8], src: &[u8]) {
        let n = dst.len();
        let mut i = 0;
        while i + 32 <= n {
            unsafe {
                let vd = _mm256_loadu_si256(dst.as_ptr().add(i) as *const __m256i);
                let vs = _mm256_loadu_si256(src.as_ptr().add(i) as *const __m256i);
                let result = _mm256_adds_epu8(vd, vs);
                _mm256_storeu_si256(dst.as_mut_ptr().add(i) as *mut __m256i, result);
            }
            i += 32;
        }
        // 尾部 fallback
        let dst_tail = &mut dst[i..];
        let src_tail = &src[i..];
        for j in 0..dst_tail.len() {
            let sum = (dst_tail[j] as u16) + (src_tail[j] as u16);
            dst_tail[j] = if sum > 255 { 255 } else { sum as u8 };
        }
    }

    /// AVX-2 批量 Q8.8 定点乘法：一次处理 16 个 uint8
    /// 策略：8 个 u8 → 零扩展 16-bit → u16 × u16 → 右移 8 → pack 回 u8
    /// 使用 __m128i 操作（兼容 SSE2 fallback），每次处理 16 字节
    #[target_feature(enable = "avx2")]
    #[inline]
    pub unsafe fn uint8_multiply_batch_avx2(
        dst: &mut [u8],
        a: &[u8],
        b: &[u8],
    ) {
        let n = dst.len();
        let mut i = 0;
        while i + 16 <= n {
            unsafe {
                // 加载 16 字节到 __m128i
                let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
                let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                let zero = _mm_setzero_si128();

                // 低 8 字节（lane 0）：u8→u16 → 乘 → 右移 8 → pack 回 u8
                let lo_a_wide = _mm_cvtepu8_epi16(_mm_unpacklo_epi8(va, zero));
                let lo_b_wide = _mm_cvtepu8_epi16(_mm_unpacklo_epi8(vb, zero));
                let lo_mul = _mm_mullo_epi16(lo_a_wide, lo_b_wide);
                let lo_shifted = _mm_srli_epi16(lo_mul, 8);
                let lo_packed = _mm_packus_epi16(lo_shifted, lo_shifted);
                _mm_storel_epi64(dst.as_mut_ptr().add(i) as *mut __m128i, lo_packed);

                // 高 8 字节（lane 1）：同样处理
                let hi_a_wide = _mm_cvtepu8_epi16(_mm_unpackhi_epi8(va, zero));
                let hi_b_wide = _mm_cvtepu8_epi16(_mm_unpackhi_epi8(vb, zero));
                let hi_mul = _mm_mullo_epi16(hi_a_wide, hi_b_wide);
                let hi_shifted = _mm_srli_epi16(hi_mul, 8);
                let hi_packed = _mm_packus_epi16(hi_shifted, hi_shifted);
                _mm_storel_epi64(
                    dst.as_mut_ptr().add(i + 8) as *mut __m128i,
                    hi_packed,
                );
            }
            i += 16;
        }
        // 尾部 fallback
        let dst_tail = &mut dst[i..];
        let a_tail = &a[i..];
        let b_tail = &b[i..];
        for j in 0..dst_tail.len() {
            dst_tail[j] = ((a_tail[j] as u16 * b_tail[j] as u16) >> 8) as u8;
        }
    }
}

// ----------------------------------------------------------------------------
// 运行时 CPU 特性检测
// ----------------------------------------------------------------------------

/// CPU 是否支持 AVX-2（运行时检测，一次调用永久返回相同值）
/// x86-64 平台用 is_x86_feature_detected! 宏检测；
/// 其他架构直接返回 false，走标量回退路径。
#[inline]
fn cpu_supports_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ----------------------------------------------------------------------------
// 公共 API：运行时自动 dispatch
// ----------------------------------------------------------------------------
// 对外暴露 3 个核心 batch 函数 + 1 个辅助函数，签名稳定。
// 内部先检测 CPU 特性，然后 dispatch 到最优实现。
// ----------------------------------------------------------------------------

/// 批量 uint8 饱和加法（运行时自动选择 AVX-2 / 标量）
/// 将两个 &[u8] 按元素做 sat8_add，结果写入 dst: &mut [u8]。
/// 长度必须相等（debug_assert 检查）。
///
/// 性能：
/// - AVX-2 路径：一次 vpaddusb 处理 32 个 u8，≈ 10-15x 加速
/// - 标量路径：4 字节分组让编译器自动向量化
pub fn sat8_add_batch(dst: &mut [u8], a: &[u8], b: &[u8]) {
    debug_assert_eq!(dst.len(), a.len());
    debug_assert_eq!(a.len(), b.len());

    if cfg!(target_arch = "x86_64") && cpu_supports_avx2() && dst.len() >= 32 {
        unsafe { x86_avx2::sat8_add_batch_avx2(dst, a, b) }
    } else {
        scalar_sat8_add_batch(dst, a, b)
    }
}

/// in-place 批量 uint8 饱和加法：dst = sat8_add(dst, src)
/// 脉冲传播核心场景：区块激活累加
#[inline]
pub fn sat8_add_batch_inplace(dst: &mut [u8], src: &[u8]) {
    debug_assert_eq!(dst.len(), src.len());

    if cfg!(target_arch = "x86_64") && cpu_supports_avx2() && dst.len() >= 32 {
        unsafe { x86_avx2::sat8_add_batch_inplace_avx2(dst, src) }
    } else {
        scalar_sat8_add_batch_inplace(dst, src)
    }
}

/// 批量 Q8.8 定点乘法（运行时自动选择 AVX-2 / 标量）
/// Q8.8 乘法：(a × b) >> 8 —— 等价于 Q8.8 × Q8.8 = Q8.8
/// 典型场景：脉冲传播强度衰减 new_strength = uint8_multiply(pulse, weight)
pub fn uint8_multiply_batch(dst: &mut [u8], a: &[u8], b: &[u8]) {
    debug_assert_eq!(dst.len(), a.len());
    debug_assert_eq!(a.len(), b.len());

    if cfg!(target_arch = "x86_64") && cpu_supports_avx2() && dst.len() >= 16 {
        unsafe { x86_avx2::uint8_multiply_batch_avx2(dst, a, b) }
    } else {
        scalar_uint8_multiply_batch(dst, a, b)
    }
}

/// 标量饱和钳位辅助（u16 → u8 饱和）
#[inline]
pub fn saturate_u8(value: u16) -> u8 {
    if value > 255 { 255 } else { value as u8 }
}

// ----------------------------------------------------------------------------
// 纯标量回退版本（非 x86-64 或 CPU 不支持 AVX-2 或数据量太小）
// ----------------------------------------------------------------------------
// 4 字节分组帮助编译器在 target-cpu=native 时自动向量化。
// 即使没有 AVX-2 intrinsic，标量路径也能吃到编译器 SIMD。
// ----------------------------------------------------------------------------

/// 标量版 sat8_add_batch
fn scalar_sat8_add_batch(dst: &mut [u8], a: &[u8], b: &[u8]) {
    let n = dst.len();
    let mut i = 0;
    while i + 4 <= n {
        let x0 = a[i] as u16 + b[i] as u16;
        let x1 = a[i + 1] as u16 + b[i + 1] as u16;
        let x2 = a[i + 2] as u16 + b[i + 2] as u16;
        let x3 = a[i + 3] as u16 + b[i + 3] as u16;
        dst[i] = if x0 > 255 { 255 } else { x0 as u8 };
        dst[i + 1] = if x1 > 255 { 255 } else { x1 as u8 };
        dst[i + 2] = if x2 > 255 { 255 } else { x2 as u8 };
        dst[i + 3] = if x3 > 255 { 255 } else { x3 as u8 };
        i += 4;
    }
    for j in i..n {
        let sum = (a[j] as u16) + (b[j] as u16);
        dst[j] = if sum > 255 { 255 } else { sum as u8 };
    }
}

/// 标量版 sat8_add_batch_inplace
fn scalar_sat8_add_batch_inplace(dst: &mut [u8], src: &[u8]) {
    let n = dst.len();
    let mut i = 0;
    while i + 4 <= n {
        let x0 = dst[i] as u16 + src[i] as u16;
        let x1 = dst[i + 1] as u16 + src[i + 1] as u16;
        let x2 = dst[i + 2] as u16 + src[i + 2] as u16;
        let x3 = dst[i + 3] as u16 + src[i + 3] as u16;
        dst[i] = if x0 > 255 { 255 } else { x0 as u8 };
        dst[i + 1] = if x1 > 255 { 255 } else { x1 as u8 };
        dst[i + 2] = if x2 > 255 { 255 } else { x2 as u8 };
        dst[i + 3] = if x3 > 255 { 255 } else { x3 as u8 };
        i += 4;
    }
    for j in i..n {
        let sum = (dst[j] as u16) + (src[j] as u16);
        dst[j] = if sum > 255 { 255 } else { sum as u8 };
    }
}

/// 标量版 uint8_multiply_batch
fn scalar_uint8_multiply_batch(dst: &mut [u8], a: &[u8], b: &[u8]) {
    let n = dst.len();
    let mut i = 0;
    while i + 4 <= n {
        dst[i] = ((a[i] as u16 * b[i] as u16) >> 8) as u8;
        dst[i + 1] = ((a[i + 1] as u16 * b[i + 1] as u16) >> 8) as u8;
        dst[i + 2] = ((a[i + 2] as u16 * b[i + 2] as u16) >> 8) as u8;
        dst[i + 3] = ((a[i + 3] as u16 * b[i + 3] as u16) >> 8) as u8;
        i += 4;
    }
    for j in i..n {
        dst[j] = ((a[j] as u16 * b[j] as u16) >> 8) as u8;
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::integer_ops::{sat8_add as scalar_sat8_add, uint8_multiply as scalar_mult};

    #[test]
    fn test_sat8_add_batch_zero() {
        let a = [0u8; 16];
        let b = [0u8; 16];
        let mut dst = [0u8; 16];
        sat8_add_batch(&mut dst, &a, &b);
        assert_eq!(dst, [0u8; 16]);
    }

    #[test]
    fn test_sat8_add_batch_saturation() {
        let a = [200u8; 8];
        let b = [200u8; 8];
        let mut dst = [0u8; 8];
        sat8_add_batch(&mut dst, &a, &b);
        assert_eq!(dst, [255u8; 8]);
    }

    #[test]
    fn test_sat8_add_batch_vs_scalar() {
        // 固定值对比 batch 结果和标量结果（确定性测试）
        let a: Vec<u8> = vec![0, 50, 100, 128, 150, 200, 250, 255, 10, 200, 30, 220, 42, 99, 204, 230,
                              1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let b: Vec<u8> = vec![0, 50, 100, 128, 150, 200, 250, 255, 100, 55, 180, 200, 210, 100, 50, 20,
                              100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200, 210, 220, 230, 240, 250];
        let mut dst = vec![0u8; 32];
        sat8_add_batch(&mut dst, &a, &b);
        for i in 0..a.len() {
            assert_eq!(dst[i], scalar_sat8_add(a[i], b[i]), "mismatch at idx {}", i);
        }
    }

    #[test]
    fn test_uint8_multiply_batch_zero() {
        let a = [128u8; 16];
        let b = [0u8; 16];
        let mut dst = [0u8; 16];
        uint8_multiply_batch(&mut dst, &a, &b);
        assert_eq!(dst, [0u8; 16]);
    }

    #[test]
    fn test_uint8_multiply_batch_vs_scalar() {
        // Q8.8 乘法结果应该与标量 uint8_multiply 完全一致
        let a: Vec<u8> = vec![255, 204, 128, 51, 26, 0, 100, 200];
        let b: Vec<u8> = vec![230, 230, 128, 255, 100, 255, 204, 51];
        let mut dst = vec![0u8; 8];
        uint8_multiply_batch(&mut dst, &a, &b);
        for i in 0..8 {
            assert_eq!(dst[i], scalar_mult(a[i], b[i]), "mismatch at idx {}: batch={}, scalar={}", i, dst[i], scalar_mult(a[i], b[i]));
        }
    }

    #[test]
    fn test_sat8_add_batch_inplace() {
        let mut dst = vec![100u8; 6];
        let src = vec![50u8, 100, 200, 0, 30, 200];
        sat8_add_batch_inplace(&mut dst, &src);
        assert_eq!(dst[0], 150);       // 100+50=150
        assert_eq!(dst[1], 200);       // 100+100=200
        assert_eq!(dst[2], 255);       // 100+200=300→饱和到255
        assert_eq!(dst[3], 100);       // 100+0=100
        assert_eq!(dst[4], 130);       // 100+30=130
        assert_eq!(dst[5], 255);       // 100+200=300→饱和到255
    }
}
