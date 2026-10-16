// ============================================================================
// block_types.rs —— 区块类型 / 连接类型 / 脉冲类型 枚举定义
// ============================================================================
// 对应设计文档：
//   - 网状区块架构 v2 §4 区块设计（9种类型）
//   - 网状区块架构 v2 §5 连接设计（5种类型）
//   - 网状区块架构 v2 §6.1 五种脉冲类型
// ============================================================================

use serde::{Deserialize, Serialize};

// ============================================================================
// BlockType —— 9 种区块类型枚举
// ============================================================================
// MesBA 知识表示的全部原子类型，覆盖从感知到抽象的完整认知链路。
// 参考：网状区块架构 v2 §4.2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockType {
    /// 感知区块：接收原始输入信号，提取初级特征
    /// 存储内容：特征提取规则/感知模板
    /// 示例："数字识别"接收"9"→输出{type:digit, value:9}
    Perceptual,

    /// 概念区块：表示一个概念/语义实体，聚合相关事实和规则
    /// 存储内容：概念内涵、特征集合、典型实例、相关概念
    /// 示例："加法"聚合加法规则+加法表+进位概念
    Concept,

    /// 事实区块：存储陈述性知识，Key→Value 查表
    /// 存储内容：KV 映射表（支持精确匹配/模糊匹配）
    /// 示例："9+9→18"、"中国首都→北京"
    Fact,

    /// 规则区块：存储程序性知识，输出执行步骤序列
    /// 存储内容：步骤列表+前置条件+后置条件
    /// 示例："加法策略": [对齐→逐位相加→处理进位→输出]
    Rule,

    /// 判断区块：条件判断，输出分支决策
    /// 存储内容：条件表达式（对工作记忆变量比较）+ 分支目标
    /// 示例："和≥10?"：true→进位流程，false→不进位
    Judgment,

    /// 计划区块：存储策略/方案模板，用于复杂任务分解
    /// 存储内容：目标分解模板、子目标依赖、风险预估、备选方案
    /// 示例："应用题求解": [理解→提取→列式→计算→检验]
    Plan,

    /// 操作区块：执行具体动作，调用外部工具/API
    /// 存储内容：操作接口定义+参数映射+前置条件+结果格式
    /// 示例："计算器加法"调用外部 API；"写文件"写入文件系统
    Operation,

    /// 经验区块：存储过往经历（成功/失败案例），用于纠错和类比
    /// 存储内容：问题情境+方案+结果+教训标注
    /// 示例："99+99算成188"→教训：处理完个位必须检查进位
    Experience,

    /// 抽象区块：跨领域通用模式，支撑迁移
    /// 存储内容：变量化的规则骨架、适用范围、已映射的具体领域
    /// 示例："逐位运算"从加法迁移到减法/乘法
    Abstract,
}

impl BlockType {
    /// 区块类型的简短标识符（用于 BlockId 中段）
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockType::Perceptual => "Perceive",
            BlockType::Concept   => "Concept",
            BlockType::Fact      => "Fact",
            BlockType::Rule      => "Rule",
            BlockType::Judgment  => "Judge",
            BlockType::Plan      => "Plan",
            BlockType::Operation => "Op",
            BlockType::Experience=> "Exp",
            BlockType::Abstract  => "Abstract",
        }
    }

    /// 默认激活阈值（来自 MeshBlock v2 §3.3）
    /// 感知区块阈值最低（易激活），抽象区块最高（需多证据）
    pub fn default_threshold(&self) -> u8 {
        match self {
            BlockType::Perceptual => 30,
            BlockType::Fact       => 50,
            BlockType::Concept    => 80,
            BlockType::Rule       => 60,
            BlockType::Judgment   => 40,
            BlockType::Operation  => 70,
            BlockType::Plan       => 100,
            BlockType::Experience => 60,
            BlockType::Abstract   => 120,
        }
    }

    /// 是否属于"可查表"类型（事实区块直接返回 value）
    pub fn is_lookup_type(&self) -> bool {
        matches!(self, BlockType::Fact)
    }

    /// 是否属于"可执行"类型（规则/操作区块）
    pub fn is_executable_type(&self) -> bool {
        matches!(self, BlockType::Rule | BlockType::Operation)
    }

    /// 是否属于"决策"类型（判断/计划区块）
    pub fn is_decision_type(&self) -> bool {
        matches!(self, BlockType::Judgment | BlockType::Plan)
    }

    /// 全部9种类型迭代器（用于遍历和注册）
    pub fn all() -> [BlockType; 9] {
        [
            BlockType::Perceptual,
            BlockType::Concept,
            BlockType::Fact,
            BlockType::Rule,
            BlockType::Judgment,
            BlockType::Plan,
            BlockType::Operation,
            BlockType::Experience,
            BlockType::Abstract,
        ]
    }
}

impl std::fmt::Display for BlockType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// ConnectionType —— 5 种连接类型枚举
// ============================================================================
// MesBA 区块网络的全部边类型，支持从抽象到具体、从同级协作、
// 跨域类比、自指反馈到因果时序的完整连接拓扑。
// 参考：网状区块架构 v2 §5.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionType {
    /// 纵向连接：父区块 ↔ 子区块（抽象 ↔ 具体，指导 ↔ 支撑）
    /// 默认权重高（204~255），衰减小（0.90/hop），信号强传递
    Vertical,

    /// 横向连接：同级区块之间（协作/互补/竞争）
    /// 默认权重中等（51~127），衰减中等（0.70/hop）
    Horizontal,

    /// 斜向连接：跨领域/跨层级（跨域联想/类比迁移）
    /// 默认权重低（13~51），衰减大（0.50/hop），只有强脉冲才能激活
    Diagonal,

    /// 回环连接：自指或绕回（反思/循环/自检/反馈）
    /// 默认门控关闭（由判断区块或游离机制按需打开），防死循环
    Loop,

    /// 时序连接：前驱 → 后继（因果顺序/步骤链/时间先后）
    /// 默认权重高（153~230），衰减小（0.85/hop）
    Temporal,
}

impl ConnectionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionType::Vertical   => "vertical",
            ConnectionType::Horizontal => "horizontal",
            ConnectionType::Diagonal   => "diagonal",
            ConnectionType::Loop       => "loop",
            ConnectionType::Temporal   => "temporal",
        }
    }

    /// 默认初始权重（来自 MeshBlock v2 §3.1）
    pub fn default_weight(&self) -> u8 {
        use crate::value::{Q8_8_0_10, Q8_8_0_50, Q8_8_0_80, Q8_8_0_90};
        match self {
            ConnectionType::Vertical   => Q8_8_0_90,   // 230
            ConnectionType::Horizontal => Q8_8_0_50,   // 128
            ConnectionType::Diagonal   => Q8_8_0_10,   // 26
            ConnectionType::Loop       => 0,            // 门控默认关闭
            ConnectionType::Temporal   => Q8_8_0_80,   // 204
        }
    }

    /// 默认衰减系数（来自 MeshBlock v2 §3.2）
    pub fn default_decay(&self) -> u8 {
        use crate::value::{Q8_8_0_50, Q8_8_0_70, Q8_8_0_80, Q8_8_0_85, Q8_8_0_90};
        match self {
            ConnectionType::Vertical   => Q8_8_0_90,   // 230
            ConnectionType::Horizontal => Q8_8_0_70,   // 179
            ConnectionType::Diagonal   => Q8_8_0_50,   // 128
            ConnectionType::Loop       => Q8_8_0_80,   // 204
            ConnectionType::Temporal   => Q8_8_0_85,   // 218
        }
    }

    /// 默认信号 TTL（跳数上限）
    pub fn default_ttl(&self) -> u8 {
        match self {
            ConnectionType::Vertical   => 8,
            ConnectionType::Horizontal => 5,
            ConnectionType::Diagonal   => 3,
            ConnectionType::Loop       => 3,
            ConnectionType::Temporal   => 10,
        }
    }

    /// 纵向连接是否表示"子→父"方向（支撑关系）
    pub fn is_vertical(&self) -> bool {
        matches!(self, ConnectionType::Vertical)
    }

    /// 是否为跨域连接（斜向）
    pub fn is_cross_domain(&self) -> bool {
        matches!(self, ConnectionType::Diagonal)
    }

    /// 是否为回环连接（需要门控）
    pub fn needs_gate(&self) -> bool {
        matches!(self, ConnectionType::Loop)
    }

    /// 全部5种类型迭代器
    pub fn all() -> [ConnectionType; 5] {
        [
            ConnectionType::Vertical,
            ConnectionType::Horizontal,
            ConnectionType::Diagonal,
            ConnectionType::Loop,
            ConnectionType::Temporal,
        ]
    }
}

impl std::fmt::Display for ConnectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// PulseType —— 5 种脉冲类型枚举
// ============================================================================
// MesBA 脉冲传播中全部信号类型，实现信号差异化传递。
// 参考：网状区块架构 v2 §6.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PulseType {
    /// 激活脉冲（ActivatePulse）：激活目标区块，启动读出
    /// 产生者：游离机制 / 已激活区块
    /// 功能：查询参数 / 调用参数
    /// 视觉标记：🔴 红
    Activate,

    /// 抑制脉冲（InhibitPulse）：抑制目标区块（竞争/排除）
    /// 产生者：判断区块 / 游离机制
    /// 功能：抑制强度
    /// 视觉标记：🔵 蓝
    Inhibit,

    /// 控制脉冲（ControlPulse）：流程控制（开始/暂停/回退/终止）
    /// 产生者：游离机制 Controller
    /// 功能：控制指令
    /// 视觉标记：🟡 黄
    Control,

    /// 结果脉冲（ResultPulse）：携带读出结果传回/传给邻居
    /// 产生者：被激活区块输出
    /// 功能：结果值 + 来源区块 ID
    /// 视觉标记：🟢 绿
    Result,

    /// 误差脉冲（ErrorPulse）：携带误差信号，修正权重/阈值
    /// 产生者：反馈系统
    /// 功能：误差大小 + 误差类型
    /// 视觉标记：⚪ 白
    Error,
}

impl PulseType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PulseType::Activate => "activate",
            PulseType::Inhibit  => "inhibit",
            PulseType::Control  => "control",
            PulseType::Result   => "result",
            PulseType::Error    => "error",
        }
    }

    /// 是否增加目标区块激活强度（累加）
    pub fn is_accumulate(&self) -> bool {
        matches!(self, PulseType::Activate | PulseType::Result)
    }

    /// 是否减少目标区块激活强度（累减）
    pub fn is_decay(&self) -> bool {
        matches!(self, PulseType::Inhibit)
    }

    /// 是否为即时执行类型（不改变激活强度，直接处理）
    pub fn is_immediate(&self) -> bool {
        matches!(self, PulseType::Control | PulseType::Error)
    }

    /// 全部5种类型迭代器
    pub fn all() -> [PulseType; 5] {
        [
            PulseType::Activate,
            PulseType::Inhibit,
            PulseType::Control,
            PulseType::Result,
            PulseType::Error,
        ]
    }
}

impl std::fmt::Display for PulseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_type_count() {
        assert_eq!(BlockType::all().len(), 9);
    }

    #[test]
    fn test_block_type_thresholds() {
        // 感知阈值最低（易激活），抽象最高（需多证据）
        let perceptual = BlockType::Perceptual.default_threshold();
        let abstract_t = BlockType::Abstract.default_threshold();
        assert!(perceptual < abstract_t);
        assert!(perceptual <= 50);
        assert!(abstract_t >= 100);
    }

    #[test]
    fn test_connection_type_weights() {
        // 纵向权重最高，斜向最低
        let v = ConnectionType::Vertical.default_weight();
        let h = ConnectionType::Horizontal.default_weight();
        let d = ConnectionType::Diagonal.default_weight();
        assert!(v > h);
        assert!(h > d);
        assert_eq!(ConnectionType::Loop.default_weight(), 0);
    }

    #[test]
    fn test_connection_type_decays() {
        // 纵向衰减小，斜向衰减大
        let v = ConnectionType::Vertical.default_decay();
        let d = ConnectionType::Diagonal.default_decay();
        assert!(v > d);
    }

    #[test]
    fn test_pulse_type_accumulate() {
        assert!(PulseType::Activate.is_accumulate());
        assert!(PulseType::Result.is_accumulate());
        assert!(!PulseType::Inhibit.is_accumulate());
        assert!(PulseType::Inhibit.is_decay());
        assert!(PulseType::Control.is_immediate());
        assert!(PulseType::Error.is_immediate());
    }

    #[test]
    fn test_type_display() {
        assert_eq!(format!("{}", BlockType::Fact), "Fact");
        assert_eq!(format!("{}", ConnectionType::Vertical), "vertical");
        assert_eq!(format!("{}", PulseType::Activate), "activate");
    }
}
