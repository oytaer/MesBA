// ============================================================================
// execution.rs —— 执行接口（§3.1 / 图 2.1）
// ============================================================================
// 设计文档：
//   ┌──────────────────────┐           ┌──────────────────────┐
//   │   执行接口            │           │   反馈系统            │
//   │ Execution Interface  │──────────▶│ Feedback System      │
//   │ · 操作区块调用        │  执行动作  │ · 结果观察           │
//   │ · 外部工具API        │           │ · 误差计算           │
//   │ · 环境交互           │           │ · 奖励/惩罚信号       │
//   │ · 动作序列编排        │           │ · 错误归因           │
//   └──────────────────────┘           └──────────────────────┘
//
// 核心职责：
//   1. ExecutionInterface trait —— 统一执行接口（execute() 返回 Result<i32, String>）
//   2. InternalExecutor —— 内置实现：映射 Operation block 的 algorithm 字符串 →
//      mesba_core::Block::eval_arith() 求值（§12 饱和算术 + 整数除法）
//   3. ExternalExecutor stub —— 为未来 HTTP API / 工具调用预留（MVP 阶段只留 trait）
//   4. ExecutionHandle —— 超时跟踪（F-19 遗留：step 级超时）
// ============================================================================

use std::collections::HashMap;

use mesba_core::{Block, BlockContent, BlockType};
use serde::{Deserialize, Serialize};

// ============================================================================
// ExecutionError —— 执行错误类型
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionError {
    /// Operation 区块缺失（network 里找不到）
    BlockNotFound { block_id: String },
    /// 区块类型不是 Operation
    NotAnOperation { block_type: String },
    /// Operation 区块的 algorithm 字符串为空
    EmptyAlgorithm { block_id: String },
    /// 超时（ExecutionHandle 跟踪 timeout_ms）
    Timeout { elapsed_ms: u64, timeout_ms: u64 },
    /// 外部工具调用失败（ExternalExecutor）
    ExternalToolFailed { tool_name: String, msg: String },
}

impl ExecutionError {
    pub fn short_msg(&self) -> String {
        match self {
            ExecutionError::BlockNotFound { block_id } => {
                format!("block not found: {}", block_id)
            }
            ExecutionError::NotAnOperation { block_type } => {
                format!("not an operation block: {}", block_type)
            }
            ExecutionError::EmptyAlgorithm { block_id } => {
                format!("empty algorithm: {}", block_id)
            }
            ExecutionError::Timeout {
                elapsed_ms,
                timeout_ms,
            } => format!("timeout: {}ms > {}ms", elapsed_ms, timeout_ms),
            ExecutionError::ExternalToolFailed { tool_name, msg } => {
                format!("tool {} failed: {}", tool_name, msg)
            }
        }
    }
}

// ============================================================================
// ExecutionHandle —— step 级超时跟踪（§14.3 F-19 超时机制）
// ============================================================================
#[derive(Debug, Clone)]
pub struct ExecutionHandle {
    /// step 开始时间戳（epoch ms）
    pub started_at_ms: u64,
    /// step 允许的最大执行时间（ms）
    pub timeout_ms: u64,
}

impl ExecutionHandle {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            started_at_ms: crate::api::now_ms(),
            timeout_ms,
        }
    }

    /// 是否已超时
    pub fn is_expired(&self) -> bool {
        let elapsed = crate::api::now_ms().saturating_sub(self.started_at_ms);
        elapsed >= self.timeout_ms
    }

    /// 已消耗 ms
    pub fn elapsed_ms(&self) -> u64 {
        crate::api::now_ms().saturating_sub(self.started_at_ms)
    }
}

// ============================================================================
// ExecutionInterface trait —— 统一执行接口
// ============================================================================
/// 执行接口：给定 Operation block + 输入变量 → 输出整数结果
/// mesba_free::execute::tick() 调用 ExecutionInterface::execute()
pub trait ExecutionInterface {
    fn execute(
        &mut self,
        operation_block: &Block,
        input_vars: &HashMap<String, i32>,
        handle: &ExecutionHandle,
    ) -> Result<i32, ExecutionError>;
}

// ============================================================================
// InternalExecutor —— 内置实现（algorithm 字符串 → eval_arith）
// ============================================================================
/// AdvisorTool 指导：复用 mesba_core::Block::eval_arith（§12 饱和算术）
/// 而非重新实现算术运算
pub struct InternalExecutor {
    /// 默认 step 超时（ms），默认 1000
    pub default_timeout_ms: u64,
}

impl Default for InternalExecutor {
    fn default() -> Self {
        Self {
            default_timeout_ms: 1000,
        }
    }
}

impl InternalExecutor {
    pub fn new(default_timeout_ms: u64) -> Self {
        Self { default_timeout_ms }
    }
}

impl ExecutionInterface for InternalExecutor {
    fn execute(
        &mut self,
        operation_block: &Block,
        input_vars: &HashMap<String, i32>,
        handle: &ExecutionHandle,
    ) -> Result<i32, ExecutionError> {
        // 1) 超时检查
        if handle.is_expired() {
            return Err(ExecutionError::Timeout {
                elapsed_ms: handle.elapsed_ms(),
                timeout_ms: handle.timeout_ms,
            });
        }

        // 2) 类型检查：必须是 Operation
        if operation_block.block_type != BlockType::Operation {
            return Err(ExecutionError::NotAnOperation {
                block_type: format!("{:?}", operation_block.block_type),
            });
        }

        // 3) 提取 algorithm 字符串
        let algorithm = match &operation_block.content {
            BlockContent::Operation { algorithm, .. } => algorithm.clone(),
            _ => return Err(ExecutionError::NotAnOperation {
                block_type: format!("{:?}", operation_block.block_type),
            }),
        };

        if algorithm.is_empty() {
            return Err(ExecutionError::EmptyAlgorithm {
                block_id: operation_block.id.to_string(),
            });
        }

        // 4) ★ 核心：调 mesba_core::BlockContent::eval_arith() 求值
        //    §12 饱和算术 + 整数除法（saturating_add/sub/mul）
        let result = BlockContent::eval_arith(&algorithm, input_vars);
        Ok(result)
    }
}

// ============================================================================
// ExternalExecutor stub —— 外部工具调用占位
// ============================================================================
/// MVP 阶段 ExternalExecutor 是 stub：只返回 ExternalToolFailed
/// 未来接入 HTTP API / WASM 工具 / Python Runtime 时实现具体逻辑
#[derive(Debug, Clone, Default)]
pub struct ExternalExecutorStub {
    /// 已注册的工具列表（未来扩展用）
    pub tool_names: Vec<String>,
}

impl ExecutionInterface for ExternalExecutorStub {
    fn execute(
        &mut self,
        operation_block: &Block,
        _input_vars: &HashMap<String, i32>,
        handle: &ExecutionHandle,
    ) -> Result<i32, ExecutionError> {
        if handle.is_expired() {
            return Err(ExecutionError::Timeout {
                elapsed_ms: handle.elapsed_ms(),
                timeout_ms: handle.timeout_ms,
            });
        }

        // MVP：所有外部工具调用一律返回失败（未实现）
        let tool_name = match &operation_block.content {
            BlockContent::Operation { algorithm, .. } => algorithm.clone(),
            _ => "unknown".into(),
        };
        Err(ExecutionError::ExternalToolFailed {
            tool_name,
            msg: "ExternalExecutor 未实现（MVP 阶段）".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_op_block(algorithm: &str) -> Block {
        Block::new(
            mesba_core::BlockId::from("test.op"),
            BlockType::Operation,
            "test".into(),
            0,
            BlockContent::Operation {
                description: "test op".into(),
                op_type: mesba_core::OperationType::Internal,
                input_variables: vec!["a".into(), "b".into()],
                output_variables: vec!["result".into()],
                algorithm: algorithm.into(),
                api_endpoint: String::new(),
                timeout_ms: 100,
                retry_count: 0,
            },
        )
    }

    #[test]
    fn test_internal_basic_add() {
        let mut exec = InternalExecutor::default();
        let op = make_op_block("a + b");
        let vars: HashMap<String, i32> = [("a".into(), 3), ("b".into(), 5)].into();
        let handle = ExecutionHandle::new(1000);
        assert_eq!(exec.execute(&op, &vars, &handle).unwrap(), 8);
    }

    #[test]
    fn test_internal_complex() {
        let mut exec = InternalExecutor::default();
        let op = make_op_block("a*2 + b - 1");
        let vars: HashMap<String, i32> = [("a".into(), 5), ("b".into(), 3)].into();
        let handle = ExecutionHandle::new(1000);
        assert_eq!(exec.execute(&op, &vars, &handle).unwrap(), 12);
    }

    #[test]
    fn test_internal_not_operation() {
        let mut exec = InternalExecutor::default();
        let concept = Block::new(
            mesba_core::BlockId::from("test.concept"),
            BlockType::Concept,
            "test".into(),
            0,
            BlockContent::Concept {
                definition: "d".into(),
                properties: vec![],
                related_concepts: vec![],
            },
        );
        let vars = HashMap::new();
        let handle = ExecutionHandle::new(1000);
        let err = exec.execute(&concept, &vars, &handle).unwrap_err();
        assert!(matches!(err, ExecutionError::NotAnOperation { .. }));
    }

    #[test]
    fn test_external_stub_always_fails() {
        let mut exec = ExternalExecutorStub::default();
        let op = make_op_block("HTTP_GET /api/test");
        let vars = HashMap::new();
        let handle = ExecutionHandle::new(1000);
        let err = exec.execute(&op, &vars, &handle).unwrap_err();
        assert!(matches!(
            err,
            ExecutionError::ExternalToolFailed { .. }
        ));
    }

    #[test]
    fn test_execution_handle_not_expired() {
        let h = ExecutionHandle::new(10000); // 10s
        assert!(!h.is_expired());
    }

    #[test]
    fn test_overflow_saturates() {
        // eval_arith 用 saturating_add，所以 a+b 在 i32 饱和时不会 panic
        let mut exec = InternalExecutor::default();
        let op = make_op_block("a + b");
        let vars: HashMap<String, i32> = [("a".into(), i32::MAX), ("b".into(), 1)].into();
        let handle = ExecutionHandle::new(1000);
        let result = exec.execute(&op, &vars, &handle).unwrap();
        assert_eq!(result, i32::MAX); // saturating_add
    }
}
