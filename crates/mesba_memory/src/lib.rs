// ============================================================================
// mesba_memory —— MesBA 三级记忆系统
// ============================================================================
// 设计文档 §7 三级记忆：
//   L0 PerceptualBuffer —— 感知缓冲区（瞬时记忆，7±2 槽位）
//   L1 GWS（本 crate 核心）：
//     PulseBuffer       —— 脉冲缓存（工作记忆变量表，20 槽位 + Fixed/Dynamic）
//     GoalStack         —— 子目标栈（深度 ≤ 7 + checkpoint 回滚）
//     GlobalWorkspace   —— 聚合所有子模块 + impl MemoryBackend trait
//   L2 MBN              —— 网状区块网络（mesba_network crate）
//
// mesba_memory 依赖 mesba_core（trait + 基础类型），不依赖 mesba_engine，
// 让 mesba_engine::PulsePropagator<GlobalWorkspace> 能在无循环依赖的情况下组合。
//
// 设计约束：lib.rs 只做模块引入 + re-export，不实现业务逻辑
// ============================================================================

pub mod perceptual_buffer;
pub mod pulse_buffer;
pub mod goal_stack;
pub mod episodic;
pub mod gws;

// 重新导出核心类型，方便下游 crate（mesba_engine / mesba_free / mesba_runtime）使用
pub use perceptual_buffer::{Modality, PerceptualBuffer, PerceptualSlot, DEFAULT_CAPACITY};
pub use pulse_buffer::{PulseBuffer, PulseSlot, SlotType};
pub use goal_stack::{GoalStack, SubGoal, Checkpoint, Condition, Completion, MAX_DEPTH};
pub use episodic::EpisodicMemory;
pub use gws::{GlobalWorkspace, SituationModel};