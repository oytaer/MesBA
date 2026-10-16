// ============================================================================
// pulse.rs —— 脉冲（Pulse）结构体定义
// ============================================================================
// 功能：定义 MesBA 网状区块网络中的信号载体——脉冲
// 设计原则：
//   1. 脉冲是 MesBA 计算的基本单位：推理 = 脉冲沿连接涟漪传播
//   2. 携带信息：类型、强度、payload、传播路径、TTL
//   3. 防爆炸：TTL 限制 + 路径去重 + 强度阈值 + 不应期
//
// 对应设计文档：
//   - 网状区块架构 v2 §6.2 Pulse 数据结构
//   - 网状区块架构 v2 §6.3 脉冲传播算法（涟漪扩散）
//   - 网状区块架构 v2 §6.4 防脉冲爆炸六道防线
// ============================================================================

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::block_types::PulseType;
use crate::id::{BlockId, PulseId};

/// =========================================================================
/// Pulse —— 脉冲信号载体
/// =========================================================================
/// 脉冲在 MBN 的连接上传播，激活区块并携带查询参数/执行指令/结果/误差。
///
/// 传播过程（设计文档 §6.3）：
///   1. 检查 TTL：若 ttl==0 则消亡
///   2. 查找目标区块：若不存在则消亡
///   3. 检查不应期：若目标区块正在不应期则跳过
///   4. 根据脉冲类型累加/累减激活强度
///   5. 若激活强度≥阈值：读出区块内容 → 写入脉冲缓存
///   6. 设置不应期
///   7. 向所有开放连接转发新脉冲（强度经权重×衰减）
///   8. 路径去重：已访问区块不再重复
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pulse {
    /// 脉冲唯一 ID（全局递增）
    pub id: PulseId,

    /// 脉冲类型（激活/抑制/控制/结果/误差）
    pub pulse_type: PulseType,

    /// 起始区块 ID（脉冲从哪个区块发出）
    pub origin: BlockId,

    /// 当前目标区块 ID（脉冲下一步要到达的区块）
    pub target: BlockId,

    /// 当前脉冲强度（uint8 0~255，传播中会经权重×衰减逐步变弱）
    pub strength: u8,

    /// 脉冲携带的 payload（查询参数 / 执行指令 / 结果值 / 误差数据）
    /// 类型：Vec<u8> —— 通用字节数组，允许携带任意数据
    /// 具体内容由使用场景决定（如事实区块读取时携带查询 Key）
    pub payload: Vec<u8>,

    /// 已访问区块 ID 列表（防环：路径去重）
    /// 脉冲永远不会再次激活已经访问过的区块，防止死循环
    pub path: Vec<BlockId>,

    /// 剩余跳数（TTL）：脉冲在网络中最多还能传播多少跳
    /// 每到达一个区块后 ttl-1，归零则消亡
    pub ttl: u8,

    /// 脉冲产生时的计步器时间戳（用于调试/日志/追踪）
    pub timestamp: u64,
}

impl Pulse {
    // -----------------------------------------------------------------------
    // 构造函数
    // -----------------------------------------------------------------------

    /// 创建新脉冲（完整参数版本）
    #[allow(clippy::too_many_arguments)]  // 脉冲完整构造参数多是合理的设计
    pub fn new(
        id: PulseId,
        pulse_type: PulseType,
        origin: BlockId,
        target: BlockId,
        strength: u8,
        payload: Vec<u8>,
        ttl: u8,
        timestamp: u64,
    ) -> Self {
        Self {
            id,
            pulse_type,
            origin: origin.clone(),
            target,
            strength,
            payload,
            path: vec![origin],
            ttl,
            timestamp,
        }
    }

    /// 创建简化版激活脉冲（最常用的脉冲类型）
    pub fn activate(
        id: PulseId,
        origin: BlockId,
        target: BlockId,
        strength: u8,
        payload: Vec<u8>,
        timestamp: u64,
    ) -> Self {
        Self::new(id, PulseType::Activate, origin, target, strength, payload,
                  crate::value::DEFAULT_TTL, timestamp)
    }

    // -----------------------------------------------------------------------
    // 传播辅助方法
    // -----------------------------------------------------------------------

    /// 检查脉冲是否还能继续传播（三道防线：TTL > 0 + 强度 >= MIN_SIGNAL + 目标区块不在 path）
    pub fn can_propagate_to(&self, target_id: &BlockId) -> bool {
        // 防线1：TTL > 0
        if self.ttl == 0 {
            return false;
        }
        // 防线2：强度 >= MIN_SIGNAL（5/255 ≈ 2%）
        if self.strength < crate::value::MIN_SIGNAL {
            return false;
        }
        // 防线3：路径去重 —— 目标区块已在路径中
        if self.path.iter().any(|b| b == target_id) {
            return false;
        }
        true
    }

    /// 消耗一跳：TTL - 1，并将当前 target 加入 path
    /// 产生要发送给下一跳的新脉冲副本
    pub fn forward_to(
        &self,
        new_target: BlockId,
        new_strength: u8,
        new_id: PulseId,
        new_type: PulseType,
        timestamp: u64,
    ) -> Pulse {
        // 更新 path：当前 target 是上一跳已访问区块
        let mut new_path = self.path.clone();
        new_path.push(self.target.clone());

        Pulse {
            id: new_id,
            pulse_type: new_type,
            origin: self.origin.clone(),     // origin 不变
            target: new_target,               // 新目标
            strength: new_strength,
            payload: self.payload.clone(),    // payload 传递
            path: new_path,
            ttl: self.ttl.saturating_sub(1),  // TTL - 1
            timestamp,
        }
    }

    // -----------------------------------------------------------------------
    // payload 便捷操作
    // -----------------------------------------------------------------------

    /// 将 payload 解析为字符串（如事实区块的查询 Key）
    pub fn payload_as_string(&self) -> Option<String> {
        String::from_utf8(self.payload.clone()).ok()
    }

    /// 将 payload 解析为整数（如结果脉冲携带的值）
    pub fn payload_as_i32(&self) -> Option<i32> {
        if self.payload.len() >= 4 {
            Some(i32::from_le_bytes([
                self.payload[0],
                self.payload[1],
                self.payload[2],
                self.payload[3],
            ]))
        } else {
            None
        }
    }

    /// 将整数写入 payload（小端序）
    pub fn payload_from_i32(value: i32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    /// 将字符串写入 payload
    pub fn payload_from_string(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    // -----------------------------------------------------------------------
    // 调试辅助
    // -----------------------------------------------------------------------

    /// 当前脉冲路径深度（已经过多少区块）
    pub fn path_depth(&self) -> usize {
        self.path.len()
    }

    /// 剩余传播深度（TTL - 已走过）
    pub fn remaining_depth(&self) -> i32 {
        self.ttl as i32 - self.path.len() as i32
    }
}

impl fmt::Display for Pulse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Pulse[{} #{}] {}→{} strength={} ttl={} path={} depth",
            self.pulse_type,
            self.id.as_u64(),
            self.origin,
            self.target,
            self.strength,
            self.ttl,
            self.path_depth()
        )
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::DEFAULT_TTL;

    #[test]
    fn test_pulse_creation() {
        let pulse = Pulse::activate(
            PulseId::new(1),
            BlockId::new("A"),
            BlockId::new("B"),
            255,
            b"query_key".to_vec(),
            0,
        );
        assert_eq!(pulse.id.as_u64(), 1);
        assert_eq!(pulse.pulse_type, PulseType::Activate);
        assert_eq!(pulse.origin.as_str(), "A");
        assert_eq!(pulse.target.as_str(), "B");
        assert_eq!(pulse.strength, 255);
        assert_eq!(pulse.ttl, DEFAULT_TTL);
        assert_eq!(pulse.path.len(), 1);
        assert_eq!(pulse.path[0].as_str(), "A");
    }

    #[test]
    fn test_pulse_forward() {
        let pulse = Pulse::activate(
            PulseId::new(1),
            BlockId::new("A"),
            BlockId::new("B"),
            200,
            vec![],
            0,
        );
        let forwarded = pulse.forward_to(
            BlockId::new("C"),
            150,
            PulseId::new(2),
            PulseType::Result,
            1,
        );
        assert_eq!(forwarded.ttl, pulse.ttl - 1);
        assert_eq!(forwarded.strength, 150);
        assert_eq!(forwarded.path.len(), 2);
        assert_eq!(forwarded.path[1].as_str(), "B");
        assert_eq!(forwarded.origin.as_str(), "A");  // origin 不变
        assert_eq!(forwarded.target.as_str(), "C");
    }

    #[test]
    fn test_propagation_guards() {
        let pulse = Pulse::activate(
            PulseId::new(1),
            BlockId::new("A"),
            BlockId::new("B"),
            200,
            vec![],
            0,
        );

        // 正常情况
        assert!(pulse.can_propagate_to(&BlockId::new("C")));

        // 防线1：TTL耗尽 —— 构造一个 ttl=0 的脉冲
        let mut p2 = pulse.clone();
        p2.ttl = 0;
        assert!(!p2.can_propagate_to(&BlockId::new("C")));

        // 防线2：强度 < MIN_SIGNAL
        let mut p3 = pulse.clone();
        p3.strength = 0;
        assert!(!p3.can_propagate_to(&BlockId::new("C")));
        p3.strength = crate::value::MIN_SIGNAL - 1;
        assert!(!p3.can_propagate_to(&BlockId::new("C")));
        p3.strength = crate::value::MIN_SIGNAL;
        assert!(p3.can_propagate_to(&BlockId::new("C")));

        // 防线3：路径去重 —— 目标已在 path 中
        assert!(!pulse.can_propagate_to(&BlockId::new("A")));  // A 是 origin
    }

    #[test]
    fn test_payload_conversions() {
        let payload_str = Pulse::payload_from_string("9+9");
        let payload_int = Pulse::payload_from_i32(198);

        let p_str = Pulse::activate(
            PulseId::new(1), BlockId::new("A"), BlockId::new("B"),
            200, payload_str, 0,
        );
        assert_eq!(p_str.payload_as_string(), Some("9+9".to_string()));

        let p_int = Pulse::activate(
            PulseId::new(2), BlockId::new("C"), BlockId::new("D"),
            200, payload_int, 0,
        );
        assert_eq!(p_int.payload_as_i32(), Some(198));
    }

    #[test]
    fn test_path_tracking() {
        let mut pulse = Pulse::activate(
            PulseId::new(1),
            BlockId::new("Start"),
            BlockId::new("Step1"),
            200, vec![], 0,
        );
        assert_eq!(pulse.path_depth(), 1);

        pulse = pulse.forward_to(
            BlockId::new("Step2"), 150, PulseId::new(2),
            PulseType::Result, 1,
        );
        assert_eq!(pulse.path_depth(), 2);

        pulse = pulse.forward_to(
            BlockId::new("Step3"), 100, PulseId::new(3),
            PulseType::Result, 2,
        );
        assert_eq!(pulse.path_depth(), 3);
        assert_eq!(pulse.path[0].as_str(), "Start");
        assert_eq!(pulse.path[1].as_str(), "Step1");
        assert_eq!(pulse.path[2].as_str(), "Step2");
    }
}
