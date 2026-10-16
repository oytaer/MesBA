// ============================================================================
// abstraction.rs —— 抽象发现算法（设计文档 §9 环节 8 / §10.4）
// ============================================================================
// 功能：从多次推理的 Experience 区块（attempt_summary 步骤序列）中，
//       提取共同骨架（pattern_steps）→ 创建 Abstract 区块 + 建立 Diagonal 连接
// 示例：从加法/减法多次经验中发现"逐位运算 → 检查进位 → 汇总"的共同模式
//
// 核心函数：
//   discover_abstractions(network, min_pattern_freq) → Vec<AbstractionCandidate>
//   register_abstraction(network, candidate, level) → BlockId
// ============================================================================

use std::collections::{HashMap, HashSet};

use mesba_core::{Block, BlockContent, BlockId, BlockType};
use mesba_network::MeshBlockNetwork;

/// ============================================================================
/// AbstractionCandidate —— 发现的抽象模式候选
/// ============================================================================
#[derive(Debug, Clone)]
pub struct AbstractionCandidate {
    /// 模式名称（如 "逐位处理" / "checkpoint_and_backtrack"）
    pub name: String,
    /// 抽象后的步骤骨架（变量化）
    pub pattern_steps: Vec<String>,
    /// 抽象变量名（如 "position", "carry", "process_func"）
    pub abstract_variables: Vec<String>,
    /// 出现在多少个 Experience 中（频率）
    pub frequency: usize,
    /// 涉及的领域列表
    pub domains: HashSet<String>,
}

impl AbstractionCandidate {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            pattern_steps: Vec::new(),
            abstract_variables: Vec::new(),
            frequency: 0,
            domains: HashSet::new(),
        }
    }
}

// ============================================================================
// discover_abstractions —— 从 Experience 区块中提取抽象模式
// ============================================================================
/// 算法：
///   1. 扫描所有 Experience 区块 → 收集 attempt_summary（步骤序列）
///   2. 把每次步骤序列按空格/标点拆分成 tokens
///   3. 统计 2-gram / 3-gram 共现频率
///   4. 取 min_pattern_freq 以上的高频序列 → 组合成 pattern_steps + 变量化
pub fn discover_abstractions(
    network: &MeshBlockNetwork,
    min_pattern_freq: usize,
) -> Vec<AbstractionCandidate> {
    let mut candidates: Vec<AbstractionCandidate> = Vec::new();

    // 1) 收集所有 Experience 的 situation（BlockContent::Experience.attempt_summary 默认空串，我们用 situation）
    let experiences: Vec<(&Block, Vec<String>, String)> = network
        .find_by_type(BlockType::Experience)
        .iter()
        .filter_map(|block| {
            if let BlockContent::Experience {
                situation, ..
            } = &block.content
            {
                let tokens = tokenize_steps(situation);
                Some((*block, tokens, block.domain.clone()))
            } else {
                None
            }
        })
        .collect();

    if experiences.is_empty() {
        return candidates;
    }

    // 2) 统计 2-gram 和 3-gram 共现
    let mut bigram_freq: HashMap<(String, String), usize> = HashMap::new();
    let mut trigram_freq: HashMap<(String, String, String), usize> = HashMap::new();
    for (_, tokens, _) in &experiences {
        if tokens.len() >= 2 {
            for window in tokens.windows(2) {
                let key = (window[0].clone(), window[1].clone());
                *bigram_freq.entry(key).or_insert(0) += 1;
            }
        }
        if tokens.len() >= 3 {
            for window in tokens.windows(3) {
                let key = (window[0].clone(), window[1].clone(), window[2].clone());
                *trigram_freq.entry(key).or_insert(0) += 1;
            }
        }
    }

    // 3) 高频 trigram → 组合成 pattern_steps
    let mut pattern_map: HashMap<Vec<String>, HashSet<String>> = HashMap::new();
    let mut pattern_freq: HashMap<Vec<String>, usize> = HashMap::new();
    for ((a, b, c), freq) in &trigram_freq {
        if *freq >= min_pattern_freq {
            let pattern = vec![a.clone(), b.clone(), c.clone()];
            for (_, _, domain) in &experiences {
                // 简化：任何 Experience 都算覆盖此模式（MVP 够用）
                pattern_map.entry(pattern.clone()).or_default().insert(domain.clone());
            }
            *pattern_freq.entry(pattern).or_insert(0) += freq;
        }
    }

    // 4) 从 pattern 构造 AbstractionCandidate
    for (pattern, domains) in &pattern_map {
        let mut cand = AbstractionCandidate::new(&format!(
            "pattern_{}_{}_{}",
            pattern[0], pattern[1], pattern[2]
        ));
        cand.pattern_steps = pattern.clone();
        cand.abstract_variables = extract_variables_from_pattern(pattern);
        cand.frequency = pattern_freq.get(pattern).copied().unwrap_or(0);
        cand.domains = domains.clone();
        candidates.push(cand);
    }

    // 按频率降序（clippy unnecessary_sort_by → sort_by_key + Reverse）
    candidates.sort_by_key(|c| std::cmp::Reverse(c.frequency));

    candidates
}

// ============================================================================
// register_abstraction —— 在 network 中创建 Abstract 区块 + 建立 Diagonal 连接
// ============================================================================
/// 返回新区块的 BlockId；level = 抽象层级（1=具体，2=域间，3=跨域通用）
pub fn register_abstraction(
    network: &mut MeshBlockNetwork,
    candidate: &AbstractionCandidate,
    level: u32,
) -> BlockId {
    let block_id = BlockId::from(format!("Abstract.{}", candidate.name));

    // 1) 构造 BlockContent::Abstract
    let abstract_block = Block::abstract_block(
        block_id.clone(),
        "abstract.generic".to_string(),
        level,
        candidate.name.clone(),
        candidate.pattern_steps.clone(),
        candidate.abstract_variables.clone(),
    );

    // 2) 注册到 network
    network.register_block(abstract_block);

    // 3) 收集要连接的 Experience IDs（先 immutable borrow 收集，避免后面 connect 的 mutable 冲突）
    let mut targets: Vec<BlockId> = Vec::new();
    for (_, block) in network.blocks() {
        if block.block_type == BlockType::Experience
            && candidate.domains.contains(&block.domain)
        {
            targets.push(block.id.clone());
        }
    }

    // 4) 建立 Diagonal 连接（Experience → Abstract）
    for target in targets {
        network.connect_with_weight(
            target, // source（Experience）
            block_id.clone(), // target（Abstract）
            mesba_core::ConnectionType::Diagonal,
            40, // 初始权重 40（弱连接，后续 Hebbian 可强化）
            255, // 初始衰减 255（不衰减）
        );
    }

    block_id
}

// ============================================================================
// 内部工具函数
// ============================================================================

/// 把步骤摘要按空格/标点拆分成 tokens（过滤空串和过短词）
fn tokenize_steps(summary: &str) -> Vec<String> {
    summary
        .split(|c: char| c.is_whitespace() || ".,;:!?()[]{}\"".contains(c))
        .map(|w| w.trim().to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect()
}

/// 从 pattern_steps 提取"看起来像变量"的 token（纯数字跳过；名词/动词提取）
fn extract_variables_from_pattern(pattern: &[String]) -> Vec<String> {
    let mut vars: Vec<String> = pattern.to_vec();
    // 去掉纯数字
    vars.retain(|w| !w.chars().all(|c| c.is_ascii_digit()));
    // 去重保持顺序
    let mut seen = HashSet::new();
    vars.retain(|v| seen.insert(v.clone()));
    vars
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_steps() {
        let tokens = tokenize_steps("步骤1 步骤2 check 进位 overflow");
        assert!(tokens.contains(&"步骤1".to_string()));
        assert!(tokens.contains(&"check".to_string()));
        assert!(tokens.contains(&"进位".to_string()));
        // 单字符被过滤
        assert!(!tokens.contains(&"1".to_string()));
    }

    #[test]
    fn test_extract_variables_from_pattern() {
        let vars = extract_variables_from_pattern(&[
            "check".to_string(),
            "进位".to_string(),
            "123".to_string(), // 纯数字跳过
            "overflow".to_string(),
        ]);
        assert_eq!(vars.len(), 3);
        assert!(!vars.iter().any(|v| v == "123"));
    }

    #[test]
    fn test_discover_abstractions_empty_network() {
        let net = MeshBlockNetwork::new();
        let cands = discover_abstractions(&net, 2);
        assert!(cands.is_empty());
    }

    #[test]
    fn test_discover_abstractions_with_experiences() {
        // 构造 3 个 Experience 区块，两个有相同的 situation 前缀
        let mut net = MeshBlockNetwork::new();

        let exp1 = Block::experience(
            BlockId::from("Exp.add.1"),
            "math.arithmetic.addition".to_string(),
            1,
            "check 进位 process overflow".to_string(),
            "处理每一位检查进位".to_string(),
        );
        let exp2 = Block::experience(
            BlockId::from("Exp.add.2"),
            "math.arithmetic.addition".to_string(),
            1,
            "check 进位 process overflow".to_string(),
            "处理每一位检查进位".to_string(),
        );
        let exp3 = Block::experience(
            BlockId::from("Exp.sub.1"),
            "math.arithmetic.subtraction".to_string(),
            1,
            "check 借位 process zero".to_string(),
            "处理每一位检查借位".to_string(),
        );

        net.register_block(exp1);
        net.register_block(exp2);
        net.register_block(exp3);

        let cands = discover_abstractions(&net, 2);
        // check 进位 process 和 进位 process overflow 各出现 2 次（add 领域）
        assert!(cands.len() >= 2, "应该发现至少 2 个高频模式：{:?}", cands);

        let top = &cands[0];
        assert!(top.frequency >= 2);
        assert!(!top.pattern_steps.is_empty());
        assert!(top.domains.contains("math.arithmetic.addition"));
    }

    #[test]
    fn test_register_abstraction() {
        let mut net = MeshBlockNetwork::new();

        let exp1 = Block::experience(
            BlockId::from("Exp.add.1"),
            "math.arithmetic.addition".to_string(),
            1,
            "step1 step2".to_string(),
            "".to_string(),
        );
        net.register_block(exp1);

        let mut cand = AbstractionCandidate::new("test_pattern");
        cand.pattern_steps = vec!["step1".to_string(), "step2".to_string()];
        cand.abstract_variables = vec!["pos".to_string()];
        cand.domains.insert("math.arithmetic.addition".to_string());

        let bid = register_abstraction(&mut net, &cand, 2);
        assert!(net.contains_block(&bid));
        assert!(net.block_count() > 1);
        // Abstract 区块应该至少有一条 Diagonal 连接到 Experience
        let incoming_edges = net.incoming_connections(&bid);
        assert!(!incoming_edges.is_empty()); // Experience → Abstract
    }
}
