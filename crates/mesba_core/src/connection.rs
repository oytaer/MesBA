// ============================================================================
// connection.rs —— 连接结构体定义
// ============================================================================
// 功能：定义 MesBA 网状区块网络（MBN）的边（Edge）数据结构
// 设计原则：
//   1. 每条连接关联源区块 → 目标区块，带权重/衰减/门控/可塑性
//   2. 权重、衰减系数、门控均用 uint8 表示（Q8.8 定点小数）
//   3. 支持 Hebbian 学习：连接可被加强/减弱/修剪
//
// 对应设计文档：
//   - 网状区块架构 v2 §5.2 Connection 属性
//   - 网状区块模型 v2 §3.1-§3.2 权重/衰减初始值
// ============================================================================

use serde::{Deserialize, Serialize};

use crate::block_types::ConnectionType;
use crate::id::{BlockId, ConnectionId};

/// =========================================================================
/// Connection —— 区块间连接（MBN 的边）
/// =========================================================================
/// 连接代表两个区块之间的关联关系，是脉冲传播的通道。
/// 连接方向：source → target（脉冲从 source 区块流向 target 区块）
///
/// 权重含义（Q8.8 定点小数，0~255 映射到 0.0~1.0）：
///   - 权重越高，脉冲通过此连接时衰减越少
///   - 每次脉冲传播时：new_strength = uint8_multiply(old_strength, weight)
///
/// 衰减系数含义（也是 Q8.8）：
///   - 权重表示"这条通道的重要程度"
///   - 衰减系数表示"每走一跳的自然损耗"
///   - 实际衰减 = weight × decay（两层乘法）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// 连接唯一 ID（u64 自增）
    pub id: ConnectionId,

    /// 源区块 ID（脉冲从这里出发）
    pub source_id: BlockId,

    /// 目标区块 ID（脉冲到达这里）
    pub target_id: BlockId,

    /// 连接类型（纵向/横向/斜向/回环/时序）
    pub conn_type: ConnectionType,

    /// 连接权重（Q8.8 定点小数，0~255 → 0.0~1.0）
    /// 默认值由 conn_type.default_weight() 决定
    pub weight: u8,

    /// 信号衰减系数（Q8.8 定点小数，0~255 → 0.0~1.0）
    /// 每跳的信号衰减：new_strength = uint8_multiply(strength, decay)
    pub decay: u8,

    /// 门控状态（true=开放，false=关闭）
    /// 回环连接默认关闭，由判断区块或游离机制按需打开
    pub gate: bool,

    /// 可塑性率（Hebbian 学习此连接的速度，uint8 表示 0~1.0）
    pub hebbian_rate: u8,

    /// 最后权重更新时间（用于衰减计算）
    /// 以内部计步器步数表示，不是真实 wall-clock 时间
    pub last_updated_step: u64,
}

impl Connection {
    // -----------------------------------------------------------------------
    // 构造函数
    // -----------------------------------------------------------------------

    /// 创建新连接，使用类型对应的默认权重/衰减/门控
    pub fn new(
        id: ConnectionId,
        source_id: BlockId,
        target_id: BlockId,
        conn_type: ConnectionType,
    ) -> Self {
        Self {
            id,
            source_id,
            target_id,
            conn_type,
            weight: conn_type.default_weight(),
            decay: conn_type.default_decay(),
            gate: !conn_type.needs_gate(),  // 回环默认关闭，其他默认开放
            hebbian_rate: 50,  // 默认学习率 50/255 ≈ 0.2
            last_updated_step: 0,
        }
    }

    /// 创建新连接并指定权重和衰减（用于手动初始化/训练后写入）
    pub fn with_weight_decay(
        id: ConnectionId,
        source_id: BlockId,
        target_id: BlockId,
        conn_type: ConnectionType,
        weight: u8,
        decay: u8,
    ) -> Self {
        Self {
            id,
            source_id,
            target_id,
            conn_type,
            weight,
            decay,
            gate: !conn_type.needs_gate(),
            hebbian_rate: 50,
            last_updated_step: 0,
        }
    }

    // -----------------------------------------------------------------------
    // 功能方法
    // -----------------------------------------------------------------------

    /// 计算经过此连接后的脉冲强度衰减结果
    /// 算法：先乘权重，再乘衰减系数（两次 uint8_multiply）
    /// 返回 None 表示衰减后强度 < MIN_SIGNAL，此连接可以跳过
    pub fn transmit_strength(&self, incoming_strength: u8) -> u8 {
        let after_weight = crate::integer_ops::uint8_multiply(incoming_strength, self.weight);
        crate::integer_ops::uint8_multiply(after_weight, self.decay)
    }

    /// 是否允许脉冲通过（门控开启 + 权重>0）
    pub fn is_open(&self) -> bool {
        self.gate && self.weight > 0
    }

    /// Hebbian 加强连接权重
    /// Δw = hebbian_rate × activation_i × activation_j / 255（定点乘法近似）
    /// 结果钳位到 0~255
    pub fn strengthen(&mut self, activation_strength_i: u8, activation_strength_j: u8) {
        let delta = crate::integer_ops::uint8_multiply(
            self.hebbian_rate,
            crate::integer_ops::uint8_multiply(activation_strength_i, activation_strength_j),
        );
        self.weight = crate::integer_ops::sat8_add(self.weight, delta);
    }

    /// 显式增加权重（用于错误反馈修正）
    pub fn increase_weight(&mut self, amount: u8) {
        self.weight = crate::integer_ops::sat8_add(self.weight, amount);
    }

    /// 显式减少权重（用于错误反馈修正）
    pub fn decrease_weight(&mut self, amount: u8) {
        self.weight = crate::integer_ops::sat8_sub(self.weight, amount);
    }

    /// 打开门控
    pub fn open_gate(&mut self) {
        self.gate = true;
    }

    /// 关闭门控
    pub fn close_gate(&mut self) {
        self.gate = false;
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::MIN_SIGNAL;

    #[test]
    fn test_connection_creation() {
        let conn = Connection::new(
            ConnectionId::new(1),
            BlockId::new("A"),
            BlockId::new("B"),
            ConnectionType::Vertical,
        );
        assert_eq!(conn.id.as_u64(), 1);
        assert_eq!(conn.source_id.as_str(), "A");
        assert_eq!(conn.target_id.as_str(), "B");
        assert!(conn.is_open());  // 纵向默认开放
        assert_eq!(conn.weight, ConnectionType::Vertical.default_weight());
    }

    #[test]
    fn test_loop_connection_default_gate_closed() {
        let conn = Connection::new(
            ConnectionId::new(2),
            BlockId::new("X"),
            BlockId::new("X"),
            ConnectionType::Loop,
        );
        assert!(!conn.is_open());  // 回环默认关闭
        assert_eq!(conn.weight, 0);
    }

    #[test]
    fn test_transmit_strength() {
        // 强纵向连接：权重 230(0.90)，衰减 230(0.90)
        let conn = Connection::with_weight_decay(
            ConnectionId::new(3),
            BlockId::new("S"),
            BlockId::new("T"),
            ConnectionType::Vertical,
            230,  // 0.90
            230,  // 0.90
        );
        let result = conn.transmit_strength(255);  // 满强度脉冲
        // 255 × 230 / 256 ≈ 229；229 × 230 / 256 ≈ 206
        assert!((180..=220).contains(&result));
        assert!(result >= MIN_SIGNAL);
    }

    #[test]
    fn test_transmit_strength_diagonal_weak() {
        // 斜向连接很弱：权重 26(0.10)，衰减 128(0.50)
        let conn = Connection::with_weight_decay(
            ConnectionId::new(4),
            BlockId::new("A"),
            BlockId::new("B"),
            ConnectionType::Diagonal,
            26,
            128,
        );
        let result = conn.transmit_strength(255);
        // 255 × 26 / 256 ≈ 25；25 × 128 / 256 ≈ 12
        assert!(result < 30);  // 很弱
    }

    #[test]
    fn test_hebbian_strengthen() {
        let mut conn = Connection::with_weight_decay(
            ConnectionId::new(5),
            BlockId::new("A"),
            BlockId::new("B"),
            ConnectionType::Horizontal,
            50,   // 初始权重
            179,  // 衰减
        );
        let initial_weight = conn.weight;
        // 两边都强激活
        conn.strengthen(255, 255);
        assert!(conn.weight > initial_weight);
        // 多次加强
        conn.strengthen(255, 255);
        conn.strengthen(255, 255);
        assert!(conn.weight < 255);  // 不会超过饱和值
    }

    #[test]
    fn test_weight_adjust() {
        let mut conn = Connection::new(
            ConnectionId::new(6),
            BlockId::new("A"),
            BlockId::new("B"),
            ConnectionType::Horizontal,
        );
        let initial = conn.weight;
        // increase_weight 后验证
        let expected_increased = crate::integer_ops::sat8_add(initial, 10);
        conn.increase_weight(10);
        assert_eq!(conn.weight, expected_increased);
        // decrease_weight 前先计算期望值，再执行操作后验证
        let expected_decreased = crate::integer_ops::sat8_sub(conn.weight, 100);
        conn.decrease_weight(100);
        assert_eq!(conn.weight, expected_decreased);
    }

    #[test]
    fn test_gate_control() {
        let mut conn = Connection::new(
            ConnectionId::new(7),
            BlockId::new("X"),
            BlockId::new("Y"),
            ConnectionType::Loop,
        );
        assert!(!conn.gate);
        conn.open_gate();
        assert!(conn.gate);
        conn.close_gate();
        assert!(!conn.gate);
    }
}
