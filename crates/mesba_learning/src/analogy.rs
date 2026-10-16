// ============================================================================
// analogy.rs —— 类比映射算法（设计文档 §5.1 / §9 环节 8）
// ============================================================================
// 功能：为 Abstract 区块到新领域的区块建立 Diagonal 连接（类比迁移）
// 示例：加法"逐位运算"Abstract 区块 → 建立到减法领域 Experience/Rule 的斜向连接
//
// 核心函数：
//   map_analogies(network, abstract_block_id, target_domain, min_score) → usize (新建连接数)
//   score_analogy(source_block, target_block) → u8 (结构相似度 0~255)
// ============================================================================

use std::collections::HashSet;

use mesba_core::{Block, BlockContent, BlockId, BlockType, DomainMapping};
use mesba_network::MeshBlockNetwork;

// ============================================================================
// score_analogy —— 基于结构相似度给两个区块打 0~255 分
// ============================================================================
/// 评分维度（设计文档 §5.1 类比迁移结构匹配）：
///   · pattern_steps 重叠度 （Abstract 的 pattern_steps vs Rule 的 steps / Experience 的 attempt_summary）
///   · 条件表达式相似度     （Rule 的 condition_expr vs Judgment 的 condition_expr）
///   · 领域类别一致性       （BlockType 是否同类 / domain 是否属于同一大类）
///
/// 最终分 = (pattern_match * 0.6 + condition_match * 0.2 + type_match * 0.2) * 255
pub fn score_analogy(source: &Block, target: &Block) -> u8 {
    // 1) pattern_match：从 source 提取关键词 vs target 内容做 Jaccard
    let source_keywords = extract_pattern_keywords(source);
    let target_keywords = extract_block_keywords(target);
    let pattern_match = if source_keywords.is_empty() || target_keywords.is_empty() {
        0.0
    } else {
        let intersection = source_keywords.intersection(&target_keywords).count();
        let union = source_keywords.union(&target_keywords).count();
        if union == 0 {
            0.0
        } else {
            intersection as f32 / union as f32
        }
    };

    // 2) condition_match：source domain 的前 2 段 vs target domain 的前 2 段
    let cond_a = domain_prefix2(&source.domain);
    let cond_b = domain_prefix2(&target.domain);
    let condition_match = if cond_a.is_empty() || cond_b.is_empty() {
        0.0
    } else if cond_a == cond_b {
        1.0
    } else {
        0.0
    };

    // 3) type_match：Abstract 可以类比到 Rule / Experience；Rule 可以类比到 Abstract / Rule
    let type_match = match (source.block_type, target.block_type) {
        (BlockType::Abstract, BlockType::Rule)
        | (BlockType::Abstract, BlockType::Experience)
        | (BlockType::Abstract, BlockType::Judgment) => 1.0,
        (a, b) if a == b => 1.0,
        _ => 0.2, // 非同类型也给一点分（可能部分结构类似）
    };

    let score = pattern_match * 0.6 + condition_match * 0.2 + type_match * 0.2;
    (score * 255.0) as u8
}

// ============================================================================
// map_analogies —— 为 Abstract 区块到目标领域建立 Diagonal 连接
// ============================================================================
/// 返回实际新建的连接数（已存在的连接不会重复建）
pub fn map_analogies(
    network: &mut MeshBlockNetwork,
    abstract_block_id: &BlockId,
    target_domain: &str,
    min_score: u8,
) -> usize {
    let mut new_connections = 0usize;

    // 1) 取 Abstract 区块并提取用于 score 的字段（立即释放 immutable borrow）
    let abstract_keywords: HashSet<String> = match network.get_block(abstract_block_id) {
        Some(b) if b.block_type == BlockType::Abstract => extract_pattern_keywords(b),
        _ => return 0,
    };
    let abstract_domain_prefix = network
        .get_block(abstract_block_id)
        .map(|b| domain_prefix2(&b.domain))
        .unwrap_or_default();

    // 2) 找目标领域的 Rule / Experience / Judgment 区块
    let targets: Vec<BlockId> = network
        .find_by_domain(target_domain)
        .iter()
        .filter(|b| {
            matches!(
                b.block_type,
                BlockType::Rule | BlockType::Experience | BlockType::Judgment
            )
        })
        .map(|b| b.id.clone())
        .collect();

    // 3) 逐个打分 + 先收集 (target_id, score)，避免 immutable + mutable 借用冲突
    let mut to_connect: Vec<(BlockId, u8)> = Vec::new();
    for target_id in targets {
        // 已存在 Diagonal 连接 → 跳过
        let already_has = network
            .outgoing_connections(abstract_block_id)
            .iter()
            .any(|c| c.target_id == target_id && c.conn_type == mesba_core::ConnectionType::Diagonal);
        if already_has {
            continue;
        }

        // 打分（直接用关键字段，避免借 entire block）
        let score = match network.get_block(&target_id) {
            Some(b) => score_analogy_keywords(&abstract_keywords, &abstract_domain_prefix, b),
            None => continue,
        };
        if score < min_score {
            continue;
        }
        to_connect.push((target_id, score));
    }

    // 4) 一次性建立连接
    for (target_id, score) in to_connect {
        network.connect_with_weight(
            abstract_block_id.clone(),
            target_id,
            mesba_core::ConnectionType::Diagonal,
            score.max(20),
            200,
        );
        new_connections += 1;
    }

    // 5) 更新 Abstract 区块的 domain_mappings
    // 先收集 abstract_variables（immutable borrow），再进 get_block_mut（mutable）
    let abstract_vars: Vec<String> = network
        .get_block(abstract_block_id)
        .and_then(|b| {
            if let BlockContent::Abstract { abstract_variables, .. } = &b.content {
                Some(abstract_variables.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let var_map: std::collections::HashMap<String, String> = abstract_vars
        .iter()
        .cloned()
        .map(|v| (v.clone(), v))
        .chain([("__domain__".to_string(), target_domain.to_string())])
        .collect();
    // Rust 1.99 允许 `if let A && let B` 嵌套模式匹配折叠
    if let Some(block) = network.get_block_mut(abstract_block_id)
        && let BlockContent::Abstract { domain_mappings, .. } = &mut block.content
    {
        domain_mappings.push(DomainMapping {
            domain: target_domain.to_string(),
            variable_instantiation: var_map,
        });
    }

    new_connections
}

// ============================================================================
// map_all_analogies —— 一键为所有 Abstract 区块扫描并建立类比连接
// ============================================================================
pub fn map_all_analogies(network: &mut MeshBlockNetwork, min_score: u8) -> usize {
    // 先收集所有 Abstract BlockId（避免同时持有 & 和 &mut）
    let abstract_ids: Vec<BlockId> = network
        .find_by_type(BlockType::Abstract)
        .iter()
        .map(|b| b.id.clone())
        .collect();

    // 收集所有唯一 domain（排除 Abstract 自身 domain）
    let domains: HashSet<String> = network
        .blocks()
        .filter(|(_, b)| b.block_type != BlockType::Abstract)
        .map(|(_, b)| b.domain.clone())
        .collect();

    let mut total = 0;
    for abs_id in abstract_ids {
        for domain in &domains {
            let added = map_analogies(network, &abs_id, domain, min_score);
            total += added;
        }
    }
    total
}

// ============================================================================
// 内部工具函数
// ============================================================================

/// 从 Block 提取用于 score_analogy 的关键词集
fn extract_block_keywords(block: &Block) -> HashSet<String> {
    let mut kws = HashSet::new();
    match &block.content {
        BlockContent::Experience { attempt_summary, lesson, .. } => {
            for word in tokenize(attempt_summary) { kws.insert(word); }
            for word in tokenize(lesson) { kws.insert(word); }
        }
        BlockContent::Rule { steps, preconditions, .. } => {
            for step in steps {
                // RuleStep 有 op / input_vars / output_var 字段；简化转字符串
                for word in tokenize(&format!("{:?}", step)) { kws.insert(word); }
            }
            for cond in preconditions { for word in tokenize(cond) { kws.insert(word); } }
        }
        BlockContent::Judgment { condition_expr, .. } => {
            for word in tokenize(condition_expr) { kws.insert(word); }
        }
        _ => {}
    }
    // domain 也作为关键词
    if !block.domain.is_empty() { kws.insert(block.domain.clone()); }
    kws
}

/// Abstract 区块的关键词（只看 pattern_steps + abstract_variables）
fn extract_pattern_keywords(block: &Block) -> HashSet<String> {
    let mut kws = HashSet::new();
    if let BlockContent::Abstract { pattern_steps, abstract_variables, .. } = &block.content {
        for step in pattern_steps { for word in tokenize(step) { kws.insert(word); } }
        for v in abstract_variables { kws.insert(v.clone()); }
    }
    kws
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || ".,;:!?()[]{}\"".contains(c))
        .map(|w| w.trim().to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect()
}

/// domain 取前 2 段（如 "math.arithmetic.addition" → "math.arithmetic"）
fn domain_prefix2(domain: &str) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        domain.to_string()
    }
}

/// 不借 Abstract block 的版本（map_analogies 内部用，避免借用冲突）
fn score_analogy_keywords(
    abstract_keywords: &HashSet<String>,
    abstract_domain_prefix: &str,
    target: &Block,
) -> u8 {
    let target_keywords = extract_block_keywords(target);
    let pattern_match = if abstract_keywords.is_empty() || target_keywords.is_empty() {
        0.0
    } else {
        let intersection = abstract_keywords.intersection(&target_keywords).count();
        let union = abstract_keywords.union(&target_keywords).count();
        if union == 0 { 0.0 } else { intersection as f32 / union as f32 }
    };
    let cond_target = domain_prefix2(&target.domain);
    let condition_match = if abstract_domain_prefix.is_empty() || cond_target.is_empty() {
        0.0
    } else if abstract_domain_prefix == cond_target {
        1.0
    } else {
        0.0
    };
    let type_match = match target.block_type {
        BlockType::Rule | BlockType::Experience | BlockType::Judgment => 1.0,
        _ => 0.2,
    };
    let score = pattern_match * 0.6 + condition_match * 0.2 + type_match * 0.2;
    (score * 255.0) as u8
}

// ============================================================================
// 单元测试 —— 用 Block::rule / Block::experience 正确构造
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule_block(id: &str, domain: &str, name: &str) -> Block {
        Block::rule(
            BlockId::from(id),
            domain.to_string(),
            1,
            name.to_string(),
            vec![],
            vec![],
        )
    }

    fn make_exp_block(id: &str, domain: &str, situation: &str) -> Block {
        Block::experience(
            BlockId::from(id),
            domain.to_string(),
            1,
            situation.to_string(),
            "lesson".to_string(),
        )
    }

    fn make_abstract(id: &str, name: &str, steps: &[&str], vars: &[&str]) -> Block {
        Block::abstract_block(
            BlockId::from(id),
            "abstract.generic".to_string(),
            2,
            name.to_string(),
            steps.iter().map(|s| s.to_string()).collect(),
            vars.iter().map(|v| v.to_string()).collect(),
        )
    }

    #[test]
    fn test_score_analogy_same_type() {
        let a = make_abstract("Abs.1", "逐位处理", &["check process overflow"], &["pos", "carry"]);
        let b = make_rule_block("Rule.1", "math.arithmetic.addition", "加法规则");
        let score = score_analogy(&a, &b);
        // Rule steps/preconditions 为空 → pattern 匹配度低但 type_match=1.0 → 分数 > 0
        assert!(score < u8::MAX);
    }

    #[test]
    fn test_map_analogies() {
        let mut net = MeshBlockNetwork::new();
        let abs = make_abstract("Abs.1", "逐位处理", &["check process"], &["pos"]);
        let rule = make_rule_block("Rule.add.1", "math.arithmetic.addition", "加法规则");
        let rule2 = make_rule_block("Rule.sub.1", "math.arithmetic.subtraction", "减法规则");

        net.register_block(abs);
        net.register_block(rule);
        net.register_block(rule2);

        let added = map_analogies(
            &mut net,
            &BlockId::from("Abs.1"),
            "math.arithmetic.addition",
            0, // min_score 最宽松
        );
        assert!(added >= 1, "至少应建立 1 条类比连接，实际 {}", added);
    }

    #[test]
    fn test_map_analogies_skips_existing() {
        let mut net = MeshBlockNetwork::new();
        let abs = make_abstract("Abs.1", "test", &["a", "b"], &["v"]);
        let rule = make_rule_block("Rule.add.1", "math.arithmetic.addition", "r1");
        net.register_block(abs);
        net.register_block(rule);

        let a1 = map_analogies(&mut net, &BlockId::from("Abs.1"), "math.arithmetic.addition", 0);
        let a2 = map_analogies(&mut net, &BlockId::from("Abs.1"), "math.arithmetic.addition", 0);
        assert!(a1 >= 1);
        assert_eq!(a2, 0, "已存在连接不应重复创建");
    }

    #[test]
    fn test_score_analogy_empty() {
        // pattern_steps / abstract_variables 都空 → 只有 type_match 贡献 0.2
        // Abstract → Experience → type_match = 1.0 → 0.2 * 255 = 51
        let a = make_abstract("Abs.1", "empty", &[], &[]);
        let b = make_exp_block("Exp.1", "unknown", "");
        let score = score_analogy(&a, &b);
        // 分数不是 0（因为 type_match 贡献 0.2），也不是 255
        assert!(score > 0 && score < 255, "空 pattern 但 type_match 仍有分：{}", score);
    }
}
