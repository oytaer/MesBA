// ============================================================================
// mesba_core —— Mesh Block Architecture 核心数据结构 crate
// ============================================================================
// 本 crate 是 MesBA 整个项目的基石，定义了所有其他 crate 依赖的基础类型：
//
//   integer_ops —— 整数运算核心（sat8_add / sat8_sub / uint8_multiply / sat32_add / 整数除法）
//   value       —— Q8.8 定点小数常量 / 全局参数 / 量化辅助函数
//   id          —— BlockId / ConnectionId / PulseId 类型（类型安全的标识符）
//   block_types —— BlockType (9种) / ConnectionType (5种) / PulseType (5种) 枚举
//   block       —— Block / BlockState / BlockContent / Block 9 种工厂方法
//   connection  —— Connection（区块间连接，含权重/衰减/门控/Hebbian学习）
//   pulse       —— Pulse（脉冲信号载体，涟漪传播的核心）
//   buffer      —— Buffer / BufferSlot / BufferSnapshot（脉冲缓存/工作记忆）
//   lut         —— 256项查找表（sigmoid / exp / softmax，零浮点O(1)查表）
//   kernel      —— Layer 1 热点 Kernel（SIMD加速的sat8批量加法）
//
// 设计约束：lib.rs 只做模块引入，不实现业务逻辑
// ============================================================================

pub mod integer_ops;
pub mod value;
pub mod id;
pub mod block_types;
pub mod block;
pub mod connection;
pub mod pulse;
pub mod buffer;
pub mod lut;
pub mod kernel;
pub mod memory_backend;

// 重新导出最常用的类型，方便下游 crate 使用
pub use block::{Block, BlockContent, BlockState, ReadResult, FactEntry, FactValue,
                RuleStep, JudgmentBranch, PlanSubgoal, BackupPlan,
                PerceptualPattern, OperationType, DomainMapping};
pub use block_types::{BlockType, ConnectionType, PulseType};
pub use buffer::{Buffer, BufferSlot, BufferSnapshot};
pub use connection::Connection;
pub use id::{BlockId, ConnectionId, PulseId};
pub use integer_ops::{sat8_add, sat8_sub, uint8_multiply, sat32_add, uint8_divide, sqrt_u32_newton};
pub use memory_backend::{MemoryBackend, TraceEntry};
pub use pulse::Pulse;
pub use value::{MIN_SIGNAL, ACTIVATION_DECAY, REFRACTORY_PERIOD,
                DEFAULT_TTL, MAX_PULSES_PER_STEP,
                SYSTEM1_MAX_STEPS, SYSTEM2_MAX_STEPS};
