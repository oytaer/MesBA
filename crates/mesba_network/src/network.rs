// ============================================================================
// network.rs —— MeshBlockNetwork 网状区块网络核心
// ============================================================================
// 功能：MesBA 知识网络的集中管理器
// 设计原则（关键架构决策）：
//   1. Connection 不存储在 Block 内部 —— 由 MeshBlockNetwork 统一持有
//      避免连接重复，支持 5B 连接量级（700B 目标）
//   2. 双向邻接索引：outgoing[incoming]: HashMap<BlockId, Vec<ConnectionId>>
//      实现 O(1) 邻居查找，脉冲传播时直接拿到邻接表
//   3. 多维度查询索引：domain_index / type_index / tag_index
//      支持快速按领域/类型/标签批量查找区块
//   4. 为 mmap 存储预留接口 —— 索引和数据分离，可 page-map
//
// 对应设计文档：
//   - 网状区块架构 v2 §7.1 网状区块网络（MBN）
//   - 网状区块架构 v2 §7.2 邻接表拓扑结构
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use mesba_core::{
    Block, BlockId, BlockType, Connection, ConnectionId, ConnectionType,
};

// 导入同 crate 内的 topology 模块（度约束 / 拓扑规则校验）
use crate::topology;

/// =========================================================================
/// MeshBlockNetwork —— 网状区块网络（MBN）核心容器
/// =========================================================================
/// 作为 MesBA 整个架构的"知识主体"：存储全部区块和连接，
/// 并维护邻接索引以实现高效脉冲传播。
///
/// # 内存布局
/// ```text
/// MeshBlockNetwork
/// ├── blocks:       HashMap<BlockId, Block>         —— 全部区块注册表
/// ├── connections:  HashMap<ConnectionId, Connection> —— 全部连接注册表
/// ├── outgoing:     HashMap<BlockId, Vec<ConnectionId>> —— 出边索引（O(1)）
/// ├── incoming:     HashMap<BlockId, Vec<ConnectionId>> —— 入边索引（O(1)）
/// ├── domain_index: HashMap<String, Vec<BlockId>>      —— 域名索引
/// ├── type_index:   HashMap<BlockType, Vec<BlockId>>   —— 类型索引
/// └── tag_index:    HashMap<String, Vec<BlockId>>      —— 标签索引
/// ```
///
/// # 为什么 Connection 不存储在 Block 内部？
///   1. **5B 连接量级**：700B 目标有 500M 区块 / 5B 连接，每个 Block 持有连接的一半是 2.5B 重复对象
///   2. **O(1) 邻居查找**：脉冲传播时 Block.outgoing_connections() 必须扫描 Block.connections 过滤 source_id
///      → O(N) per lookup；使用索引直接拿 Vec<ConnectionId> → O(1)
///   3. **mmap 存储友好**：未来大模型存储时，connections 可以独立 page-map，Block 内容独立 page-map
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshBlockNetwork {
    /// 全部区块注册表（唯一所有权）
    pub blocks: HashMap<BlockId, Block>,

    /// 全部连接注册表（唯一所有权）
    pub connections: HashMap<ConnectionId, Connection>,

    /// 出边邻接索引：source_block_id → [conn_id1, conn_id2, ...]
    /// 脉冲传播时从这个索引直接拿到邻居列表
    outgoing: HashMap<BlockId, Vec<ConnectionId>>,

    /// 入边邻接索引：target_block_id → [conn_id1, conn_id2, ...]
    /// 用于"谁连接到了我"的反向查询
    incoming: HashMap<BlockId, Vec<ConnectionId>>,

    /// 域名索引："math.arithmetic" → [Fact.Add_1_1, Fact.Add_2_2, ...]
    domain_index: HashMap<String, Vec<BlockId>>,

    /// 类型索引：BlockType::Fact → [Fact.Add_1_1, ...]
    type_index: HashMap<BlockType, Vec<BlockId>>,

    /// 标签索引："addition" → [Fact.Add_1_1, Rule.AdditionStrategy, ...]
    tag_index: HashMap<String, Vec<BlockId>>,

    /// 下一个 ConnectionId 计数器（自增）
    next_conn_id: u64,
}

impl MeshBlockNetwork {
    // -----------------------------------------------------------------------
    // 构造函数
    // -----------------------------------------------------------------------

    /// 创建空网络
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            connections: HashMap::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            domain_index: HashMap::new(),
            type_index: HashMap::new(),
            tag_index: HashMap::new(),
            next_conn_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // 区块管理
    // -----------------------------------------------------------------------

    /// 注册一个新区块（会同时更新 domain/type/tag 索引）
    pub fn register_block(&mut self, block: Block) {
        let id = block.id.clone();
        let domain = block.domain.clone();
        let block_type = block.block_type;
        let tags = block.tags.clone();

        // 区块注册表
        self.blocks.insert(id.clone(), block);

        // 域名索引
        self.domain_index
            .entry(domain)
            .or_default()
            .push(id.clone());

        // 类型索引
        self.type_index
            .entry(block_type)
            .or_default()
            .push(id.clone());

        // 标签索引
        for tag in tags {
            self.tag_index
                .entry(tag)
                .or_default()
                .push(id.clone());
        }
    }

    /// 按 ID 获取区块引用
    pub fn get_block(&self, id: &BlockId) -> Option<&Block> {
        self.blocks.get(id)
    }

    /// 按 ID 获取区块可变引用
    pub fn get_block_mut(&mut self, id: &BlockId) -> Option<&mut Block> {
        self.blocks.get_mut(id)
    }

    /// 获取所有区块的可变引用迭代器（用于全局衰减/不应期 countdown）
    pub fn blocks_mut(&mut self) -> impl Iterator<Item = (&BlockId, &mut Block)> {
        self.blocks.iter_mut()
    }

    /// 获取所有区块引用（只读遍历）
    pub fn blocks(&self) -> impl Iterator<Item = (&BlockId, &Block)> {
        // ★ 确定性排序：按 BlockId 字符串排序（避免 HashMap 随机种子导致的非确定性）
        let mut sorted: Vec<_> = self.blocks.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        sorted.into_iter()
    }

    /// 获取所有连接引用（只读遍历，用于测试/调试）
    pub fn connections(&self) -> impl Iterator<Item = (&ConnectionId, &Connection)> {
        // ★ 确定性排序：按 ConnectionId 数值排序（避免 HashMap 随机种子）
        let mut sorted: Vec<_> = self.connections.iter().collect();
        sorted.sort_by_key(|(cid, _)| cid.0);
        sorted.into_iter()
    }

    /// 区块是否存在
    pub fn contains_block(&self, id: &BlockId) -> bool {
        self.blocks.contains_key(id)
    }

    // -----------------------------------------------------------------------
    // 连接管理
    // -----------------------------------------------------------------------

    /// 创建并注册一条新连接（自动分配 ConnectionId + 更新双向邻接索引）
    ///
    /// 设计文档 §5.3 度约束：创建后会对 source/target 分别检查是否超出度数上限
    /// （原子块 ≤ 32，领域块 ≤ 256），超限则 tracing::warn! 记录但不阻断连接 ——
    /// 为现有种子网络留出过渡期（MVP 阶段仅告警，未来可升级为 hard block）。
    pub fn connect(
        &mut self,
        source_id: BlockId,
        target_id: BlockId,
        conn_type: ConnectionType,
    ) -> ConnectionId {
        let conn_id = ConnectionId::new(self.next_conn_id);
        self.next_conn_id += 1;

        let conn = Connection::new(conn_id, source_id.clone(), target_id.clone(), conn_type);

        // 连接注册表
        self.connections.insert(conn_id, conn);

        // 双向邻接索引
        self.outgoing
            .entry(source_id.clone())
            .or_default()
            .push(conn_id);
        self.incoming
            .entry(target_id.clone())
            .or_default()
            .push(conn_id);

        // === 拓扑度约束检查（§5.3 rule 6，MVP：仅告警不阻断） ===
        self.check_degree_advisory(&source_id, &target_id);

        conn_id
    }

    /// 创建并注册一条新连接（指定权重和衰减）
    /// 同样会执行度约束告警（见 connect() 注释）。
    pub fn connect_with_weight(
        &mut self,
        source_id: BlockId,
        target_id: BlockId,
        conn_type: ConnectionType,
        weight: u8,
        decay: u8,
    ) -> ConnectionId {
        let conn_id = ConnectionId::new(self.next_conn_id);
        self.next_conn_id += 1;

        let conn = Connection::with_weight_decay(
            conn_id,
            source_id.clone(),
            target_id.clone(),
            conn_type,
            weight,
            decay,
        );

        self.connections.insert(conn_id, conn);
        self.outgoing.entry(source_id.clone()).or_default().push(conn_id);
        self.incoming.entry(target_id.clone()).or_default().push(conn_id);

        // === 拓扑度约束检查（§5.3 rule 6，MVP：仅告警不阻断） ===
        self.check_degree_advisory(&source_id, &target_id);

        conn_id
    }

    /// 内部辅助：对 source/target 两个区块执行度约束检查，超限则 warn 日志。
    /// MVP 阶段仅告警（种子网络已有 105+ 连接超出原子块 32 的限制，硬阻断会破坏初始化）。
    fn check_degree_advisory(&self, source_id: &BlockId, target_id: &BlockId) {
        // 检查 source 区块的总度（出边 + 入边索引长度之和）
        if let Some(src_block) = self.blocks.get(source_id) {
            let src_degree = self.outgoing.get(source_id).map(|v| v.len()).unwrap_or(0)
                + self.incoming.get(source_id).map(|v| v.len()).unwrap_or(0);
            if topology::is_degree_exceeded(src_block.block_type, src_degree) {
                let limit = match src_block.block_type {
                    BlockType::Fact
                    | BlockType::Judgment
                    | BlockType::Perceptual
                    | BlockType::Operation
                    | BlockType::Experience => topology::ATOMIC_MAX_DEGREE,
                    _ => topology::DOMAIN_MAX_DEGREE,
                };
                tracing::warn!(
                    "拓扑度约束告警 §5.3: source 区块 {} (type={:?}) 总度={} 已超出上限={}",
                    source_id, src_block.block_type, src_degree, limit
                );
            }
        }

        // 检查 target 区块的总度
        if let Some(tgt_block) = self.blocks.get(target_id) {
            let tgt_degree = self.outgoing.get(target_id).map(|v| v.len()).unwrap_or(0)
                + self.incoming.get(target_id).map(|v| v.len()).unwrap_or(0);
            if topology::is_degree_exceeded(tgt_block.block_type, tgt_degree) {
                let limit = match tgt_block.block_type {
                    BlockType::Fact
                    | BlockType::Judgment
                    | BlockType::Perceptual
                    | BlockType::Operation
                    | BlockType::Experience => topology::ATOMIC_MAX_DEGREE,
                    _ => topology::DOMAIN_MAX_DEGREE,
                };
                tracing::warn!(
                    "拓扑度约束告警 §5.3: target 区块 {} (type={:?}) 总度={} 已超出上限={}",
                    target_id, tgt_block.block_type, tgt_degree, limit
                );
            }
        }
    }

    /// 按 ID 获取连接引用
    pub fn get_connection(&self, id: &ConnectionId) -> Option<&Connection> {
        self.connections.get(id)
    }

    /// 按 ID 获取连接可变引用（用于 Hebbian 更新/门控操作）
    pub fn get_connection_mut(&mut self, id: &ConnectionId) -> Option<&mut Connection> {
        self.connections.get_mut(id)
    }

    // -----------------------------------------------------------------------
    // O(1) 邻接查询（脉冲传播核心）
    // -----------------------------------------------------------------------

    /// 获取指定区块的所有出边连接（直接索引查找，O(1)）
    /// 返回开放连接的 &Connection 迭代器
    /// 脉冲传播时用这个来转发新脉冲
    /// ★ 确定性排序：按 ConnectionId 数值排序（Propagator Pass 2b 遍历顺序必须确定）
    pub fn outgoing_connections(&self, block_id: &BlockId) -> Vec<&Connection> {
        let mut ids: Vec<ConnectionId> = self
            .outgoing
            .get(block_id)
            .cloned()
            .unwrap_or_default();
        ids.sort_by_key(|cid| cid.0);

        let mut result: Vec<_> = self
            .connections
            .iter()
            .filter(move |(cid, c)| ids.contains(cid) && c.is_open())
            .map(|(_, c)| c)
            .collect();
        result.sort_by_key(|c| c.id.0);
        result
    }

    /// 获取指定区块的所有入边连接
    pub fn incoming_connections(&self, block_id: &BlockId) -> Vec<&Connection> {
        let mut ids: Vec<ConnectionId> = self
            .incoming
            .get(block_id)
            .cloned()
            .unwrap_or_default();
        ids.sort_by_key(|cid| cid.0);

        let mut result: Vec<_> = self
            .connections
            .iter()
            .filter(move |(cid, _)| ids.contains(cid))
            .map(|(_, c)| c)
            .collect();
        result.sort_by_key(|c| c.id.0);
        result
    }

    /// 获取指定区块的所有出边邻接目标 BlockId 列表
    /// 脉冲传播时用这个来构造新脉冲的 target
    pub fn outgoing_targets(&self, block_id: &BlockId) -> Vec<(BlockId, u8)> {
        self.outgoing_connections(block_id)
            .into_iter()
            .map(|c| (c.target_id.clone(), c.weight))
            .collect()
    }

    /// 获取指定区块的邻居（通过出边），支持类型过滤
    /// 用于"查找从 A 出发的所有 Concept 类型邻居"等场景
    pub fn outgoing_neighbors_by_type(
        &self,
        block_id: &BlockId,
        target_type: BlockType,
    ) -> Vec<(BlockId, ConnectionId)> {
        self.outgoing
            .get(block_id)
            .iter()
            .flat_map(|conn_ids| conn_ids.iter())
            .filter_map(|cid| {
                let conn = self.connections.get(cid)?;
                if !conn.is_open() {
                    return None;
                }
                let target = self.blocks.get(&conn.target_id)?;
                if target.block_type == target_type {
                    Some((conn.target_id.clone(), *cid))
                } else {
                    None
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // 多维度索引查询
    // -----------------------------------------------------------------------

    /// 按域名查找所有区块
    pub fn find_by_domain(&self, domain: &str) -> Vec<&Block> {
        self.domain_index
            .get(domain)
            .iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.blocks.get(id)))
            .collect()
    }

    /// 按类型查找所有区块
    pub fn find_by_type(&self, block_type: BlockType) -> Vec<&Block> {
        self.type_index
            .get(&block_type)
            .iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.blocks.get(id)))
            .collect()
    }

    /// 按标签查找所有区块
    pub fn find_by_tag(&self, tag: &str) -> Vec<&Block> {
        self.tag_index
            .get(tag)
            .iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.blocks.get(id)))
            .collect()
    }

    /// 按域名前缀查找（如 "Math.Arithmetic" → 包含 "Math.Arithmetic.*"）
    pub fn find_by_domain_prefix(&self, prefix: &str) -> Vec<&Block> {
        self.blocks
            .values()
            .filter(|b| b.domain.starts_with(prefix))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 统计信息
    // -----------------------------------------------------------------------

    /// 区块总数
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// 连接总数
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 指定区块的出边数量（开放）
    pub fn outgoing_degree(&self, block_id: &BlockId) -> usize {
        self.outgoing_connections(block_id).len()
    }

    /// 指定区块的入边数量
    pub fn incoming_degree(&self, block_id: &BlockId) -> usize {
        self.incoming_connections(block_id).len()
    }

    /// 按类型统计区块数
    pub fn block_type_counts(&self) -> HashMap<BlockType, usize> {
        let mut counts = HashMap::new();
        for bt in BlockType::all() {
            counts.insert(bt, self.type_index.get(&bt).map(|v| v.len()).unwrap_or(0));
        }
        counts
    }

    /// 重置所有区块的激活状态（推理开始前调用）
    pub fn reset_all_states(&mut self) {
        for block in self.blocks.values_mut() {
            block.state.reset();
        }
    }

    /// 对所有区块执行一步自然衰减（每轮脉冲传播后调用）
    pub fn natural_decay_all(&mut self) {
        for block in self.blocks.values_mut() {
            block.state.natural_decay();
            block.state.tick_refractory();
        }
    }
}

impl Default for MeshBlockNetwork {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::{FactEntry, FactValue};

    /// 创建测试事实区块：id 作为 BlockId 和 domain（点分段）
    fn make_fact(id: &str, value: i32) -> Block {
        // id 如 "Math.Arithmetic.Fact.Add_1_1" → domain 取前 N-1 段
        let parts: Vec<&str> = id.split('.').collect();
        let domain = if parts.len() > 1 {
            parts[..parts.len() - 1].join(".")
        } else {
            "test".to_string()
        };
        Block::fact(
            BlockId::new(id),
            domain,
            0,
            vec![FactEntry {
                keys: vec![id.to_string()],
                value: FactValue::Integer(value),
            }],
        )
    }

    #[test]
    fn test_block_registry() {
        let mut net = MeshBlockNetwork::new();
        let block = make_fact("Test.Fact.One", 1);
        let id = block.id.clone();
        net.register_block(block);

        assert_eq!(net.block_count(), 1);
        assert!(net.contains_block(&id));
        assert_eq!(net.get_block(&id).unwrap().id.as_str(), "Test.Fact.One");
    }

    #[test]
    fn test_connection_registry() {
        let mut net = MeshBlockNetwork::new();
        net.register_block(make_fact("A", 0));
        net.register_block(make_fact("B", 0));
        net.register_block(make_fact("C", 0));

        // A → B (temporal), B → C (temporal), A → C (diagonal)
        net.connect(BlockId::new("A"), BlockId::new("B"), ConnectionType::Temporal);
        net.connect(BlockId::new("B"), BlockId::new("C"), ConnectionType::Temporal);
        net.connect(BlockId::new("A"), BlockId::new("C"), ConnectionType::Diagonal);

        assert_eq!(net.connection_count(), 3);

        // 检查双向邻接索引（Vec 顺序不保证，用 len 检查数量）
        assert_eq!(net.outgoing_degree(&BlockId::new("A")), 2);  // A→B + A→C
        assert_eq!(net.outgoing_degree(&BlockId::new("B")), 1);  // B→C
        assert_eq!(net.outgoing_degree(&BlockId::new("C")), 0);  // 无出边

        assert_eq!(net.incoming_degree(&BlockId::new("A")), 0);  // 无入边
        assert_eq!(net.incoming_degree(&BlockId::new("B")), 1);  // A→B
        assert_eq!(net.incoming_degree(&BlockId::new("C")), 2);  // B→C + A→C
    }

    #[test]
    fn test_outgoing_targets() {
        let mut net = MeshBlockNetwork::new();
        net.register_block(make_fact("A", 0));
        net.register_block(make_fact("B", 0));
        net.register_block(make_fact("C", 0));

        net.connect(BlockId::new("A"), BlockId::new("B"), ConnectionType::Vertical);
        net.connect(BlockId::new("A"), BlockId::new("C"), ConnectionType::Horizontal);

        let targets = net.outgoing_targets(&BlockId::new("A"));
        assert_eq!(targets.len(), 2);
        // Vec 顺序不保证，检查包含性而非固定位置
        let target_ids: Vec<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
        assert!(target_ids.contains(&"B"));
        assert!(target_ids.contains(&"C"));
        // 权重验证
        let (_, v_weight) = targets.iter().find(|(id, _)| id.as_str() == "B").unwrap();
        assert_eq!(*v_weight, ConnectionType::Vertical.default_weight());
    }

    #[test]
    fn test_domain_and_type_index() {
        let mut net = MeshBlockNetwork::new();
        net.register_block(make_fact("Math.Arithmetic.Fact.Add_1_1", 2));
        net.register_block(make_fact("Math.Arithmetic.Fact.Add_2_2", 4));
        net.register_block(make_fact("Physics.Motion.Fact.Velocity", 10));

        // 域名索引（精确域名匹配）
        let arithmetic_facts = net.find_by_domain("Math.Arithmetic.Fact");
        assert_eq!(arithmetic_facts.len(), 2);

        let physics_facts = net.find_by_domain("Physics.Motion.Fact");
        assert_eq!(physics_facts.len(), 1);

        // 类型索引
        let fact_blocks = net.find_by_type(BlockType::Fact);
        assert_eq!(fact_blocks.len(), 3);

        // 类型统计
        let counts = net.block_type_counts();
        assert_eq!(*counts.get(&BlockType::Fact).unwrap(), 3);
    }

    #[test]
    fn test_domain_prefix_search() {
        let mut net = MeshBlockNetwork::new();
        net.register_block(make_fact("Math.Arithmetic.Fact.X", 1));
        net.register_block(make_fact("Math.Arithmetic.Rule.Y", 2));
        net.register_block(make_fact("Math.Geometry.Fact.Z", 3));

        let all_math = net.find_by_domain_prefix("Math");
        assert_eq!(all_math.len(), 3);

        let arithmetic = net.find_by_domain_prefix("Math.Arithmetic");
        assert_eq!(arithmetic.len(), 2);
    }

    #[test]
    fn test_reset_and_decay() {
        let mut net = MeshBlockNetwork::new();
        let mut block = make_fact("Test.Fact.X", 1);
        block.state.accumulate(200);  // 激活到 200
        net.register_block(block);

        let test_id = BlockId::new("Test.Fact.X");

        // 重置
        net.reset_all_states();
        assert_eq!(net.get_block(&test_id).unwrap().state.activation, 0);

        // 重新累加
        net.get_block_mut(&test_id).unwrap().state.accumulate(100);
        // 自然衰减
        net.natural_decay_all();
        assert!(net.get_block(&test_id).unwrap().state.activation < 100);
    }
}
