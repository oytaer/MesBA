// ============================================================================
// episodic.rs —— L2 情景记忆（设计文档 §7.1 三级记忆长期子系统）
// ============================================================================
// 长期记忆三大子系统（设计文档 §7.1）：
//   · 语义记忆（已实现：Fact 区块）
//   · 情景记忆（本模块：Experience 区块存储 + 相似度搜索）
//   · 程序记忆（已实现：Rule 区块）
//
// EpisodicMemory 存储 MesBA 每次"经验"——问题情境 + 尝试方案 + 结果 + 教训。
// 被 Reason 环节在推理卡壳时调用（find_similar 找相似经验）；
// 被 Feedback 环节在发现新教训时调用（add_experience 记录新经验）。
//
// 相似度算法：Jaccard Similarity (tags 的交集 / 并集)
// ============================================================================

use std::collections::{HashMap, HashSet};

use mesba_core::{Block, BlockContent, BlockId};

/// ============================================================================
/// ExperienceEntry —— 情景记忆里的一条经验
/// ============================================================================
#[derive(Debug, Clone)]
pub struct ExperienceEntry {
    /// Experience 区块 BlockId（在 network 里的真实区块）
    pub block_id: BlockId,
    /// 问题情境（原样复制 Experience.situation）
    pub situation: String,
    /// 尝试的方案/步骤
    pub attempt_summary: String,
    /// 错误类型（可为空 = 成功经验）
    pub error_type: String,
    /// 正确结果（字符串形式）
    pub correct_result: String,
    /// 教训标注（如 "处理完每一位必须检查进位"）
    pub lesson: String,
    /// 预防性区块 BlockId
    pub preventive_block: Option<BlockId>,
    /// 该经验的 tags 集合（从 situation + lesson + error_type 自动提取，用于相似度搜索）
    pub tags: HashSet<String>,
    /// 被访问次数（越高 = 越常用的经验）
    pub access_count: u32,
    /// 创建时间戳（毫秒）
    pub created_at: u64,
}

/// ============================================================================
/// EpisodicMemory —— 主体（L2 长期记忆子系统）
/// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct EpisodicMemory {
    /// 经验条目表：BlockId → ExperienceEntry
    pub experiences: HashMap<BlockId, ExperienceEntry>,
    /// 总访问计数（用于计算经验热度）
    total_accesses: u64,
}

impl EpisodicMemory {
    pub fn new() -> Self {
        Self::default()
    }

    // ========================================================================
    // 从 Block 迭代器重建索引（不依赖 mesba_network，由调用方传入）
    // ========================================================================
    /// 接受任何能迭代 &Block 的类型（MeshBlockNetwork::blocks() / find_by_type() 等）
    /// mesba_runtime 初始化或从存储加载时调用
    pub fn rebuild_from_blocks<'a, I>(&mut self, blocks: I)
    where
        I: IntoIterator<Item = &'a Block>,
    {
        self.experiences.clear();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        for block in blocks {
            if block.block_type == mesba_core::BlockType::Experience {
                let entry = Self::entry_from_block(block, now);
                self.experiences.insert(entry.block_id.clone(), entry);
            }
        }
    }

    // ========================================================================
    // add_experience_builder —— 接受完整 Entry 结构，避免参数过多
    // ========================================================================
    pub fn add_experience_entry(&mut self, entry: ExperienceEntry) {
        self.experiences.insert(entry.block_id.clone(), entry);
    }

    // ========================================================================
    // make_entry —— 工厂方法，从 BlockId + 字段构造 ExperienceEntry
    // ========================================================================
    pub fn make_entry(
        block_id: BlockId,
        situation: String,
        attempt_summary: String,
        error_type: String,
        correct_result: String,
        lesson: String,
        preventive_block: Option<BlockId>,
    ) -> ExperienceEntry {
        let tags = Self::extract_tags(&situation, &lesson, &error_type);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        ExperienceEntry {
            block_id,
            situation,
            attempt_summary,
            error_type,
            correct_result,
            lesson,
            preventive_block,
            tags,
            access_count: 0,
            created_at: now,
        }
    }

    // ========================================================================
    // find_similar —— 按 Jaccard 相似度搜索 top-k 相似经验
    // ========================================================================
    /// 输入：当前情境 tags（从 SituationModel.known_vars 或 classification 提取）
    /// 输出：按相似度降序排列的 (BlockId, jaccard_score 0~255) 列表
    pub fn find_similar(
        &mut self,
        query_tags: &HashSet<String>,
        top_k: usize,
    ) -> Vec<(BlockId, u8)> {
        if self.experiences.is_empty() || query_tags.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(BlockId, f32)> = self
            .experiences
            .iter()
            .map(|(bid, entry)| {
                let sim = Self::jaccard(query_tags, &entry.tags);
                (bid.clone(), sim)
            })
            .collect();

        // 按 Jaccard 降序
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 取 top_k 并提升 access_count（LRU 热度）
        let mut result = Vec::with_capacity(top_k);
        for (bid, sim) in scored.into_iter().take(top_k) {
            if let Some(entry) = self.experiences.get_mut(&bid) {
                entry.access_count += 1;
                self.total_accesses += 1;
            }
            // 转成 0~255 整数（Jaccard 0.0~1.0 → u8 0~255）
            let score = (sim * 255.0) as u8;
            result.push((bid, score));
        }
        result
    }

    // ========================================================================
    // 从 SituationModel 变量快照提取 tags（Reason 环节推理卡壳时调用）
    // ========================================================================
    pub fn tags_from_var_map(var_map: &HashMap<String, i32>, classification: &str) -> HashSet<String> {
        let mut tags: HashSet<String> = HashSet::new();
        // classification 整体作为 tag（如 "math.arithmetic.addition"）
        if !classification.is_empty() {
            tags.insert(classification.to_string());
        }
        // 每个非零变量变成 "name=value" tag（如 "a=3"）
        for (k, &v) in var_map.iter() {
            if v != 0 {
                tags.insert(format!("{}={}", k, v));
            }
        }
        tags
    }

    // ========================================================================
    // 统计辅助
    // ========================================================================
    pub fn len(&self) -> usize {
        self.experiences.len()
    }
    pub fn is_empty(&self) -> bool {
        self.experiences.is_empty()
    }
    pub fn get(&self, block_id: &BlockId) -> Option<&ExperienceEntry> {
        self.experiences.get(block_id)
    }

    // ========================================================================
    // 内部工具函数
    // ========================================================================

    /// 从 Block 提取 ExperienceEntry（rebuild_from_network 用）
    fn entry_from_block(block: &Block, now: u64) -> ExperienceEntry {
        match &block.content {
            BlockContent::Experience {
                situation,
                attempt_summary,
                error_type,
                correct_result,
                lesson,
                preventive_block,
            } => {
                let tags = Self::extract_tags(situation, lesson, error_type);
                ExperienceEntry {
                    block_id: block.id.clone(),
                    situation: situation.clone(),
                    attempt_summary: attempt_summary.clone(),
                    error_type: error_type.clone(),
                    correct_result: correct_result.clone(),
                    lesson: lesson.clone(),
                    preventive_block: preventive_block.clone(),
                    tags,
                    access_count: 0,
                    created_at: now,
                }
            }
            _ => {
                // 兜底：非 Experience 类型不应该走到这里
                ExperienceEntry {
                    block_id: block.id.clone(),
                    situation: String::new(),
                    attempt_summary: String::new(),
                    error_type: String::new(),
                    correct_result: String::new(),
                    lesson: String::new(),
                    preventive_block: None,
                    tags: HashSet::new(),
                    access_count: 0,
                    created_at: now,
                }
            }
        }
    }

    /// 从 situation + lesson + error_type 字符串提取 tags（小写化、按空格+标点拆分、去重）
    fn extract_tags(situation: &str, lesson: &str, error_type: &str) -> HashSet<String> {
        let mut tags = HashSet::new();
        for src in &[situation, lesson, error_type] {
            for word in src.split(|c: char| c.is_whitespace() || ".,;:!?()[]{}\"".contains(c)) {
                let w = word.trim().to_lowercase();
                if w.len() >= 2 {
                    tags.insert(w);
                }
            }
        }
        // error_type 整体也作为 tag（如 "forgot_carry"）
        if !error_type.is_empty() {
            tags.insert(error_type.to_lowercase());
        }
        tags
    }

    /// Jaccard 相似度 = |A ∩ B| / |A ∪ B|
    fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
        if a.is_empty() && b.is_empty() {
            return 0.0;
        }
        let intersection = a.intersection(b).count();
        let union = a.union(b).count();
        if union == 0 {
            return 0.0;
        }
        intersection as f32 / union as f32
    }
}

// ============================================================================
// 单元测试 —— 覆盖 rebuild / add / find_similar Jaccard / tags_from_var_map
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{Block, BlockContent, BlockType};

    #[allow(dead_code)]
    fn make_exp_block(id: &str, situation: &str, lesson: &str) -> Block {
        Block::new(
            BlockId::from(id),
            BlockType::Experience,
            "test.domain".to_string(),
            1,
            BlockContent::Experience {
                situation: situation.to_string(),
                attempt_summary: "步骤1 步骤2".to_string(),
                error_type: "forgot_carry".to_string(),
                correct_result: "18".to_string(),
                lesson: lesson.to_string(),
                preventive_block: None,
            },
        )
    }

    #[test]
    fn test_extract_tags_basic() {
        let tags = EpisodicMemory::extract_tags(
            "忘记加进位导致结果错误",
            "处理完每一位必须检查进位",
            "forgot_carry",
        );
        assert!(tags.contains("forgot_carry"));
        // 中文分词：空格+标点拆分 → 2 字以上的词会被提取（中文按字符可能偏少，但 MVP 够用）
        assert!(!tags.is_empty()); // 至少有 forgot_carry
    }

    #[test]
    fn test_jaccard_identical() {
        let mut a = HashSet::new();
        a.insert("a".to_string());
        a.insert("b".to_string());
        let b = a.clone();
        assert!((EpisodicMemory::jaccard(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_disjoint() {
        let mut a = HashSet::new();
        a.insert("a".to_string());
        let mut b = HashSet::new();
        b.insert("b".to_string());
        assert!((EpisodicMemory::jaccard(&a, &b) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_partial() {
        let mut a = HashSet::new();
        a.insert("a".to_string());
        a.insert("b".to_string());
        let mut b = HashSet::new();
        b.insert("b".to_string());
        b.insert("c".to_string());
        // 交集 {b} = 1，井集 {a,b,c} = 3 → 1/3 = 0.333
        let sim = EpisodicMemory::jaccard(&a, &b);
        assert!((sim - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_add_and_find_similar() {
        let mut em = EpisodicMemory::new();
        let bid1 = BlockId::from("Exp.1");
        let bid2 = BlockId::from("Exp.2");

        em.add_experience_entry(EpisodicMemory::make_entry(
            bid1.clone(),
            "两位数加法忘记进位".to_string(),
            "步骤1 步骤2".to_string(),
            "forgot_carry".to_string(),
            "18".to_string(),
            "处理完每一位必须检查进位".to_string(),
            None,
        ));
        em.add_experience_entry(EpisodicMemory::make_entry(
            bid2.clone(),
            "三位数减法忘记借位".to_string(),
            "步骤1 步骤2".to_string(),
            "forgot_borrow".to_string(),
            "123".to_string(),
            "处理完每一位必须检查借位".to_string(),
            None,
        ));

        assert_eq!(em.len(), 2);

        // 查询："forgot_carry 加法" — 应该找到 Exp.1（相同 error_type）
        let mut query = HashSet::new();
        query.insert("forgot_carry".to_string());
        query.insert("add".to_string());
        let results = em.find_similar(&query, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, bid1);
    }

    #[test]
    fn test_find_similar_empty() {
        let mut em = EpisodicMemory::new();
        let query = HashSet::new();
        assert!(em.find_similar(&query, 5).is_empty());

        let mut tags = HashSet::new();
        tags.insert("x".to_string());
        // 空的 EpisodicMemory
        assert!(em.find_similar(&tags, 5).is_empty());
    }

    #[test]
    fn test_tags_from_var_map() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), 3);
        vars.insert("b".to_string(), 5);
        vars.insert("temp".to_string(), 0); // 零值跳过

        let tags = EpisodicMemory::tags_from_var_map(&vars, "math.addition");
        assert!(tags.contains("math.addition"));
        assert!(tags.contains("a=3"));
        assert!(tags.contains("b=5"));
        assert!(!tags.contains("temp=0"));
    }
}
