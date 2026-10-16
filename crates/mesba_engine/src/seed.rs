// ============================================================================
// seed.rs —— MVP 种子区块初始化器（一位加法 + 两位加法 + 完整分解链）
// ============================================================================
// 设计文档 §15.6 MVP 种子区块：加法推理网络
//
// 架构：8 环认知结构的完整最小实现
//   Perceptual → Concept → Plan → Rule → Judgment → Operation → Fact → Abstract
//
// 衰减曲线设计（关键！）：
//   默认 Vertical weight=230, decay=230 → 8  hop 后 ≈ 44（Concept threshold=80 会断裂）
//   Boosted  weight=254, decay=254     → 10 hop 后 ≈ 235（全阈值安全）
//   两位数分解链必须使用 boosted connections！
//
// 本模块提供：
//   1. build_single_digit_addition(a, b) —— 单数字 4 区块链
//   2. build_addition_table() —— 100 Fact 完整表
//   3. build_two_digit_addition(a, b) —— 两位分解全链路
//   4. 端到端传播测试（含 23+45→68）
// ============================================================================

use mesba_core::{
    Block, BlockId, ConnectionId, ConnectionType, FactEntry, FactValue,
};
use mesba_network::MeshBlockNetwork;

/// MVP 领域前缀（所有种子区块归属同一领域）
pub const MVP_DOMAIN: &str = "arithmetic_mvp";

/// =========================================================================
/// 连接工具函数 —— 支持 weight/decay 微调
/// =========================================================================
/// 创建连接后手动 boost weight/decay（用于长链路防衰减断裂）
/// 返回 ConnectionId（方便调试）
fn connect_boosted(
    net: &mut MeshBlockNetwork,
    source: BlockId,
    target: BlockId,
    conn_type: ConnectionType,
    weight: u8,
    decay: u8,
) -> ConnectionId {
    let conn_id = net.connect(source, target, conn_type);
    if let Some(conn) = net.get_connection_mut(&conn_id) {
        conn.weight = weight;
        conn.decay = decay;
    }
    conn_id
}

/// 最强配置：weight=254, decay=254 — 10  hop 后仍有 ~235 强度
const BOOSTED_WEIGHT: u8 = 254;
const BOOSTED_DECAY: u8 = 254;

// ============================================================================
// build_single_digit_addition —— 构建单数字加法网络
// ============================================================================
// 最小 4 区块链，全部默认 Vertical 连接（4  hop 足够）
// ============================================================================
pub fn build_single_digit_addition(a: u8, b: u8) -> MeshBlockNetwork {
    let mut net = MeshBlockNetwork::new();

    let sum = a + b;
    let input_key = format!("{}+{}", a, b);

    // --- Level 1: Perceptual ---
    let perceptual_id = BlockId::from(format!("percept_{}_{}", a, b));
    net.register_block(Block::perceptual(
        perceptual_id.clone(),
        MVP_DOMAIN.to_string(),
        1,
        "math_expression".to_string(),
        vec![],
    ));

    // --- Level 2: Concept ---
    let concept_id = BlockId::from("concept_addition");
    net.register_block(Block::concept(
        concept_id.clone(),
        MVP_DOMAIN.to_string(),
        2,
        "加法运算".to_string(),
    ));

    // --- Level 3: Plan ---
    let plan_id = BlockId::from("plan_single_digit");
    net.register_block(Block::plan(
        plan_id.clone(),
        MVP_DOMAIN.to_string(),
        3,
        format!("一位加法 {}+{}", a, b),
        vec![],
    ));

    // --- Level 4: Fact（具体查表结果）---
    let fact_id = BlockId::from(format!("fact_add_{}_{}", a, b));
    net.register_block(Block::fact(
        fact_id.clone(),
        MVP_DOMAIN.to_string(),
        4,
        vec![FactEntry {
            keys: vec![input_key],
            value: FactValue::Integer(sum as i32),
        }],
    ));

    // 默认 Vertical（4  hop 够用，每 hop ≈ 0.78 → 4 hop ≈ 0.37 → 255*0.37 ≈ 94）
    net.connect(perceptual_id, concept_id.clone(), ConnectionType::Vertical);
    net.connect(concept_id, plan_id.clone(), ConnectionType::Vertical);
    net.connect(plan_id, fact_id, ConnectionType::Vertical);

    net
}

// ============================================================================
// build_addition_table —— 构建完整 100 个 Fact 加法表
// ============================================================================
pub fn build_addition_table() -> MeshBlockNetwork {
    let mut net = MeshBlockNetwork::new();

    let perc_id = BlockId::from("perc_addition_input");
    net.register_block(Block::perceptual(
        perc_id.clone(),
        MVP_DOMAIN.to_string(),
        1,
        "digit_addition".to_string(),
        vec![],
    ));

    let concept_id = BlockId::from("concept_addition");
    net.register_block(Block::concept(
        concept_id.clone(),
        MVP_DOMAIN.to_string(),
        2,
        "加法运算".to_string(),
    ));
    net.connect(perc_id, concept_id.clone(), ConnectionType::Vertical);

    let plan_id = BlockId::from("plan_single_digit");
    net.register_block(Block::plan(
        plan_id.clone(),
        MVP_DOMAIN.to_string(),
        3,
        "单数字加法计算".to_string(),
        vec![],
    ));
    net.connect(concept_id, plan_id.clone(), ConnectionType::Vertical);

    // --- Level 3.5: Rule（进位传播规则）---
    let rule_carry_id = BlockId::from("rule_carry_propagation");
    net.register_block(Block::rule(
        rule_carry_id.clone(),
        MVP_DOMAIN.to_string(),
        3,
        "加法进位规则".to_string(),
        vec!["sum >= 10".to_string()],
        vec![mesba_core::RuleStep {
            id: "rs_carry_gen".to_string(),
            action: "set".to_string(),
            target: None,
            variable: Some("carry".to_string()),
            value_text: Some("1".to_string()),
            description: "生成 carry=1 并传递给十位加法".to_string(),
            loop_until: None,
        }],
    ));
    connect_boosted(&mut net, plan_id.clone(), rule_carry_id.clone(), ConnectionType::Vertical, 200, 200);

    // --- Level 3.6: Judgment（sum >= 10? 进位检测）---
    let judgment_carry_check_id = BlockId::from("judgment_carry_check");
    net.register_block(Block::judgment(
        judgment_carry_check_id.clone(),
        MVP_DOMAIN.to_string(),
        4,
        "sum >= 10".to_string(),
        vec!["sum".to_string()],
        mesba_core::JudgmentBranch {
            action: "set_carry".to_string(),
            target: None,
            set_variables: {
                let mut m = std::collections::HashMap::new();
                m.insert("carry".to_string(), "1".to_string());
                m
            },
        },
        mesba_core::JudgmentBranch {
            action: "no_carry".to_string(),
            target: None,
            set_variables: {
                let mut m = std::collections::HashMap::new();
                m.insert("carry".to_string(), "0".to_string());
                m
            },
        },
    ));
    connect_boosted(&mut net, rule_carry_id.clone(), judgment_carry_check_id.clone(), ConnectionType::Vertical, 200, 200);

    // --- Level 3.7: Operation（两位数的数字提取）---
    let op_extract_id = BlockId::from("op_extract_digits");
    net.register_block(Block::operation_internal(
        op_extract_id.clone(),
        MVP_DOMAIN.to_string(),
        4,
        "从两位数中提取个位和十位数字".to_string(),
        vec!["a".to_string(), "b".to_string()],
        vec!["a_ones".to_string(), "b_ones".to_string(), "a_tens".to_string(), "b_tens".to_string()],
        "a%10+b%10".to_string(),
    ));
    connect_boosted(&mut net, plan_id.clone(), op_extract_id.clone(), ConnectionType::Vertical, 200, 200);

    // --- Level 3.8: Experience（常见错误模式）---
    let exp_no_carry_id = BlockId::from("exp_forgot_carry");
    net.register_block(Block::experience(
        exp_no_carry_id.clone(),
        MVP_DOMAIN.to_string(),
        5,
        "两位加法忘记进位导致结果偏差 10".to_string(),
        "遇到和>=10必须设置 carry=1 并加到十位".to_string(),
    ));
    connect_boosted(&mut net, plan_id.clone(), exp_no_carry_id.clone(), ConnectionType::Loop, 200, 200);

    // --- Level 3.9: Abstract（通用加法模式）---
    let abstract_add_id = BlockId::from("abstract_addition_pattern");
    net.register_block(Block::abstract_block(
        abstract_add_id.clone(),
        MVP_DOMAIN.to_string(),
        8,
        "通用逐位加法算法".to_string(),
        vec!["提取个位".to_string(), "个位相加".to_string(), "判断进位".to_string(), "提取十位".to_string(), "十位相加".to_string(), "重组结果".to_string()],
        vec!["a_ones".to_string(), "b_ones".to_string(), "a_tens".to_string(), "b_tens".to_string(), "carry".to_string(), "final_result".to_string()],
    ));
    connect_boosted(&mut net, plan_id.clone(), abstract_add_id.clone(), ConnectionType::Diagonal, 200, 200);

    // 100 个 Fact（0+0~9+9）
    for a in 0u8..=9 {
        for b in 0u8..=9 {
            let sum = a + b;
            let fact_id = BlockId::from(format!("fact_add_{}_{}", a, b));
            net.register_block(Block::fact(
                fact_id.clone(),
                MVP_DOMAIN.to_string(),
                4,
                vec![FactEntry {
                    keys: vec![format!("{}+{}", a, b)],
                    value: FactValue::Integer(sum as i32),
                }],
            ));
            net.connect(plan_id.clone(), fact_id, ConnectionType::Vertical);
        }
    }

    net
}

// ============================================================================
// build_two_digit_addition —— 构建双位数分解加法网络
// ============================================================================
// 完整 10+  hop 链路，全部 boosted connections
//
// 分解链（以 23+45 为例）：
//   Perceptual "23+45"
//     → Concept "加法"
//       → Plan "两位加法"
//         → Operation "提取个位" (3, 5)
//           → Fact "3+5=8" (个位和)
//             → Judgment "和>=10?" (8<10 → No carry)
//               → Operation "提取十位" (2, 4)
//                 → Fact "2+4=6" (十位和)
//                   → Operation "重组" (十位×10+个位)
//                     → Abstract "结果 68"
// ============================================================================
pub fn build_two_digit_addition(a: u16, b: u16) -> MeshBlockNetwork {
    let mut net = MeshBlockNetwork::new();

    let a_tens = (a / 10) as u8;
    let a_ones = (a % 10) as u8;
    let b_tens = (b / 10) as u8;
    let b_ones = (b % 10) as u8;
    let ones_sum = a_ones + b_ones;
    let carry = ones_sum >= 10;
    let tens_sum_raw = a_tens + b_tens;
    let tens_sum = if carry { tens_sum_raw + 1 } else { tens_sum_raw };
    let final_result = tens_sum as u16 * 10 + (if carry { ones_sum - 10 } else { ones_sum } as u16);

    // ============ Level 1: Perceptual ============
    let perc_id = BlockId::from(format!("percept_two_{}_{}", a, b));
    net.register_block(Block::perceptual(
        perc_id.clone(),
        MVP_DOMAIN.to_string(),
        1,
        "two_digit_expression".to_string(),
        vec![],
    ));

    // ============ Level 2: Concept ============
    let concept_id = BlockId::from("concept_addition");
    if !net.contains_block(&concept_id) {
        net.register_block(Block::concept(
            concept_id.clone(),
            MVP_DOMAIN.to_string(),
            2,
            "加法运算：多位数分解计算".to_string(),
        ));
    }

    // ============ Level 3: Plan ============
    let plan_id = BlockId::from(format!("plan_two_digit_{}_{}", a, b));
    net.register_block(Block::plan(
        plan_id.clone(),
        MVP_DOMAIN.to_string(),
        3,
        format!("两位加法 {}+{}", a, b),
        vec![],
    ));

    // ============ Level 4: Operations ============
    // 每个 Operation 块持有可执行的算法字符串：
    //   - input_variables: 纯变量名（execute_algorithm 从 buffer_vars 查）
    //   - output_variables: 结果存入哪个 buffer_vars 变量
    //   - algorithm: execute_algorithm 能直接解析的格式

    // 4a: 提取个位并求和 → ones_sum
    let op_extract_ones_id = BlockId::from(format!("op_extract_ones_{}_{}", a, b));
    net.register_block(Block::operation_internal(
        op_extract_ones_id.clone(),
        MVP_DOMAIN.to_string(),
        4,
        "提取个位并求和 a%10+b%10 → ones_sum".to_string(),
        vec!["a".to_string(), "b".to_string()],
        vec!["ones_sum".to_string()],
        "a%10+b%10".to_string(),
    ));

    // 4b: 提取十位并加上 carry → tens_sum
    let op_extract_tens_id = BlockId::from(format!("op_extract_tens_{}_{}", a, b));
    net.register_block(Block::operation_internal(
        op_extract_tens_id.clone(),
        MVP_DOMAIN.to_string(),
        4,
        "提取十位加 carry: a/10+b/10+carry → tens_sum".to_string(),
        vec!["a".to_string(), "b".to_string(), "carry".to_string()],
        vec!["tens_sum".to_string()],
        "a/10+b/10+carry".to_string(),
    ));

    // 4c: 重组最终结果（十位×10 + 个位，减去 carry*10 把 ones_sum ≥ 10 修正掉）
    let op_recombine_id = BlockId::from(format!("op_recombine_{}_{}", a, b));
    net.register_block(Block::operation_internal(
        op_recombine_id.clone(),
        MVP_DOMAIN.to_string(),
        8,
        "重组 tens_sum*10+ones_sum-carry*10 → final_result".to_string(),
        vec![
            "tens_sum".to_string(),
            "ones_sum".to_string(),
            "carry".to_string(),
        ],
        vec!["final_result".to_string()],
        "tens_sum*10+ones_sum-carry*10".to_string(),
    ));

    // ============ Level 5: Facts ============

    // 5a: 个位加法 Fact
    let fact_ones_id = BlockId::from(format!("fact_add_{}_{}", a_ones, b_ones));
    if !net.contains_block(&fact_ones_id) {
        net.register_block(Block::fact(
            fact_ones_id.clone(),
            MVP_DOMAIN.to_string(),
            5,
            vec![FactEntry {
                keys: vec![format!("{}+{}", a_ones, b_ones)],
                value: FactValue::Integer(ones_sum as i32),
            }],
        ));
    }

    // 5b: 十位加法 Fact（加上进位）
    let fact_tens_id = BlockId::from(format!("fact_tens_{}_{}", a_tens, b_tens));
    if !net.contains_block(&fact_tens_id) {
        net.register_block(Block::fact(
            fact_tens_id.clone(),
            MVP_DOMAIN.to_string(),
            5,
            vec![FactEntry {
                keys: vec![format!("tens_{}+{}", a_tens, b_tens)],
                value: FactValue::Integer(tens_sum as i32),
            }],
        ));
    }

    // ============ Level 6: Judgment（进位判断）========
    // condition_expr: "ones_sum >= 10" — 无问号，直接布尔表达式
    // true_branch / false_branch: target=None → 只设 carry 变量，不阻断机械传播
    //   Propagator 会先 apply set_variables（carry=0/1 写入 buffer_vars）
    //   然后 fall-through 继续走 outgoing 连接（OpExtractTens）
    let judgment_carry_id = BlockId::from(format!("judgment_carry_{}_{}", a_ones, b_ones));
    net.register_block(Block::judgment(
        judgment_carry_id.clone(),
        MVP_DOMAIN.to_string(),
        6,
        "ones_sum >= 10".to_string(),
        vec!["ones_sum".to_string()],
        mesba_core::JudgmentBranch {
            action: "carry_on".to_string(),
            target: None,
            set_variables: {
                let mut m = std::collections::HashMap::new();
                m.insert("carry".to_string(), "1".to_string());
                m
            },
        },
        mesba_core::JudgmentBranch {
            action: "carry_off".to_string(),
            target: None,
            set_variables: {
                let mut m = std::collections::HashMap::new();
                m.insert("carry".to_string(), "0".to_string());
                m
            },
        },
    ));

    // ============ Level 7: Abstract（最终结果）========
    let abstract_result_id = BlockId::from(format!("abstract_result_{}_{}", a, b));
    net.register_block(Block::abstract_block(
        abstract_result_id.clone(),
        MVP_DOMAIN.to_string(),
        9,
        format!("{}+{}={}", a, b, final_result),
        vec![format!("result={}", final_result)],
        vec![format!("final_sum={}", final_result)],
    ));

    // ============ 连接（全部 BOOSTED，weight=254, decay=254）========
    // 链 1: Perceptual → Concept → Plan → OpExtractOnes → FactOnes
    connect_boosted(&mut net, perc_id.clone(), concept_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, concept_id.clone(), plan_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, plan_id.clone(), op_extract_ones_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, op_extract_ones_id, fact_ones_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);

    // 链 2: FactOnes → JudgmentCarry → OpExtractTens → FactTens
    connect_boosted(&mut net, fact_ones_id.clone(), judgment_carry_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, judgment_carry_id, op_extract_tens_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, op_extract_tens_id, fact_tens_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);

    // 链 3: FactTens → OpRecombine → AbstractResult
    connect_boosted(&mut net, fact_tens_id.clone(), op_recombine_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);
    connect_boosted(&mut net, op_recombine_id, abstract_result_id.clone(), ConnectionType::Vertical, BOOSTED_WEIGHT, BOOSTED_DECAY);

    net
}

// ============================================================================
// get_fact_value —— 查表辅助（从 Fact 提取 Integer 值）
// ============================================================================
pub fn get_fact_value(net: &MeshBlockNetwork, a: u8, b: u8) -> Option<i32> {
    let fact_id = BlockId::from(format!("fact_add_{}_{}", a, b));
    net.get_block(&fact_id).and_then(|blk| {
        if let mesba_core::BlockContent::Fact { entries } = &blk.content {
            entries.first().and_then(|e| {
                if let FactValue::Integer(v) = e.value {
                    Some(v)
                } else {
                    None
                }
            })
        } else {
            None
        }
    })
}

// ============================================================================
// 单元测试 —— MVP 端到端验证
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagator::PulsePropagator;
    use mesba_core::{Pulse, PulseId};

    // ---------- 一位加法测试 ----------

    #[test]
    fn test_single_digit_3_5_network_exists() {
        let net = build_single_digit_addition(3, 5);
        assert_eq!(net.blocks().count(), 4);
        assert_eq!(net.connections().count(), 3);
        assert!(net.contains_block(&BlockId::from("percept_3_5")));
        assert!(net.contains_block(&BlockId::from("concept_addition")));
        assert!(net.contains_block(&BlockId::from("plan_single_digit")));
        assert!(net.contains_block(&BlockId::from("fact_add_3_5")));
    }

    #[test]
    fn test_addition_table_has_100_facts() {
        let net = build_addition_table();
        let fact_count = net
            .blocks()
            .filter(|(_, b)| b.block_type == mesba_core::BlockType::Fact)
            .count();
        assert_eq!(fact_count, 100);
    }

    #[test]
    fn test_addition_table_has_all_nine_block_types() {
        let net = build_addition_table();
        // 计数每种类型
        let counts: std::collections::HashMap<_, usize> = net
            .blocks()
            .map(|(_, b)| b.block_type)
            .fold(std::collections::HashMap::new(), |mut h, t| {
                *h.entry(t).or_insert(0) += 1;
                h
            });
        // 必须拥有全部 9 种类型
        assert!(
            counts.contains_key(&mesba_core::BlockType::Perceptual),
            "缺 Perceptual"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Concept),
            "缺 Concept"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Plan),
            "缺 Plan"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Fact),
            "缺 Fact"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Rule),
            "缺 Rule"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Judgment),
            "缺 Judgment"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Operation),
            "缺 Operation"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Experience),
            "缺 Experience"
        );
        assert!(
            counts.contains_key(&mesba_core::BlockType::Abstract),
            "缺 Abstract"
        );
        // Fact 仍为 100
        assert_eq!(
            *counts.get(&mesba_core::BlockType::Fact).unwrap(),
            100
        );
    }

    #[test]
    fn test_fact_lookup_values_correct() {
        let net = build_addition_table();
        assert_eq!(get_fact_value(&net, 0, 0), Some(0));
        assert_eq!(get_fact_value(&net, 3, 5), Some(8));
        assert_eq!(get_fact_value(&net, 7, 2), Some(9));
        assert_eq!(get_fact_value(&net, 9, 9), Some(18));
    }

    #[test]
    fn test_three_plus_five_propagates_to_fact() {
        let mut net = build_single_digit_addition(3, 5);
        let mut prop = PulsePropagator::new(&mut net);

        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("INPUT"),
            BlockId::from("percept_3_5"),
            255,
            vec![],
            0,
        ));

        prop.run_system2();

        let fact_block = net.get_block(&BlockId::from("fact_add_3_5")).unwrap();
        assert!(
            fact_block.state.activation >= fact_block.state.threshold,
            "Fact block 3+5 should activate: act={}, thr={}",
            fact_block.state.activation,
            fact_block.state.threshold
        );
    }

    #[test]
    fn test_all_single_digit_sums_propagate() {
        let test_cases: &[(u8, u8, i32)] = &[
            (0, 0, 0), (1, 2, 3), (4, 5, 9), (7, 8, 15),
        ];

        for &(a, b, expected) in test_cases {
            let mut net = build_single_digit_addition(a, b);
            let mut prop = PulsePropagator::new(&mut net);

            prop.inject(Pulse::activate(
                PulseId(1),
                BlockId::from("IN"),
                BlockId::from(format!("percept_{}_{}", a, b)),
                255,
                vec![],
                0,
            ));
            prop.run_system2();

            let fact = net.get_block(&BlockId::from(format!("fact_add_{}_{}", a, b))).unwrap();
            assert!(
                fact.state.activation >= fact.state.threshold,
                "Fact {}+{} should activate",
                a, b
            );
            assert_eq!(get_fact_value(&net, a, b), Some(expected));
        }
    }

    // ---------- 两位加法测试 ----------

    #[test]
    fn test_two_digit_network_structure() {
        let net = build_two_digit_addition(23, 45);
        // 至少应有 8 个 block: Perceptual + Concept + Plan + 3 Operation + 2 Fact + Judgment + Abstract
        let block_count = net.blocks().count();
        assert!(block_count >= 9, "两位数网络应有 ≥ 9 块，实际 {}", block_count);

        // 检查关键 block 存在
        assert!(net.contains_block(&BlockId::from("percept_two_23_45")));
        assert!(net.contains_block(&BlockId::from("concept_addition")));
        assert!(net.contains_block(&BlockId::from("plan_two_digit_23_45")));
        assert!(net.contains_block(&BlockId::from("fact_add_3_5")));    // 个位 3+5=8
        assert!(net.contains_block(&BlockId::from("fact_tens_2_4")));  // 十位 2+4=6
        assert!(net.contains_block(&BlockId::from("judgment_carry_3_5")));
        assert!(net.contains_block(&BlockId::from("abstract_result_23_45")));

        // 检查连接全部 boosted
        for (_, conn) in net.connections() {
            assert!(
                conn.weight >= 250,
                "Connection weight should be boosted (≥250), got {}",
                conn.weight
            );
            assert!(
                conn.decay >= 250,
                "Connection decay should be boosted (≥250), got {}",
                conn.decay
            );
        }
    }

    #[test]
    fn test_two_digit_23_45_propagates_full_chain() {
        let mut net = build_two_digit_addition(23, 45);
        let mut prop = PulsePropagator::new(&mut net);

        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("INPUT"),
            BlockId::from("percept_two_23_45"),
            255,
            vec![],
            0,
        ));

        let history = prop.run_system2();
        assert!(!history.is_empty(), "应至少有 step 执行");

        // 检查关键 block 是否被激活
        let perceptual = net.get_block(&BlockId::from("percept_two_23_45")).unwrap();
        assert!(perceptual.state.activation >= perceptual.state.threshold,
            "Perceptual should activate");

        let concept = net.get_block(&BlockId::from("concept_addition")).unwrap();
        assert!(concept.state.activation >= concept.state.threshold,
            "Concept should activate");

        let plan = net.get_block(&BlockId::from("plan_two_digit_23_45")).unwrap();
        assert!(plan.state.activation >= plan.state.threshold,
            "Plan should activate");

        let fact_ones = net.get_block(&BlockId::from("fact_add_3_5")).unwrap();
        assert!(fact_ones.state.activation >= fact_ones.state.threshold,
            "Fact ones (3+5=8) should activate");

        let abstract_result = net.get_block(&BlockId::from("abstract_result_23_45")).unwrap();
        assert!(abstract_result.state.activation >= abstract_result.state.threshold,
            "Abstract result should be the final activated block!");
    }

    #[test]
    fn test_two_digit_with_carry_47_38() {
        // 47+38=85 (有进位: 7+8=15, carry 1 → 4+3+1=8)
        let mut net = build_two_digit_addition(47, 38);
        let mut prop = PulsePropagator::new(&mut net);

        prop.inject(Pulse::activate(
            PulseId(1),
            BlockId::from("INPUT"),
            BlockId::from("percept_two_47_38"),
            255,
            vec![],
            0,
        ));
        prop.run_system2();

        let abstract_block = net.get_block(&BlockId::from("abstract_result_47_38")).unwrap();
        assert!(
            abstract_block.state.activation >= abstract_block.state.threshold,
            "带进位的两位数加法应能走到最终结果"
        );
    }

    #[test]
    fn test_two_digit_connection_strength_not_weakened() {
        // 验证 boosted 连接的强度传递：254*254/256 = ~252.4 → 不会饱和
        let weight_strength = mesba_core::uint8_multiply(255u8, BOOSTED_WEIGHT);
        let full_strength = mesba_core::uint8_multiply(weight_strength, BOOSTED_DECAY);
        assert!(
            full_strength >= 200,
            "Boosted 连接单 hop 后强度应 ≥ 200，实际 {}",
            full_strength
        );
    }

    #[test]
    fn test_two_digit_decay_curve_over_10_hops() {
        // 模拟 10 hop 后的强度衰减（boosted 254/254）
        let mut strength = 255u16;
        for _ in 0..10 {
            let mult = mesba_core::uint8_multiply(strength as u8, BOOSTED_WEIGHT) as u16;
            let decayed = mesba_core::uint8_multiply(mult as u8, BOOSTED_DECAY) as u16;
            strength = decayed;
        }
        assert!(
            strength >= 150,
            "10 hop 后强度应 ≥ 150（远高于所有 threshold），实际 {}",
            strength
        );
    }
}
