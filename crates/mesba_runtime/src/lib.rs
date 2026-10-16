// ============================================================================
// mesba_runtime —— MesBA 推理运行时
// ============================================================================
// 模块划分（一个模块一个 .rs 文件）：
//   api     —— 统一 JSON API 格式（{code, msg, data, time}）+ InferenceResult
//   runtime —— InferenceRuntime 封装 FreeMechanismController + 对外入口
// ============================================================================

pub mod api;
pub mod execution;
pub mod interfaces;
pub mod runtime;
pub mod safety;
pub mod session;

pub use api::{ApiResponse, CODE_ERROR, CODE_OK, CODE_UNAUTHORIZED, InferenceResult};
pub use execution::{
    ExecutionError, ExecutionHandle, ExecutionInterface, ExternalExecutorStub, InternalExecutor,
};
pub use interfaces::{
    Goal, KnowledgeBase, Learning, LearningReport, MesBA, MultimodalInput, NetworkStats,
    Observability, Schema, ToolInfo, ToolRegistry, ToolRegistryStore,
};
pub use runtime::InferenceRuntime;
pub use safety::{Checkpoint, SecurityGate, SecurityViolation};
pub use session::{SessionManager, SessionState, TaskId, TraceEvent};
