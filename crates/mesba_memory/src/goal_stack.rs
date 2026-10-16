// ============================================================================
// goal_stack.rs —— L1 GWS GoalStack（子目标栈）
// ============================================================================
// 设计文档 §7.3 GoalStack：
//   - 嵌套子目标，深度 ≤ 7（人类短期记忆层级上限）
//   - push: 压入子目标 + 自动 checkpoint 快照 PulseBuffer
//   - rollback: 恢复到指定 checkpoint（错误恢复 / 失败重试）
//   - pop: 子目标完成弹出 + 释放 checkpoint
//   - top: 当前激活的子目标
//
// GoalStack 与 PulseBuffer 协作：每次 push 时快照 PulseBuffer 的全部变量，
// rollback 时把 PulseBuffer 恢复到快照状态。这是 Plan 执行失败回滚的关键机制。
// ============================================================================

use std::collections::HashMap;

/// GoalStack 最大深度（设计文档 §7.3）
pub const MAX_DEPTH: usize = 7;

// ============================================================================
// SubGoal —— 栈中每一层子目标
// ============================================================================
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubGoal {
    /// 子目标名称（自然语言标识）
    pub name: String,
    /// 关联的 Plan 区块 ID（Plan 执行时把所有 SubGoal 压栈）
    pub plan_id: String,
    /// 前置条件（所有条件满足才开始执行此子目标）
    pub conditions: Vec<Condition>,
    /// 完成条件（满足则弹出子目标）
    pub completion: Completion,
    /// 进度（0-255，饱和整数）
    pub progress: u8,
    /// 创建时的 checkpoint 编号（rollback 用）
    pub checkpoint_id: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Condition {
    /// 条件变量名
    pub variable: String,
    /// 条件值（i32）
    pub expected_value: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Completion {
    /// 变量 == 指定值
    VarEquals { variable: String, value: i32 },
    /// 变量 >= 指定值
    VarGe { variable: String, value: i32 },
    /// 变量已存在（非 None）
    VarPresent { variable: String },
    /// 无自动完成条件（由 mesba_free 显式 pop）
    Manual,
}

// ============================================================================
// Checkpoint —— PulseBuffer 快照
// ============================================================================
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub id: u64,
    pub buffer_vars_snapshot: HashMap<String, i32>,
    pub timestamp: u64,
}

// ============================================================================
// GoalStack —— 主体
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct GoalStack {
    /// 子目标栈（栈顶 = 当前执行中）
    stack: Vec<SubGoal>,
    /// checkpoint 池（子目标 push 时自动创建，rollback 时恢复）
    checkpoints: Vec<Checkpoint>,
    /// checkpoint 自增 ID
    next_checkpoint_id: u64,
}

impl GoalStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前深度
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// 是否已空
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// 栈顶（当前激活子目标）
    pub fn top(&self) -> Option<&SubGoal> {
        self.stack.last()
    }

    // ========================================================================
    // 核心操作
    // ========================================================================

    /// 压入子目标（超过 MAX_DEPTH 自动回退最深层目标合并）
    /// 同时记录当前 PulseBuffer 为 checkpoint
    pub fn push(
        &mut self,
        name: String,
        plan_id: String,
        conditions: Vec<Condition>,
        completion: Completion,
        buffer_vars_snapshot: HashMap<String, i32>,
        timestamp: u64,
    ) -> Result<(), String> {
        if self.stack.len() >= MAX_DEPTH {
            // 深度溢出：不报错，但自动把最底层的目标改名为合并版
            let deepest = self.stack.first_mut().unwrap();
            deepest.name = format!("{}+{}", deepest.name, name);
            return Err(format!("GoalStack 深度 {} ≥ MAX_DEPTH {}，自动合并", self.stack.len(), MAX_DEPTH));
        }

        let checkpoint_id = self.next_checkpoint_id;
        self.next_checkpoint_id += 1;

        self.checkpoints.push(Checkpoint {
            id: checkpoint_id,
            buffer_vars_snapshot,
            timestamp,
        });

        self.stack.push(SubGoal {
            name,
            plan_id,
            conditions,
            completion,
            progress: 0,
            checkpoint_id,
        });

        Ok(())
    }

    /// 弹出栈顶（子目标完成 / 失败后清理）
    /// 返回被弹出的 checkpoint（供外部恢复 PulseBuffer）
    pub fn pop(&mut self) -> Option<Checkpoint> {
        let top = self.stack.pop()?;
        // 弹出对应的 checkpoint
        let check_idx = self
            .checkpoints
            .iter()
            .position(|c| c.id == top.checkpoint_id);
        if let Some(idx) = check_idx {
            Some(self.checkpoints.remove(idx))
        } else {
            None
        }
    }

    /// 回退到指定 checkpoint（错误恢复）
    /// 返回恢复用的 buffer_vars_snapshot（mesba_memory 用它覆盖 PulseBuffer）
    pub fn rollback_to(&mut self, checkpoint_id: u64) -> Option<HashMap<String, i32>> {
        // 把栈中所有 checkpoint_id >= 指定值的目标都弹出
        while let Some(top) = self.stack.last() {
            if top.checkpoint_id >= checkpoint_id {
                self.stack.pop();
            } else {
                break;
            }
        }
        // 找到对应 checkpoint 并返回其快照
        let checkpoint_idx = self
            .checkpoints
            .iter()
            .position(|c| c.id == checkpoint_id)?;
        let snapshot = self.checkpoints[checkpoint_idx].buffer_vars_snapshot.clone();
        // 删除所有更晚的 checkpoint
        self.checkpoints.truncate(checkpoint_idx + 1);
        Some(snapshot)
    }

    /// 检查栈顶子目标是否满足完成条件（供 mesba_free 每 tick 调用）
    pub fn check_top_completion(&self, buffer_vars: &HashMap<String, i32>) -> bool {
        let top = match self.top() {
            Some(t) => t,
            None => return false,
        };

        // 先检查前置条件（如果不满足，子目标还不能开始，自然也没完成）
        for cond in &top.conditions {
            match buffer_vars.get(&cond.variable) {
                Some(v) if *v != cond.expected_value => return false,
                None => return false,
                _ => {}
            }
        }

        match &top.completion {
            Completion::VarEquals { variable, value } => {
                buffer_vars.get(variable).copied() == Some(*value)
            }
            Completion::VarGe { variable, value } => {
                buffer_vars.get(variable).copied().unwrap_or(0) >= *value
            }
            Completion::VarPresent { variable } => buffer_vars.contains_key(variable),
            Completion::Manual => false, // 手动完成，外部 pop
        }
    }

    /// 推进栈顶子目标的进度
    pub fn advance_progress(&mut self, delta: u8) {
        if let Some(top) = self.stack.last_mut() {
            top.progress = top.progress.saturating_add(delta);
        }
    }

    /// 清空（新任务开始时）
    pub fn clear(&mut self) {
        self.stack.clear();
        self.checkpoints.clear();
        self.next_checkpoint_id = 0;
    }

    // ========================================================================
    // 调试
    // ========================================================================

    /// 当前栈快照（供 mesba_runtime 可观测性用）
    pub fn snapshot(&self) -> Vec<&SubGoal> {
        self.stack.iter().collect()
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn simple_snapshot(vars: &[(&str, i32)]) -> HashMap<String, i32> {
        let mut m = HashMap::new();
        for (k, v) in vars {
            m.insert((*k).to_string(), *v);
        }
        m
    }

    #[test]
    fn test_push_and_pop() {
        let mut gs = GoalStack::new();
        gs.push(
            "add ones".to_string(),
            "plan1".to_string(),
            vec![],
            Completion::VarPresent { variable: "ones_sum".to_string() },
            simple_snapshot(&[("a", 3), ("b", 5)]),
            100,
        ).unwrap();
        assert_eq!(gs.depth(), 1);
        let checkpoint = gs.pop();
        assert!(checkpoint.is_some());
        assert!(gs.is_empty());
    }

    #[test]
    fn test_max_depth_protection() {
        let mut gs = GoalStack::new();
        for i in 0..8 {
            let _ = gs.push(
                format!("goal_{}", i),
                "plan".to_string(),
                vec![],
                Completion::Manual,
                HashMap::new(),
                i * 100,
            );
        }
        // MAX_DEPTH=7，所以实际深度应该 ≤ 7
        assert!(gs.depth() <= 7);
    }

    #[test]
    fn test_rollback_restores_snapshot() {
        let mut gs = GoalStack::new();
        gs.push(
            "outer".to_string(),
            "plan".to_string(),
            vec![],
            Completion::Manual,
            simple_snapshot(&[("a", 1), ("b", 2)]),
            0,
        ).unwrap();
        gs.push(
            "inner".to_string(),
            "plan".to_string(),
            vec![],
            Completion::Manual,
            simple_snapshot(&[("a", 999)]), // 覆盖 a
            100,
        ).unwrap();
        assert_eq!(gs.depth(), 2);
        // 回退到 checkpoint 0
        let restored = gs.rollback_to(0).unwrap();
        assert_eq!(gs.depth(), 0); // inner 弹出，outer 也弹出（checkpoint >= 0）
        assert_eq!(restored.get("a"), Some(&1));
        assert_eq!(restored.get("b"), Some(&2));
    }

    #[test]
    fn test_check_completion_var_equals() {
        let mut gs = GoalStack::new();
        gs.push(
            "test".to_string(),
            "plan".to_string(),
            vec![],
            Completion::VarEquals {
                variable: "result".to_string(),
                value: 42,
            },
            HashMap::new(),
            0,
        ).unwrap();
        assert!(gs.check_top_completion(&simple_snapshot(&[("result", 42)])));
        assert!(!gs.check_top_completion(&simple_snapshot(&[("result", 0)])));
    }
}