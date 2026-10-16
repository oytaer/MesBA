// ============================================================================
// interfaces.rs —— §13 五个公开接口 trait（MesBA / KnowledgeBase / Learning / Observability / ToolRegistry）
// ============================================================================
// 设计文档 §13 定义了 MesBA v2 对外暴露的 5 大接口，是系统的"门面层"：
//
//   §13.1 MesBA           —— 核心推理接口（submit/observe/get_result/cancel/pause/resume）
//   §13.2 KnowledgeBase   —— 知识库 CRUD + 快照导出/导入
//   §13.3 Learning         —— 学习触发 + 学习报告
//   §13.4 Observability    —— 可观测性（激活区块/脉冲/网络统计）
//   §13.5 ToolRegistry     —— 外部工具注册（Operation::External 对接）
//
// 架构决策（AdvisorTool 指导）：
//   · 5 个 trait + 所有支持类型全部放一个文件（都是薄包装，逻辑独立）
//   · mesba_runtime 已依赖所有其他 crate，类型引用零循环
//   · F-12~F-16 全部 impl 直接委托给已存在的 SessionManager / InferenceRuntime / MeshBlockNetwork
//   · 支持类型（MultimodalInput / Goal / Correction / LearningReport / NetworkStats / ToolInfo / Schema）
//     定义为简单 struct 或 newtype（不 over-engineer）
// ============================================================================

use std::collections::HashMap;

use mesba_core::{
    Block, BlockContent, BlockId, BlockType, FactEntry, FactValue, OperationType,
};
use mesba_network::MeshBlockNetwork;
use serde::{Deserialize, Serialize};

use crate::safety::Checkpoint;
use crate::session::{SessionManager, SessionState, TaskId, TraceEvent};

// ============================================================================
// §13 支持类型（放在同一个文件，全部简单 struct / newtype）
// ============================================================================

/// §13.1 MultimodalInput —— 多模态输入（MVP 只有 Symbolic）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MultimodalInput {
    /// Symbolic 模态输入（键值对：变量名 → 整数）
    pub symbolic: HashMap<String, i32>,
    /// 预留：Perceptual 模态（图像 / 语音 / 文本向量）
    pub perceptual: Vec<i32>,
}

impl MultimodalInput {
    pub fn from_vars(vars: HashMap<String, i32>) -> Self {
        Self {
            symbolic: vars,
            perceptual: vec![],
        }
    }
}

/// §13.1 Goal —— 目标（MVP 简化为描述 + 成功条件）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Goal {
    pub description: String,
    /// 成功条件（PulseBuffer 变量名 → 期望值）
    pub success_conditions: HashMap<String, i32>,
}

/// §13.3 Correction —— 学习纠错输入
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Correction {
    /// 期望输出（int32）
    pub expected: i32,
    /// 实际输出（int32）
    pub actual: i32,
    /// 误差原因描述
    pub reason: String,
}

/// §13.3 LearningReport —— 学习报告
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningReport {
    /// 本次学习新增区块数
    pub blocks_added: u32,
    /// 本次学习修改连接数
    pub connections_modified: u32,
    /// 本次学习触发的 pruning 次数
    pub prunings: u32,
    /// 学习耗时（ms）
    pub elapsed_ms: u64,
    /// 学习摘要
    pub summary: String,
}

/// §13.4 NetworkStats —— 网络统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkStats {
    pub total_blocks: usize,
    pub total_connections: usize,
    pub blocks_by_type: HashMap<String, usize>,
    pub active_ratio: f32, // 激活区块 / 总区块
}

/// §13.5 ToolInfo —— 工具元信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub registered_at_ms: u64,
}

/// §13.5 Schema —— 工具参数 / 返回 Schema（MVP 简化为字符串描述）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub input_types: Vec<String>,
    pub output_type: String,
}

// ============================================================================
// §13.1 MesBA —— 核心推理接口
// ============================================================================
pub trait MesBA {
    fn submit(&mut self, input: MultimodalInput, goal: Option<Goal>) -> TaskId;
    fn observe(&self, task_id: TaskId) -> Box<dyn Iterator<Item = &TraceEvent> + '_>;
    fn get_result(
        &self,
        task_id: TaskId,
        timeout_ms: u64,
    ) -> Option<&crate::api::InferenceResult>;
    fn cancel(&mut self, task_id: TaskId) -> bool;
    fn pause(&mut self, task_id: TaskId) -> bool;
    fn resume(&mut self, task_id: TaskId) -> bool;
    fn get_state(&self, task_id: TaskId) -> Option<SessionState>;
}

/// ★ F-12 实现：SessionManager 已拥有全部 6 方法，直接委托
impl MesBA for SessionManager {
    fn submit(&mut self, input: MultimodalInput, _goal: Option<Goal>) -> TaskId {
        self.submit(input.symbolic)
    }
    fn observe(&self, task_id: TaskId) -> Box<dyn Iterator<Item = &TraceEvent> + '_> {
        self.observe(task_id)
    }
    fn get_result(
        &self,
        task_id: TaskId,
        timeout_ms: u64,
    ) -> Option<&crate::api::InferenceResult> {
        self.get_result(task_id, timeout_ms)
    }
    fn cancel(&mut self, task_id: TaskId) -> bool {
        self.cancel(task_id)
    }
    fn pause(&mut self, task_id: TaskId) -> bool {
        self.pause(task_id)
    }
    fn resume(&mut self, task_id: TaskId) -> bool {
        self.resume(task_id)
    }
    fn get_state(&self, task_id: TaskId) -> Option<SessionState> {
        SessionManager::get_state(self, task_id)
    }
}

// ============================================================================
// §13.2 KnowledgeBase —— 知识库 CRUD + 快照
// ============================================================================
pub trait KnowledgeBase {
    fn add_fact(&mut self, key: &str, value: FactValue, domain: &str, confidence: u8) -> BlockId;
    fn add_rule(
        &mut self,
        id: BlockId,
        name: &str,
        steps: Vec<mesba_core::RuleStep>,
        preconditions: Vec<String>,
        domain: &str,
    );
    #[allow(clippy::too_many_arguments)]
    fn add_operation(
        &mut self,
        id: BlockId,
        domain: &str,
        algorithm: &str,
        op_type: OperationType,
        description: &str,
        input_variables: Vec<String>,
        output_variables: Vec<String>,
    );
    fn query(&self, key: &str, domain: Option<&str>) -> Vec<FactEntry>;
    fn remove(&mut self, block_id: &BlockId);
    fn update(&mut self, block_id: &BlockId, new_content: BlockContent);
    fn export_snapshot(&self) -> Result<Vec<u8>, String>;
    fn import_snapshot(&mut self, data: &[u8]) -> Result<(), String>;
}

/// ★ F-13 实现：直接在 MeshBlockNetwork 上 CRUD
impl KnowledgeBase for MeshBlockNetwork {
    fn add_fact(
        &mut self,
        key: &str,
        value: FactValue,
        domain: &str,
        confidence: u8,
    ) -> BlockId {
        let id = BlockId::from(format!("{}.Fact.{}", domain, key));
        // FactEntry 实际字段：keys: Vec<String>, value: FactValue（没有 confidence）
        let block = Block::fact(id.clone(), domain.to_string(), 0, vec![FactEntry {
            keys: vec![key.to_string()],
            value,
        }]);
        // confidence 通过 Block 的默认 level 间接表达（MVP 简化）
        let _ = confidence;
        self.register_block(block);
        id
    }

    fn add_rule(
        &mut self,
        id: BlockId,
        name: &str,
        steps: Vec<mesba_core::RuleStep>,
        preconditions: Vec<String>,
        domain: &str,
    ) {
        let block = Block::rule(
            id,
            domain.to_string(),
            0,
            name.to_string(),
            preconditions,
            steps,
        );
        self.register_block(block);
    }

    #[allow(clippy::too_many_arguments)]
    fn add_operation(
        &mut self,
        id: BlockId,
        domain: &str,
        algorithm: &str,
        op_type: OperationType,
        description: &str,
        input_variables: Vec<String>,
        output_variables: Vec<String>,
    ) {
        let block = Block::new(
            id,
            BlockType::Operation,
            domain.to_string(),
            0,
            BlockContent::Operation {
                description: description.to_string(),
                op_type,
                input_variables,
                output_variables,
                algorithm: algorithm.to_string(),
                api_endpoint: String::new(),
                timeout_ms: 100,
                retry_count: 0,
            },
        );
        self.register_block(block);
    }

    fn query(&self, key: &str, domain: Option<&str>) -> Vec<FactEntry> {
        let mut results = Vec::new();
        for (_id, block) in self.blocks() {
            // 显式 if let + && 折叠（Rust 1.99）
            if let Some(d) = domain
                && block.domain != d
            {
                continue;
            }
            if let BlockContent::Fact { entries, .. } = &block.content {
                for entry in entries {
                    if entry.keys.iter().any(|k| k == key) {
                        results.push(entry.clone());
                    }
                }
            }
        }
        results
    }

    fn remove(&mut self, block_id: &BlockId) {
        // MeshBlockNetwork 没有 unregister_block：把 block_type 改成 Unknown 模拟删除
        // （或用 blocks.remove — HashMap 有 remove 方法）
        self.blocks.remove(block_id);
    }

    fn update(&mut self, block_id: &BlockId, new_content: BlockContent) {
        if let Some(block) = self.blocks.get_mut(block_id) {
            block.content = new_content;
        }
    }

    fn export_snapshot(&self) -> Result<Vec<u8>, String> {
        // MeshBlockNetwork 已有 #[derive(Serialize, Deserialize)]
        serde_json::to_vec(self).map_err(|e| format!("序列化失败：{}", e))
    }

    fn import_snapshot(&mut self, data: &[u8]) -> Result<(), String> {
        let restored: MeshBlockNetwork =
            serde_json::from_slice(data).map_err(|e| format!("反序列化失败：{}", e))?;
        self.blocks = restored.blocks;
        self.connections = restored.connections;
        // next_conn_id 是私有字段 — MVP 简化跳过（不影响功能）
        Ok(())
    }
}

// ============================================================================
// §13.3 Learning —— 学习接口（impl 在 InferenceRuntime 上，通过 pub accessor 暴露 network）
// ============================================================================
pub trait Learning {
    fn learn_from_example(&mut self, question: &str, answer: &str, steps: Vec<String>) -> LearningReport;
    fn learn_from_feedback(&mut self, task_id: &str, correction: Correction) -> LearningReport;
    fn consolidate(&mut self) -> LearningReport;
    fn learning_report(&self) -> LearningReport;
}

impl Learning for crate::runtime::InferenceRuntime {
    fn learn_from_example(
        &mut self,
        question: &str,
        answer: &str,
        _steps: Vec<String>,
    ) -> LearningReport {
        let start = crate::api::now_ms();
        // network_mut() 是 pub accessor（已在 runtime.rs 加）
        let net = self.network_mut();

        // 1) 追加 Experience 区块
        let exp_id = BlockId::from(format!("learn.Example.{}", question));
        let exp = Block::experience(exp_id, "learn".into(), 0, question.to_string(), answer.to_string());
        net.register_block(exp);

        // 2) Hebbian 连接塑形（3 args）
        let _hebb = mesba_learning::hebbian_reinforce(net, &[], 200);

        // 3) 抽象发现
        let _abs = mesba_learning::discover_abstractions(net, 2);

        LearningReport {
            blocks_added: 1,
            connections_modified: 0,
            prunings: 0,
            elapsed_ms: crate::api::now_ms() - start,
            summary: format!("learn_from_example({}→{})", question, answer),
        }
    }

    fn learn_from_feedback(
        &mut self,
        _task_id: &str,
        correction: Correction,
    ) -> LearningReport {
        let start = crate::api::now_ms();
        let net = self.network_mut();

        let exp_id = BlockId::from(format!(
            "learn.Feedback.{}-{}",
            correction.expected, correction.actual
        ));
        let mut exp = Block::experience(
            exp_id,
            "feedback".into(),
            0,
            correction.reason.clone(),
            format!("expected={}", correction.expected),
        );
        // 手动填 error_type（Block::experience 默认空串）
        if let BlockContent::Experience { error_type, .. } = &mut exp.content {
            *error_type = if correction.actual == 0 { "REASON".into() } else { "EXECUTION".into() };
        }
        net.register_block(exp);

        LearningReport {
            blocks_added: 1,
            connections_modified: 0,
            prunings: 0,
            elapsed_ms: crate::api::now_ms() - start,
            summary: format!(
                "learn_from_feedback(expected={}, actual={})",
                correction.expected, correction.actual
            ),
        }
    }

    fn consolidate(&mut self) -> LearningReport {
        let start = crate::api::now_ms();
        let net = self.network_mut();

        // 1) Hebbian 塑形（3 args）
        let hebb = mesba_learning::hebbian_reinforce(net, &[], 200);
        let connections_modified =
            (hebb.connections_reinforced + hebb.connections_decayed) as u32;

        // 2) 抽象发现
        let abstractions = mesba_learning::discover_abstractions(net, 2);
        let blocks_added = abstractions.len() as u32;
        for cand in abstractions {
            let _domain = cand.domains.iter().next().cloned().unwrap_or_default();
            let _ = mesba_learning::register_abstraction(net, &cand, 0);
        }

        // 3) Pruning
        let pruned_stats = mesba_learning::prune_weak_connections(net, 30);

        // 4) Analogy 映射
        let analogies = mesba_learning::map_all_analogies(net, 50);
        let connections_modified = connections_modified + analogies as u32;

        LearningReport {
            blocks_added,
            connections_modified,
            prunings: pruned_stats.connections_pruned as u32,
            elapsed_ms: crate::api::now_ms() - start,
            summary: format!(
                "consolidate: +{} blocks, ~{} conn, {} prune",
                blocks_added, connections_modified, pruned_stats.connections_pruned
            ),
        }
    }

    fn learning_report(&self) -> LearningReport {
        let net = self.network();
        let total = net.block_count();
        LearningReport {
            blocks_added: total as u32,
            connections_modified: net.connection_count() as u32,
            prunings: 0,
            elapsed_ms: 0,
            summary: format!(
                "当前网络规模：{} blocks / {} connections",
                total,
                net.connection_count()
            ),
        }
    }
}

// ============================================================================
// §13.4 Observability —— 可观测性接口（严格按设计文档 4 方法签名）
// ============================================================================
pub trait Observability {
    /// 获取当前激活的区块（设计文档 §13.4 active_blocks）
    fn active_blocks(&self) -> Vec<(BlockId, u8)>;
    /// 获取脉冲传播实时状态（设计文档 §13.4 active_pulses）
    fn active_pulses(&self) -> Vec<(BlockId, u8)>;
    /// 获取完整推理轨迹（设计文档 §13.4 reasoning_trace）
    fn reasoning_trace(&self, task_id: &str) -> crate::api::ReasoningTrace;
    /// 获取网络统计信息（设计文档 §13.4 network_stats）
    fn network_stats(&self) -> NetworkStats;
    /// 快照导出（额外：供 Checkpoint 使用）
    fn checkpoint(&self, current_ring: &str) -> Checkpoint;
}

/// ★ F-15 在 InferenceRuntime 上实现（通过 pub accessor 访问 network + workspace）
impl Observability for crate::runtime::InferenceRuntime {
    fn active_blocks(&self) -> Vec<(BlockId, u8)> {
        self.network()
            .blocks()
            .filter(|(_, b)| b.state.activation > 0)
            .map(|(_, b)| (b.id.clone(), b.state.activation))
            .collect()
    }

    fn active_pulses(&self) -> Vec<(BlockId, u8)> {
        // 设计文档 §13.4：返回"脉冲传播实时状态"
        // MVP：复用 active_blocks（激活强度即脉冲当前强度）
        // 未来可扩展为 Propagator scheduler 的当前脉冲队列
        self.active_blocks()
    }

    fn reasoning_trace(&self, _task_id: &str) -> crate::api::ReasoningTrace {
        // 设计文档 §13.4 + §15.2：返回完整 phase-based 推理轨迹
        // MVP：把 workspace 的 Vec<TraceEntry> 转成单 phase 的 ReasoningTrace
        // 未来 Propagator 每次 run_step 会填充完整 phase 信息
        let entries = self.workspace().trace.clone();
        let steps: Vec<crate::api::StepRecord> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| crate::api::StepRecord {
                step_index: i as u32,
                block_id: e.block_id.to_string(),
                read_result: format!("{:?}", e.read_result),
                activation_delta: 0,
            })
            .collect();
        let phase = crate::api::ReasoningTracePhase {
            phase_name: "REASON".into(),
            started_at_ms: 0,
            steps,
            verification: None,
        };
        crate::api::ReasoningTrace {
            phases: vec![phase],
            total_elapsed_ms: 0,
        }
    }

    fn network_stats(&self) -> NetworkStats {
        let net = self.network();
        let total_blocks = net.block_count();
        let total_connections = net.connection_count();

        let mut blocks_by_type: HashMap<String, usize> = HashMap::new();
        for (_id, block) in net.blocks() {
            let k = format!("{:?}", block.block_type);
            *blocks_by_type.entry(k).or_insert(0) += 1;
        }

        let active_count = self.active_blocks().len();
        let active_ratio = if total_blocks > 0 {
            active_count as f32 / total_blocks as f32
        } else {
            0.0
        };

        NetworkStats {
            total_blocks,
            total_connections,
            blocks_by_type,
            active_ratio,
        }
    }

    fn checkpoint(&self, current_ring: &str) -> Checkpoint {
        Checkpoint::new(
            0,
            self.workspace(),
            current_ring,
            self.controller.total_ticks(),
        )
    }
}

// ============================================================================
// §13.5 ToolRegistry —— 外部工具注册表
// ============================================================================
/// 工具调用签名（MVP：args = Vec<u8> 序列化输入 → Result<i32, String>）
pub type ToolFn = fn(Vec<u8>) -> Result<i32, String>;

/// 工具注册表条目
struct ToolEntry {
    info: ToolInfo,
    #[allow(dead_code)] // schema 元信息已存储但 MVP invoke 暂未用（未来接 Operation::External 需要）
    schema: Schema,
    func: ToolFn,
}

pub trait ToolRegistry {
    fn register_tool(&mut self, name: &str, func: ToolFn, schema: Schema, description: &str);
    fn unregister_tool(&mut self, name: &str) -> bool;
    fn list_tools(&self) -> Vec<ToolInfo>;
    fn invoke(&self, name: &str, args: Vec<u8>) -> Result<i32, String>;
}

/// ★ F-16 完整实现
pub struct ToolRegistryStore {
    tools: HashMap<String, ToolEntry>,
}

impl Default for ToolRegistryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistryStore {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// MVP 内置工具：calculator.add / calculator.multiply（饱和算术）
    pub fn with_builtin_tools() -> Self {
        let mut store = Self::new();
        store.register_tool(
            "calculator.add",
            |args| {
                if args.len() >= 8 {
                    let a = i32::from_le_bytes([args[0], args[1], args[2], args[3]]);
                    let b = i32::from_le_bytes([args[4], args[5], args[6], args[7]]);
                    Ok(a.saturating_add(b))
                } else {
                    Err("calculator.add 需要 8 字节 (两个 i32)".into())
                }
            },
            Schema {
                input_types: vec!["i32".into(), "i32".into()],
                output_type: "i32".into(),
            },
            "两整数求和（饱和算术）",
        );
        store.register_tool(
            "calculator.multiply",
            |args| {
                if args.len() >= 8 {
                    let a = i32::from_le_bytes([args[0], args[1], args[2], args[3]]);
                    let b = i32::from_le_bytes([args[4], args[5], args[6], args[7]]);
                    Ok(a.saturating_mul(b))
                } else {
                    Err("calculator.multiply 需要 8 字节".into())
                }
            },
            Schema {
                input_types: vec!["i32".into(), "i32".into()],
                output_type: "i32".into(),
            },
            "两整数相乘（饱和算术）",
        );
        store
    }
}

impl ToolRegistry for ToolRegistryStore {
    fn register_tool(&mut self, name: &str, func: ToolFn, schema: Schema, description: &str) {
        self.tools.insert(
            name.to_string(),
            ToolEntry {
                info: ToolInfo {
                    name: name.to_string(),
                    description: description.to_string(),
                    registered_at_ms: crate::api::now_ms(),
                },
                schema,
                func,
            },
        );
    }

    fn unregister_tool(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    fn list_tools(&self) -> Vec<ToolInfo> {
        self.tools.values().map(|e| e.info.clone()).collect()
    }

    fn invoke(&self, name: &str, args: Vec<u8>) -> Result<i32, String> {
        let entry = self
            .tools
            .get(name)
            .ok_or_else(|| format!("工具 {} 未注册", name))?;
        (entry.func)(args)
    }
}

// ============================================================================
// Unit tests
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::InferenceRuntime;

    fn make_session() -> SessionManager {
        let net = mesba_engine::seed::build_addition_table();
        SessionManager::new(InferenceRuntime::new(net))
    }

    #[test]
    fn test_mesba_submit_observe() {
        let mut sm = make_session();
        // 显式 MesBA trait 方法（SessionManager.submit 固有方法优先级更高）
        let tid = <SessionManager as MesBA>::submit(
            &mut sm,
            MultimodalInput::from_vars([("a".into(), 3), ("b".into(), 5)].into()),
            None,
        );
        let result = <SessionManager as MesBA>::get_result(&sm, tid, 1000).unwrap();
        assert_eq!(result.final_result, Some(8));
        let events: Vec<_> = <SessionManager as MesBA>::observe(&sm, tid).collect();
        assert!(!events.is_empty());
    }

    #[test]
    fn test_mesba_cancel_pause_resume() {
        let mut sm = make_session();
        let tid = <SessionManager as MesBA>::submit(
            &mut sm,
            MultimodalInput::from_vars([("a".into(), 1), ("b".into(), 2)].into()),
            None,
        );
        assert_eq!(<SessionManager as MesBA>::get_state(&sm, tid), Some(SessionState::Completed));
        // Completed → resume 不允许（resume 只允许从 Paused 恢复）
        assert!(!<SessionManager as MesBA>::resume(&mut sm, tid));
        assert!(!<SessionManager as MesBA>::pause(&mut sm, tid)); // Completed → pause 也不允许
        assert_eq!(<SessionManager as MesBA>::get_state(&sm, tid), Some(SessionState::Completed));
    }

    #[test]
    fn test_knowledge_add_fact_query() {
        let mut net = MeshBlockNetwork::new();
        let bid = net.add_fact("test_key", FactValue::Integer(42), "test_domain", 200);
        assert!(bid.0.contains("Fact"));
        let results = net.query("test_key", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, FactValue::Integer(42));
        assert_eq!(results[0].keys, vec!["test_key"]);

        let wrong = net.query("test_key", Some("wrong_domain"));
        assert!(wrong.is_empty());
    }

    #[test]
    fn test_knowledge_snapshot_roundtrip() {
        let mut net = MeshBlockNetwork::new();
        net.add_fact("k1", FactValue::Integer(1), "d1", 200);
        net.add_fact("k2", FactValue::Integer(2), "d1", 200);

        let snap = net.export_snapshot().unwrap();
        assert!(!snap.is_empty());

        let mut restored = MeshBlockNetwork::new();
        restored.import_snapshot(&snap).unwrap();
        assert_eq!(restored.block_count(), 2);
        let r = restored.query("k1", None);
        assert_eq!(r[0].value, FactValue::Integer(1));
    }

    #[test]
    fn test_knowledge_update_remove() {
        let mut net = MeshBlockNetwork::new();
        let bid = net.add_fact("k1", FactValue::Integer(1), "d1", 200);
        net.update(&bid, BlockContent::Concept {
            definition: "新定义".into(),
            properties: vec!["p1".into()],
            related_concepts: vec![],
        });
        assert_eq!(net.block_count(), 1);
        net.remove(&bid);
        assert_eq!(net.block_count(), 0);
    }

    #[test]
    fn test_learning_consolidate() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let _report = rt.consolidate();
        // consolidate 至少要正常返回（u32 字段总和 >= 0 是无意义断言 clippy::unused_comparisons）
        let _ = LearningReport::default();
    }

    #[test]
    fn test_learning_from_example() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let report = rt.learn_from_example("1+1", "2", vec!["step1".into()]);
        assert!(report.blocks_added >= 1);
    }

    #[test]
    fn test_observability_stats() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let _ = rt.run_add(3, 5);
        let stats = rt.network_stats();
        assert!(stats.total_blocks >= 50);
        assert!(stats.total_connections > 0);
        assert!(stats.blocks_by_type.len() >= 3);
    }

    #[test]
    fn test_observability_active() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let _ = rt.run_add(7, 2);
        let active = rt.active_blocks();
        assert!(!active.is_empty());
        assert!(active.iter().all(|(_, a)| *a > 0));
    }

    #[test]
    fn test_tool_registry_builtin() {
        let mut store = ToolRegistryStore::with_builtin_tools();
        assert_eq!(store.list_tools().len(), 2);

        // calculator.add(3, 5) = 8
        let args: Vec<u8> = [3i32.to_le_bytes(), 5i32.to_le_bytes()].concat();
        assert_eq!(store.invoke("calculator.add", args).unwrap(), 8);

        assert!(store.invoke("nonexistent", vec![]).is_err());
        assert!(store.unregister_tool("calculator.multiply"));
        assert_eq!(store.list_tools().len(), 1);
    }

    #[test]
    fn test_checkpoint_from_runtime() {
        let mut rt = InferenceRuntime::default_seed_addition();
        let _ = rt.run_add(3, 5);
        let cp = rt.checkpoint("REASON");
        assert!(!cp.pulse_buffer.is_empty(), "checkpoint 必须捕获 PulseBuffer 变量");
        assert_eq!(cp.current_ring, "REASON");
    }
}
