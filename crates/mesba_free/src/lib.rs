// ============================================================================
// mesba_free —— 游离控制系统 + 认知八环状态机
// ============================================================================
// 设计文档 §8：十模块 + 认知八环状态机
// 一个 crate 一个业务模块、一个模块一个 .rs 文件（mod.rs 只做引入）
// ============================================================================

mod attention;
mod controller;
mod emotion;
mod execute;
mod feedback;
mod goal_manager;
mod perceive;
mod plan;
mod reason;
mod state_machine;
mod transfer;
mod understand;

// ============================================================================
// 对外 re-export
// ============================================================================

pub use controller::{FreeMechanismController, PropagationStats};
pub use state_machine::{
    CognitiveRing, COMPLEXITY_THRESHOLD, decide_system, FAMILIAR_THRESHOLD,
    UNCERTAINTY_THRESHOLD,
};

// 以下模块暴露（供 mesba_runtime 构建更复杂 Controller 时参考）
pub use attention::AttentionRegulator;
pub use emotion::Emotion;
pub use execute::Executor;
pub use feedback::Feedback;
pub use goal_manager::GoalManager;
pub use perceive::Perceive;
pub use plan::Planner;
pub use reason::Reason;
pub use transfer::Transfer;
pub use understand::Understand;
