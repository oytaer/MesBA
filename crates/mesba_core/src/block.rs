// ============================================================================
// block.rs —— 区块（Block）核心结构体 + 9 种区块内容定义
// ============================================================================
// 功能：定义 MesBA 网状区块网络（MBN）的节点数据结构
// 设计原则：
//   1. Block 是知识的原子存储单元，9 种类型各自有不同 content 结构
//   2. 连接采用"区块存自己的连接列表"方式，避免双向查找开销
//   3. 激活状态（activation/threshold/refractory）动态更新
//   4. 区块被激活时读出 content 并产生 ResultPulse
//
// 对应设计文档：
//   - 网状区块架构 v2 §4.1 区块通用结构
//   - 网状区块模型 v2 §2 九种区块 YAML 详细定义
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::block_types::BlockType;
use crate::id::BlockId;
use crate::integer_ops::{sat8_add, sat8_sub};
use crate::value::ACTIVATION_DECAY;

// ============================================================================
// BlockContent —— 区块内容枚举（9 种类型对应 9 种内容结构）
// ============================================================================
// 每种区块类型有自己独有的 content 结构，包含该类型特有的数据：
//   - 事实区块 → KV 表
//   - 规则区块 → 步骤序列 + 前置/后置条件
//   - 判断区块 → 条件表达式 + 分支目标
//   - 概念区块 → 定义 + 相关区块引用
//   - 操作区块 → 操作算法 / 外部 API 定义
//   - 计划区块 → 目标分解模板 + 风险预估
//   - 经验区块 → 情境 + 方案 + 教训
//   - 抽象区块 → 模式骨架 + 变量 + 映射
//   - 感知区块 → 特征提取模板
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockContent {
    /// 感知区块内容：特征提取规则 / 感知模板
    Perceptual {
        /// 输入类型（text_token / image_pixel / audio_frame ...）
        input_type: String,
        /// 特征模式列表（regex / 精确匹配规则）
        patterns: Vec<PerceptualPattern>,
    },

    /// 概念区块内容：概念定义 + 相关关联
    Concept {
        /// 概念定义文本
        definition: String,
        /// 概念属性/性质列表（如加法的交换律 a+b = b+a）
        properties: Vec<String>,
        /// 相关概念 BlockId 列表
        related_concepts: Vec<BlockId>,
    },

    /// 事实区块内容：Key-Value 映射表
    Fact {
        /// KV 条目列表（支持多 key 映射到同一 value）
        entries: Vec<FactEntry>,
    },

    /// 规则区块内容：步骤序列 + 前置条件 + 后置条件
    Rule {
        /// 规则名称
        name: String,
        /// 触发前置条件列表（如 "operation == 'addition'"）
        preconditions: Vec<String>,
        /// 步骤序列（规则被激活后依次执行）
        steps: Vec<RuleStep>,
    },

    /// 判断区块内容：条件表达式 + 分支决策
    Judgment {
        /// 条件表达式字符串（如 "digit_sum >= 10"）
        condition_expr: String,
        /// 需要从脉冲缓存读取的变量名列表
        variables: Vec<String>,
        /// true 分支：条件成立时的路由目标 + 设置变量
        true_branch: JudgmentBranch,
        /// false 分支：条件不成立时的路由目标 + 设置变量
        false_branch: JudgmentBranch,
    },

    /// 计划区块内容：目标分解模板
    Plan {
        /// 计划名称
        name: String,
        /// 子目标序列
        subgoals: Vec<PlanSubgoal>,
        /// 备用方案（当主方案某步失败时）
        backup_plans: Vec<BackupPlan>,
        /// 预估步骤数
        estimated_steps: u32,
    },

    /// 操作区块内容：内部算法 或 外部 API 定义
    Operation {
        /// 操作描述
        description: String,
        /// 操作类型：internal（内部计算） 或 external（调用外部工具）
        op_type: OperationType,
        /// 输入变量名列表
        input_variables: Vec<String>,
        /// 输出变量名列表
        output_variables: Vec<String>,
        /// 内部操作的算法描述（文本形式，实际逻辑在 Runtime 中实现）
        algorithm: String,
        /// 外部操作的 API 端点
        api_endpoint: String,
        /// 外部操作的超时时间（毫秒）
        timeout_ms: u64,
        /// 外部操作的重试次数
        retry_count: u32,
    },

    /// 经验区块内容：过往成功/失败案例
    Experience {
        /// 问题情境描述
        situation: String,
        /// 尝试的方案和步骤
        attempt_summary: String,
        /// 错误类型（如 "forgot_second_carry"）
        error_type: String,
        /// 正确结果（int32 或字符串表示）
        correct_result: String,
        /// 教训标注（如 "处理完每一位必须检查进位"）
        lesson: String,
        /// 预防性区块 BlockId（下次遇到同类情况时提示的区块）
        preventive_block: Option<BlockId>,
    },

    /// 抽象区块内容：跨领域通用模式
    Abstract {
        /// 抽象模式名称（如"逐位处理"）
        name: String,
        /// 模式骨架（变量化的步骤列表）
        pattern_steps: Vec<String>,
        /// 抽象变量名（如"position", "carry", "process_func"）
        abstract_variables: Vec<String>,
        /// 已映射的具体领域（domain → 变量实例化映射）
        domain_mappings: Vec<DomainMapping>,
        /// 迁移条件（如"sequential_processing AND overflow_possible"）
        migration_condition: String,
    },
}

// ============================================================================
// ReadResult —— 区块被激活后从 BlockContent 读出的结构化结果
// ============================================================================
// 设计文档 §6.3 伪代码：`result = target.read(pulse.payload)`
// 每个区块类型有自己的 read() 实现，返回不同形状的结果：
//   - Fact       → ReadResult::Value（查表得到的 FactValue）
//   - Judgment   → ReadResult::Branch（条件分支路由目标 + 要设置的变量）
//   - Operation  → ReadResult::Value（算法执行得到的 int32 结果）
//   - Rule       → ReadResult::Steps（规则步骤序列，交给游离机制依次执行）
//   - Plan       → ReadResult::Plan（子目标列表）
//   - Concept    → ReadResult::Properties（属性/关联概念列表）
//   - Perceptual → ReadResult::Trigger（命中的感知模式对应的概念区块）
//   - Experience → ReadResult::Properties（教训/建议）
//   - Abstract   → ReadResult::Properties（模式骨架）
//
// 关键架构决策：BlockContent::read() 是纯函数（不接收 &mut self，
// 不直接写 PulseBuffer），避免 Propagator &mut Network 的 borrow checker
// 冲突。Propagator 拿到 ReadResult 后再由上层（mesba_memory / mesba_free）
// 决定如何写入 PulseBuffer / 注入脉冲。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ReadResult {
    /// 数值结果：Fact 查表 / Operation 算法执行
    Value {
        /// 整数值（绝大多数计算结果都是整数）
        int_value: i32,
        /// 文本值（可选，用于补充描述）
        text_value: Option<String>,
        /// 布尔值（可选，用于 Judgment 条件求值的中间结果）
        bool_value: Option<bool>,
        /// 这个结果应该写入 PulseBuffer 的哪个变量
        /// Operation 由其 output_variables 第一个元素声明；
        /// Fact 查表返回 None（Fact 是被动查询，不主动存变量）
        output_var_name: Option<String>,
    },

    /// 分支路由：Judgment 条件求值后的路由目标
    Branch {
        /// 路由目标区块（true_branch 或 false_branch 的 target）
        target: Option<BlockId>,
        /// 此分支执行时要写入 PulseBuffer 的变量
        set_variables: HashMap<String, String>,
        /// 分支是否为 true_branch（用于日志）
        is_true_branch: bool,
        /// 未走分支的 target（用于 Propagator 发 Inhibit 脉冲抑制竞争分支）
        alt_branch_target: Option<BlockId>,
    },

    /// 步骤序列：Rule 被激活后读出的步骤列表
    Steps(Vec<RuleStep>),

    /// 子目标列表：Plan 被激活后读出的子目标
    Plan(Vec<PlanSubgoal>),

    /// 属性/模式列表：Concept / Abstract / Experience 被激活后读出
    Properties(Vec<String>),

    /// 触发区块列表：Perceptual 命中感知模式后，要通知的 Concept 区块
    Trigger(Vec<BlockId>),

    /// 空结果：此区块被激活但不产生任何读出（通常是连接传播中间节点）
    #[default]
    None,
}

// ---- 子结构定义 ----

/// 感知区块特征模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptualPattern {
    pub pattern: String,       // regex / 精确匹配字符串
    pub output_type: String,   // 提取后的特征类型（如 "digit", "operator"）
    pub confidence: u8,        // 该模式命中的置信度
}

/// 事实区块 KV 条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactEntry {
    /// 支持多 key 映射到同一 value（如 "9+9" 和 "九加九" 都映射到 18）
    pub keys: Vec<String>,
    /// 对应的值（整数或字符串）
    pub value: FactValue,
}

/// 事实值类型（整数 / 字符串 / 布尔）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactValue {
    Integer(i32),
    Text(String),
    Boolean(bool),
}

/// 规则区块步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleStep {
    /// 步骤唯一 ID
    pub id: String,
    /// 动作类型：call(调用区块) / set(设置变量) / emit(发出脉冲)
    pub action: String,
    /// 目标区块 BlockId（当 action=call 时）
    pub target: Option<BlockId>,
    /// 设置的变量名（当 action=set 时）
    pub variable: Option<String>,
    /// 设置的变量值（文本表示）
    pub value_text: Option<String>,
    /// 步骤描述（用于日志/可解释性）
    pub description: String,
    /// 是否循环执行直到判断区块返回 true
    pub loop_until: Option<BlockId>,
}

/// 判断区块分支
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgmentBranch {
    /// 分支动作：route(路由到目标区块) / emit(发出脉冲) / stop(终止)
    pub action: String,
    /// 路由目标区块 BlockId
    pub target: Option<BlockId>,
    /// 此分支执行时设置到脉冲缓存的变量
    pub set_variables: HashMap<String, String>,
}

/// 计划区块子目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSubgoal {
    /// 子目标名称
    pub name: String,
    /// 子目标描述
    pub description: String,
    /// 关联的计划/规则/操作区块 BlockId
    pub block_ref: Option<BlockId>,
    /// 子目标依赖（必须先完成哪些子目标）
    pub dependencies: Vec<String>,
}

/// 计划区块备用方案
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPlan {
    /// 触发条件（如 "step3 fails"）
    pub trigger_condition: String,
    /// 备用方案描述
    pub alternative_action: String,
}

/// 操作区块操作类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum OperationType {
    Internal,
    External,
}

/// 抽象区块领域映射
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainMapping {
    /// 具体领域标识（如 "math.arithmetic.addition"）
    pub domain: String,
    /// 抽象变量 → 具体领域变量的映射
    pub variable_instantiation: HashMap<String, String>,
}

// ============================================================================
// BlockState —— 区块动态状态（激活强度 / 阈值 / 不应期）
// ============================================================================
// 这是区块每次推理时会变化的部分，与静态的 BlockContent 分开存储。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockState {
    /// 当前激活强度（uint8 0~255，脉冲累加/累减得到）
    pub activation: u8,

    /// 激活阈值：activation ≥ threshold 时区块被激活
    pub threshold: u8,

    /// 不应期计数器：区块激活后进入不应期，每次衰减 -1
    /// 0 表示不在不应期，可以响应新脉冲
    pub refractory: u8,
}

impl BlockState {
    pub fn new(threshold: u8) -> Self {
        Self {
            activation: 0,
            threshold,
            refractory: 0,
        }
    }

    /// 累加激活强度（Activate / Result 脉冲）
    pub fn accumulate(&mut self, strength: u8) {
        self.activation = sat8_add(self.activation, strength);
    }

    /// 累减激活强度（Inhibit 脉冲）
    pub fn decay(&mut self, strength: u8) {
        self.activation = sat8_sub(self.activation, strength);
    }

    /// 每步的自然衰减（ACTIVATION_DECAY）
    pub fn natural_decay(&mut self) {
        self.activation = sat8_sub(self.activation, ACTIVATION_DECAY);
    }

    /// 检查是否达到激活阈值
    pub fn is_activated(&self) -> bool {
        self.activation >= self.threshold
    }

    /// 激活后设置不应期
    pub fn enter_refractory(&mut self) {
        self.refractory = crate::value::REFRACTORY_PERIOD;
    }

    /// 每步检查是否在不应期（返回 true 表示在不应期）
    pub fn tick_refractory(&mut self) -> bool {
        if self.refractory > 0 {
            self.refractory -= 1;
            true
        } else {
            false
        }
    }

    /// 完全重置状态（推理开始前清零）
    pub fn reset(&mut self) {
        self.activation = 0;
        self.refractory = 0;
    }
}

// ============================================================================
// Block —— 区块主结构体（MBN 节点）
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// 区块唯一 ID（分层域名格式）
    pub id: BlockId,

    /// 区块类型（9 种之一）
    pub block_type: BlockType,

    /// 区块所属领域（如 "math.arithmetic"）
    pub domain: String,

    /// 嵌套层级：0=原子级（如事实区块），高层=领域级/概念级
    pub level: u32,

    /// 标签列表（用于快速索引，如 ["addition", "single_digit", "table"]）
    pub tags: Vec<String>,

    /// 置信度（uint8 0~255，来源可靠性/确定性程度）
    pub confidence: u8,

    /// 被激活次数（用于学习：高频区块 Hebbian 加强 + 修剪低频区块）
    pub usage_count: u64,

    /// 最后使用的计步器步数
    pub last_used_step: u64,

    /// 区块静态内容（按类型不同结构不同）
    pub content: BlockContent,

    /// 区块动态状态（激活强度/阈值/不应期）
    pub state: BlockState,
    // 注意：Connection 不再存储在 Block 内部，
    // 全部连接由 mesba_network::MeshBlockNetwork 集中管理，
    // 并维护双向邻接索引实现 O(1) 邻居查找。
}

impl Block {
    // -----------------------------------------------------------------------
    // 构造函数 —— 每种类型提供工厂方法
    // -----------------------------------------------------------------------

    /// 创建通用区块（完整参数）
    #[allow(clippy::too_many_arguments)]  // 构造函数参数多是合理的设计
    pub fn new(
        id: BlockId,
        block_type: BlockType,
        domain: String,
        level: u32,
        content: BlockContent,
    ) -> Self {
        let threshold = block_type.default_threshold();
        Self {
            id,
            block_type,
            domain,
            level,
            tags: Vec::new(),
            confidence: 255,
            usage_count: 0,
            last_used_step: 0,
            content,
            state: BlockState::new(threshold),
        }
    }

    // ---- 9 种区块工厂方法 ----

    /// 创建事实区块（最常用的区块类型）
    pub fn fact(id: BlockId, domain: String, level: u32, entries: Vec<FactEntry>) -> Self {
        Self::new(
            id,
            BlockType::Fact,
            domain,
            level,
            BlockContent::Fact { entries },
        )
    }

    /// 创建规则区块
    pub fn rule(
        id: BlockId,
        domain: String,
        level: u32,
        name: String,
        preconditions: Vec<String>,
        steps: Vec<RuleStep>,
    ) -> Self {
        Self::new(
            id,
            BlockType::Rule,
            domain,
            level,
            BlockContent::Rule { name, preconditions, steps },
        )
    }

    /// 创建判断区块
    pub fn judgment(
        id: BlockId,
        domain: String,
        level: u32,
        condition_expr: String,
        variables: Vec<String>,
        true_branch: JudgmentBranch,
        false_branch: JudgmentBranch,
    ) -> Self {
        Self::new(
            id,
            BlockType::Judgment,
            domain,
            level,
            BlockContent::Judgment {
                condition_expr,
                variables,
                true_branch,
                false_branch,
            },
        )
    }

    /// 创建概念区块
    pub fn concept(
        id: BlockId,
        domain: String,
        level: u32,
        definition: String,
    ) -> Self {
        Self::new(
            id,
            BlockType::Concept,
            domain,
            level,
            BlockContent::Concept {
                definition,
                properties: Vec::new(),
                related_concepts: Vec::new(),
            },
        )
    }

    /// 创建感知区块
    pub fn perceptual(
        id: BlockId,
        domain: String,
        level: u32,
        input_type: String,
        patterns: Vec<PerceptualPattern>,
    ) -> Self {
        Self::new(
            id,
            BlockType::Perceptual,
            domain,
            level,
            BlockContent::Perceptual { input_type, patterns },
        )
    }

    /// 创建操作区块（内部算法型）
    pub fn operation_internal(
        id: BlockId,
        domain: String,
        level: u32,
        description: String,
        input_variables: Vec<String>,
        output_variables: Vec<String>,
        algorithm: String,
    ) -> Self {
        Self::new(
            id,
            BlockType::Operation,
            domain,
            level,
            BlockContent::Operation {
                description,
                op_type: OperationType::Internal,
                input_variables,
                output_variables,
                algorithm,
                api_endpoint: String::new(),
                timeout_ms: 100,
                retry_count: 0,
            },
        )
    }

    /// 创建计划区块
    pub fn plan(
        id: BlockId,
        domain: String,
        level: u32,
        name: String,
        subgoals: Vec<PlanSubgoal>,
    ) -> Self {
        let estimated_steps = subgoals.len() as u32;
        Self::new(
            id,
            BlockType::Plan,
            domain,
            level,
            BlockContent::Plan {
                name,
                subgoals,
                backup_plans: Vec::new(),
                estimated_steps,
            },
        )
    }

    /// 创建经验区块
    pub fn experience(
        id: BlockId,
        domain: String,
        level: u32,
        situation: String,
        lesson: String,
    ) -> Self {
        Self::new(
            id,
            BlockType::Experience,
            domain,
            level,
            BlockContent::Experience {
                situation,
                attempt_summary: String::new(),
                error_type: String::new(),
                correct_result: String::new(),
                lesson,
                preventive_block: None,
            },
        )
    }

    /// 创建抽象区块
    pub fn abstract_block(
        id: BlockId,
        domain: String,
        level: u32,
        name: String,
        pattern_steps: Vec<String>,
        abstract_variables: Vec<String>,
    ) -> Self {
        Self::new(
            id,
            BlockType::Abstract,
            domain,
            level,
            BlockContent::Abstract {
                name,
                pattern_steps,
                abstract_variables,
                domain_mappings: Vec::new(),
                migration_condition: String::new(),
            },
        )
    }

    // -----------------------------------------------------------------------
    // 激活状态管理（区块自身状态更新）
    // 注意：连接管理方法（add_connection / outgoing / incoming）
    // 已迁移至 mesba_network::MeshBlockNetwork 统一处理，
    // 以支持 O(1) 邻接查找和 700B 目标的可扩展性
    // -----------------------------------------------------------------------

    /// 激活区块（记录使用次数 + 设置不应期）
    pub fn mark_activated(&mut self, step: u64) {
        self.usage_count += 1;
        self.last_used_step = step;
        self.state.enter_refractory();
    }

    // ---- 事实区块专用方法 ----

    /// 在事实区块中按 Key 查找 Value（精确匹配）
    /// 返回找到的第一个 FactValue，若多条目匹配则返回第一个
    pub fn fact_lookup(&self, key: &str) -> Option<&FactValue> {
        match &self.content {
            BlockContent::Fact { entries } => {
                entries.iter().find_map(|entry| {
                    if entry.keys.iter().any(|k| k == key) {
                        Some(&entry.value)
                    } else {
                        None
                    }
                })
            }
            _ => None,
        }
    }

    /// 在事实区块中按 Key 查找并返回整数值（常用便捷方法）
    pub fn fact_lookup_int(&self, key: &str) -> Option<i32> {
        self.fact_lookup(key).and_then(|v| match v {
            FactValue::Integer(n) => Some(*n),
            _ => None,
        })
    }

    /// 向事实区块添加新的 KV 条目
    pub fn fact_add_entry(&mut self, keys: Vec<String>, value: FactValue) {
        if let BlockContent::Fact { entries } = &mut self.content {
            entries.push(FactEntry { keys, value });
        }
    }

    // ---- 规则区块专用方法 ----

    /// 获取规则区块的步骤序列
    pub fn rule_steps(&self) -> Option<&[RuleStep]> {
        match &self.content {
            BlockContent::Rule { steps, .. } => Some(steps),
            _ => None,
        }
    }

    // ---- 判断区块专用方法 ----

    /// 获取判断区块的分支
    pub fn judgment_branches(&self) -> Option<(&JudgmentBranch, &JudgmentBranch)> {
        match &self.content {
            BlockContent::Judgment {
                true_branch,
                false_branch,
                ..
            } => Some((true_branch, false_branch)),
            _ => None,
        }
    }
}

// ============================================================================
// BlockContent::read() —— 区块被激活后读出内容的核心方法
// ============================================================================
// 设计文档 §6.3 伪代码第 5 步：`result = target.read(pulse.payload)`
// 这是整个 MesBA 推理引擎的核心执行点——脉冲传播到区块，区块达到阈值，
// 然后通过 read() 把区块的静态内容"读出来"变成结构化结果 ReadResult，
// 供上层（PulseBuffer 写入 / 分支路由 / 游离机制执行）消费。
//
// 参数：
//   - pulse_payload : 触发此次激活的脉冲携带的原始字节载荷
//                     对 Fact 来说是查表 key 的 UTF-8 编码
//                     对 Operation 来说可以是操作数（如果不是从 buffer_vars 取）
//                     对 Judgment/Rule/Plan 来说通常被忽略（它们从 buffer_vars 取）
//   - buffer_vars   : 当前 PulseBuffer 中的变量名 → int32 值映射
//                     Judgment 条件求值、Operation 参数都从这里取
//
// 返回：ReadResult 枚举，Propagator 根据变体决定后续处理：
//   Value     → 写入 PulseBuffer
//   Branch    → 生成到 target 的 Result 脉冲（带分支变量）
//   Steps     → 交给游离机制 Rule 执行器
//   Plan      → 交给游离机制 Plan 调度器
//   Trigger   → 对 Trigger 列表中每个 Concept 注入激活脉冲
//   Properties → 通常用于日志 / 概念激活
//   None      → 不产生任何结果，脉冲传播到此为止
impl BlockContent {
    /// 从 PulseBuffer 的当前变量表中安全读取 i32 值
    fn var_get(buffer_vars: &HashMap<String, i32>, name: &str) -> i32 {
        buffer_vars.get(name).copied().unwrap_or(0)
    }

    /// 把 pulse_payload 转为 UTF-8 字符串（失败则空串）
    fn payload_to_str(pulse_payload: &[u8]) -> String {
        String::from_utf8_lossy(pulse_payload).trim().to_string()
    }

    /// 把 pulse_payload 解析为 i32（支持纯数字或以数字结尾的字符串）
    fn payload_to_int(pulse_payload: &[u8]) -> i32 {
        let s = Self::payload_to_str(pulse_payload);
        // 尝试整体解析为整数
        if let Ok(v) = s.parse::<i32>() {
            return v;
        }
        // 尝试提取最后的数字段（如 "result:18" → 18）
        if let Some(digits) = s.chars().rev().find(|c| c.is_ascii_digit()) {
            // 从末尾往前找完整数字段
            let end = s.len() - (s.chars().count() - s.chars().position(|c| c == digits).unwrap_or(s.len()));
            let start = s[..end].char_indices().rev().find(|(_, c)| !c.is_ascii_digit()).map(|(i, _)| i + 1).unwrap_or(0);
            if let Ok(v) = s[start..end].parse::<i32>() {
                return v;
            }
        }
        0
    }

    // ------------------------------------------------------------------------
    // Fact 查表：把 payload 当 key，在 entries 里找匹配，返回 Value
    // 若 payload 为空 → 自动从 buffer_vars 里取 a + b 拼 "a+b" 格式 key
    // ------------------------------------------------------------------------
    fn read_fact(
        &self,
        pulse_payload: &[u8],
        buffer_vars: &HashMap<String, i32>,
    ) -> ReadResult {
        if let BlockContent::Fact { entries } = self {
            // 1) 优先用 payload 解码成 key
            let explicit_key = Self::payload_to_str(pulse_payload);
            let candidates: Vec<String> = if !explicit_key.is_empty() {
                vec![explicit_key]
            } else {
                // 2) payload 空 → 自动从 buffer_vars 拼 key（支持多种模板）
                let mut keys = Vec::new();
                if let (Some(&a), Some(&b)) = (buffer_vars.get("a"), buffer_vars.get("b")) {
                    keys.push(format!("{a}+{b}"));
                    keys.push(format!("{b}+{a}"));
                }
                if let (Some(&a), Some(&b), Some(&c)) = (
                    buffer_vars.get("a"),
                    buffer_vars.get("b"),
                    buffer_vars.get("carry"),
                ) {
                    keys.push(format!("{a}+{b}+{c}"));
                }
                if let Some(&carry) = buffer_vars.get("carry") {
                    // 单变量 key（如 carry 相关）
                    keys.push(format!("carry={carry}"));
                }
                keys
            };

            for key in &candidates {
                // 精确匹配
                for entry in entries {
                    if entry.keys.iter().any(|k| k == key) {
                        return Self::fact_value_to_read_result(&entry.value);
                    }
                }
                // 不区分大小写匹配
                let key_lower = key.to_lowercase();
                for entry in entries {
                    if entry.keys.iter().any(|k| k.to_lowercase() == key_lower) {
                        return Self::fact_value_to_read_result(&entry.value);
                    }
                }
            }
        }
        ReadResult::None
    }

    fn fact_value_to_read_result(v: &FactValue) -> ReadResult {
        match v {
            FactValue::Integer(i) => ReadResult::Value {
                int_value: *i,
                text_value: None,
                bool_value: None,
                output_var_name: None,
            },
            FactValue::Text(t) => ReadResult::Value {
                int_value: 0,
                text_value: Some(t.clone()),
                bool_value: None,
                output_var_name: None,
            },
            FactValue::Boolean(b) => ReadResult::Value {
                int_value: if *b { 1 } else { 0 },
                text_value: None,
                bool_value: Some(*b),
                output_var_name: None,
            },
        }
    }

    // ------------------------------------------------------------------------
    // Judgment 条件求值：解析 condition_expr，从 buffer_vars 取变量，
    // 求值后路由到 true_branch 或 false_branch
    // ------------------------------------------------------------------------
    fn read_judgment(
        &self,
        buffer_vars: &HashMap<String, i32>,
    ) -> ReadResult {
        if let BlockContent::Judgment {
            condition_expr,
            true_branch,
            false_branch,
            ..
        } = self
        {
            let result = Self::evaluate_condition(condition_expr, buffer_vars);
            let (taken, alt) = if result {
                (true_branch, false_branch)
            } else {
                (false_branch, true_branch)
            };
            ReadResult::Branch {
                target: taken.target.clone(),
                set_variables: taken.set_variables.clone(),
                is_true_branch: result,
                alt_branch_target: alt.target.clone(),
            }
        } else {
            ReadResult::None
        }
    }

    /// 条件表达式求值器（支持 MesBA MVP 所需的全部模式）
    /// 支持的表达式模式：
    ///   var >= int        → 单变量比较
    ///   var > int
    ///   var <= int
    ///   var < int
    ///   var == int
    ///   var != int
    ///   var1 + var2 >= int → 两变量加法后比较
    ///   var1 + var2 + var3 >= int → 多变量求和后比较
    ///   var1 - var2 >= int → 两变量减法后比较
    ///   int >= int        → 纯常量比较（主要用于测试）
    fn evaluate_condition(expr: &str, buffer_vars: &HashMap<String, i32>) -> bool {
        let expr = expr.trim();
        // 提取操作符（按优先级：>= <= == != > <）
        let ops: &[&str] = &[">=", "<=", "!=", "==", ">", "<"];
        for op in ops {
            if let Some((left, right)) = expr.split_once(op) {
                let left_val = Self::eval_arith(left.trim(), buffer_vars);
                let right_val = Self::eval_arith(right.trim(), buffer_vars);
                return match *op {
                    ">=" => left_val >= right_val,
                    "<=" => left_val <= right_val,
                    "!=" => left_val != right_val,
                    "==" => left_val == right_val,
                    ">" => left_val > right_val,
                    "<" => left_val < right_val,
                    _ => false,
                };
            }
        }
        // 无操作符：尝试解析为单个布尔变量或 0/1
        Self::eval_arith(expr, buffer_vars) != 0
    }

    /// 递归下降表达式求值器（支持链式/嵌套 + - * / %，标准优先级）
    ///
    /// 优先级：+ - 最低（最先切分，从右往左找保证左结合）→ * / % 更高
    ///
    /// 例：
    ///   "a%10+b%10"              → (a%10) + (b%10)           = 3 + 5 = 8
    ///   "a/10+b/10+carry"        → ((a/10) + (b/10)) + carry
    ///   "tens_sum*10+ones_sum-carry*10" → ((tens*10) + ones) - (carry*10)
    ///   "123"                    → 123（纯数字）
    ///   "a"                      → vars["a"]（纯变量）
    ///
    /// 设计文档 §12 数值规范：全系统零浮点 + 饱和算术 + 整数除法
    /// ★ F-10 导出为 pub：供 mesba_runtime::execution.rs 的 InternalExecutor 调用
    pub fn eval_arith(expr: &str, buffer_vars: &HashMap<String, i32>) -> i32 {
        let s = expr.trim();
        if s.is_empty() {
            return 0;
        }

        // ---- 优先级最低：+ / -（从右往左找，保证左结合）----
        let bytes = s.as_bytes();
        let mut i = bytes.len();
        while i > 0 {
            i -= 1;
            let ch = bytes[i] as char;
            if ch == '+' || ch == '-' {
                // 跳过最前面的负号（如 "-a+b" 中位置 0 的 '-'，整个表达式变负）
                if i == 0 {
                    continue;
                }
                // 跳过连续操作符（如 "a+-b" 中 '+' 后面的 '-'）
                if i > 0 {
                    let prev = bytes[i - 1] as char;
                    if prev == '+' || prev == '-' || prev == '*' || prev == '/' || prev == '%' {
                        continue;
                    }
                }
                let left = Self::eval_arith(&s[..i], buffer_vars);
                let right = Self::eval_arith(&s[i + 1..], buffer_vars);
                return if ch == '+' {
                    left.saturating_add(right)
                } else {
                    left.saturating_sub(right)
                };
            }
        }

        // ---- 优先级更高：* / % ----
        let mut i = bytes.len();
        while i > 0 {
            i -= 1;
            let ch = bytes[i] as char;
            if ch == '*' || ch == '/' || ch == '%' {
                let left = Self::eval_arith(&s[..i], buffer_vars);
                let right = Self::eval_arith(&s[i + 1..], buffer_vars);
                return match ch {
                    '*' => left.saturating_mul(right),
                    '/' => {
                        if right == 0 {
                            0 // 除零保护
                        } else {
                            left / right
                        }
                    }
                    '%' => {
                        if right == 0 {
                            0 // 取模零保护
                        } else {
                            left % right
                        }
                    }
                    _ => left,
                };
            }
        }

        // ---- 无操作符：纯变量名或纯数字 ----
        Self::resolve_operand(s, buffer_vars)
    }

    /// 解析操作数：先尝试整数常量，再查 buffer_vars
    fn resolve_operand(
        operand: &str,
        buffer_vars: &HashMap<String, i32>,
    ) -> i32 {
        if let Ok(v) = operand.parse::<i32>() {
            return v;
        }
        Self::var_get(buffer_vars, operand)
    }

    // ------------------------------------------------------------------------
    // Operation 算法执行：解析 algorithm 字符串并执行
    // ------------------------------------------------------------------------
    fn read_operation(
        &self,
        pulse_payload: &[u8],
        buffer_vars: &HashMap<String, i32>,
    ) -> ReadResult {
        if let BlockContent::Operation {
            algorithm,
            input_variables,
            output_variables,
            ..
        } = self
        {
            let result = Self::execute_algorithm(algorithm, input_variables, buffer_vars, pulse_payload);
            // output_var_name 声明了 "这个计算结果应该存入 PulseBuffer 的哪个变量"
            let output_var = output_variables.first().cloned();
            ReadResult::Value {
                int_value: result,
                text_value: None,
                bool_value: None,
                output_var_name: output_var,
            }
        } else {
            ReadResult::None
        }
    }

    /// 执行内部操作算法（支持 MesBA MVP 所需的全部计算模式）
    /// 支持的算法字符串格式：
    ///   算术运算（从 input_variables 依次取操作数）：
    ///     "a+b"         → 加法（两个输入）
    ///     "a-b"         → 减法
    ///     "a*b"         → 乘法
    ///     "a/b"         → 除法（整数除法，除零返回 0）
    ///     "a%b"         → 取模
    ///
    ///   单操作数函数式：
    ///     "mod10(x)"    → x % 10（提取个位）
    ///     "div10(x)"    → x / 10（提取十位）
    ///     "abs(x)"      → 绝对值
    ///     "neg(x)"      → 取反
    ///
    ///   多操作数加法链：
    ///     "sum"         → 所有 input_variables 的值求和
    ///
    ///   常量：
    ///     整数字面量     → 直接返回
    fn execute_algorithm(
        algorithm: &str,
        input_variables: &[String],
        buffer_vars: &HashMap<String, i32>,
        pulse_payload: &[u8],
    ) -> i32 {
        let alg = algorithm.trim();

        // ---- 函数式：name(arg) ----
        if let Some(paren_open) = alg.find('(') {
            let func_name = alg[..paren_open].trim();
            if let Some(paren_close) = alg.rfind(')') {
                let arg_str = alg[paren_open + 1..paren_close].trim();
                let arg_val = Self::resolve_operand(arg_str, buffer_vars);
                return match func_name {
                    "mod10" => arg_val % 10,
                    "div10" => arg_val / 10,
                    "abs" => arg_val.abs(),
                    "neg" => -arg_val,
                    "double" => arg_val.saturating_mul(2),
                    "square" => arg_val.saturating_mul(arg_val),
                    _ => arg_val, // 未知函数 → 返回参数值
                };
            }
        }

        // ---- 特殊关键字 ----
        match alg {
            "sum" | "SUM" => {
                let mut total: i32 = 0;
                for var in input_variables {
                    total = total.saturating_add(Self::var_get(buffer_vars, var));
                }
                return total;
            }
            "payload_int" | "PAYLOAD_INT" => return Self::payload_to_int(pulse_payload),
            "payload_str" | "PAYLOAD_STR" => return 0, // 字符串不可转 int
            _ => {}
        }

        // ---- 递归下降求值（支持链式/嵌套 + - * / %，标准优先级）----
        Self::eval_arith(alg, buffer_vars)
    }

    // ------------------------------------------------------------------------
    // 主入口：根据 BlockContent 类型分发到对应的 read_* 方法
    // ------------------------------------------------------------------------
    pub fn read(
        &self,
        pulse_payload: &[u8],
        buffer_vars: &HashMap<String, i32>,
    ) -> ReadResult {
        match self {
            // Fact：查表返回值（支持从 buffer_vars 自动拼 key）
            BlockContent::Fact { .. } => {
                self.read_fact(pulse_payload, buffer_vars)
            }

            // Judgment：条件求值 + 分支路由
            BlockContent::Judgment { .. } => self.read_judgment(buffer_vars),

            // Operation：执行算法
            BlockContent::Operation { .. } => {
                self.read_operation(pulse_payload, buffer_vars)
            }

            // Rule：读出步骤序列（交给游离机制依次执行）
            BlockContent::Rule { steps, .. } => ReadResult::Steps(steps.clone()),

            // Plan：读出子目标列表
            BlockContent::Plan { subgoals, .. } => ReadResult::Plan(subgoals.clone()),

            // Concept：读出定义 + 属性 + 相关概念
            BlockContent::Concept {
                definition,
                properties,
                related_concepts,
            } => {
                let mut all: Vec<String> = Vec::with_capacity(
                    1 + properties.len() + related_concepts.len(),
                );
                all.push(definition.clone());
                all.extend(properties.iter().cloned());
                for cid in related_concepts {
                    all.push(cid.as_str().to_string());
                }
                ReadResult::Properties(all)
            }

            // Perceptual：读出特征模式（Trigger 列表留空，实际触发由游离机制完成）
            BlockContent::Perceptual { patterns, .. } => {
                let props: Vec<String> = patterns
                    .iter()
                    .map(|p| format!("{}::{}", p.pattern, p.output_type))
                    .collect();
                ReadResult::Properties(props)
            }

            // Experience：读出教训 + 建议
            BlockContent::Experience {
                situation,
                attempt_summary,
                error_type,
                lesson,
                correct_result,
                ..
            } => {
                let all = vec![
                    format!("situation:{}", situation),
                    format!("attempt:{}", attempt_summary),
                    format!("error_type:{}", error_type),
                    format!("lesson:{}", lesson),
                    format!("correct_result:{}", correct_result),
                ];
                ReadResult::Properties(all)
            }

            // Abstract：读出模式骨架
            BlockContent::Abstract {
                pattern_steps,
                abstract_variables,
                migration_condition,
                ..
            } => {
                let mut all: Vec<String> = pattern_steps.clone();
                all.extend(abstract_variables.iter().map(|v| format!("var:{}", v)));
                all.push(format!("migration:{}", migration_condition));
                ReadResult::Properties(all)
            }
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fact_block_creation() {
        let mut fact = Block::fact(
            BlockId::new("Math.Arithmetic.Fact.Add_9_9"),
            "math.arithmetic".to_string(),
            0,
            vec![FactEntry {
                keys: vec!["9+9".to_string(), "九加九".to_string()],
                value: FactValue::Integer(18),
            }],
        );
        assert_eq!(fact.block_type, BlockType::Fact);
        assert_eq!(fact.confidence, 255);
        assert_eq!(fact.state.threshold, BlockType::Fact.default_threshold());

        // 精确匹配查询
        assert_eq!(fact.fact_lookup_int("9+9"), Some(18));
        assert_eq!(fact.fact_lookup_int("九加九"), Some(18));
        assert_eq!(fact.fact_lookup_int("unknown"), None);

        // 添加新条目
        fact.fact_add_entry(vec!["9+9+0".to_string()], FactValue::Integer(18));
        assert!(matches!(&fact.content, BlockContent::Fact { entries } if entries.len() == 2));
    }

    #[test]
    fn test_block_state() {
        let mut block = Block::concept(
            BlockId::new("Math.Arithmetic.Concept.Addition"),
            "math.arithmetic".to_string(),
            2,
            "将两个或多个数合并成一个数的运算".to_string(),
        );
        assert_eq!(block.state.activation, 0);
        assert_eq!(block.state.refractory, 0);

        // 脉冲累加
        block.state.accumulate(100);
        assert_eq!(block.state.activation, 100);

        // 饱和累加
        block.state.accumulate(200);  // 100 + 200 = 300 > 255
        assert_eq!(block.state.activation, 255);

        // 阈值检查
        assert!(block.state.is_activated());  // 255 >= 80

        // 激活后不应期
        block.mark_activated(1);
        assert_eq!(block.state.refractory, crate::value::REFRACTORY_PERIOD);
        assert!(block.state.tick_refractory());
        assert_eq!(block.state.refractory, crate::value::REFRACTORY_PERIOD - 1);
    }

    // 注：test_connection_management 已移除
    // 连接管理职责已迁移至 mesba_network::MeshBlockNetwork，
    // 相关测试将在该 crate 中实现

    #[test]
    fn test_all_nine_block_types() {
        // 验证 9 种区块工厂方法都能正确创建
        let perceptual = Block::perceptual(
            BlockId::new("Test.Perceptual.Digit"),
            "test".to_string(),
            0,
            "text_token".to_string(),
            vec![],
        );
        assert_eq!(perceptual.block_type, BlockType::Perceptual);

        let concept = Block::concept(
            BlockId::new("Test.Concept.Number"),
            "test".to_string(),
            2,
            "数字的概念".to_string(),
        );
        assert_eq!(concept.block_type, BlockType::Concept);

        let rule = Block::rule(
            BlockId::new("Test.Rule.Strategy"),
            "test".to_string(),
            1,
            "策略名称".to_string(),
            vec!["前置条件".to_string()],
            vec![],
        );
        assert_eq!(rule.block_type, BlockType::Rule);

        let judgment = Block::judgment(
            BlockId::new("Test.Judge.GreaterThan"),
            "test".to_string(),
            0,
            "a >= b".to_string(),
            vec!["a".to_string(), "b".to_string()],
            JudgmentBranch {
                action: "route".to_string(),
                target: Some(BlockId::new("Test.Op.A")),
                set_variables: HashMap::new(),
            },
            JudgmentBranch {
                action: "route".to_string(),
                target: Some(BlockId::new("Test.Op.B")),
                set_variables: HashMap::new(),
            },
        );
        assert_eq!(judgment.block_type, BlockType::Judgment);

        let operation = Block::operation_internal(
            BlockId::new("Test.Op.Calc"),
            "test".to_string(),
            0,
            "计算描述".to_string(),
            vec!["a".to_string()],
            vec!["result".to_string()],
            "算法文本描述".to_string(),
        );
        assert_eq!(operation.block_type, BlockType::Operation);

        let plan = Block::plan(
            BlockId::new("Test.Plan.Main"),
            "test".to_string(),
            3,
            "主计划".to_string(),
            vec![],
        );
        assert_eq!(plan.block_type, BlockType::Plan);

        let experience = Block::experience(
            BlockId::new("Test.Exp.Fail001"),
            "test".to_string(),
            1,
            "失败情境".to_string(),
            "教训内容".to_string(),
        );
        assert_eq!(experience.block_type, BlockType::Experience);

        let abstract_block = Block::abstract_block(
            BlockId::new("Test.Abstract.Pattern"),
            "test.abstract".to_string(),
            4,
            "通用模式".to_string(),
            vec!["步骤1".to_string()],
            vec!["变量x".to_string()],
        );
        assert_eq!(abstract_block.block_type, BlockType::Abstract);
    }
}
