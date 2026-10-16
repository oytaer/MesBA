// ============================================================================
// session.rs —— 推理会话管理（§13.1）
// ============================================================================
// 设计文档定义的核心接口：
//   submit(input: MultimodalInput, goal?) → TaskId
//   observe(task_id)                      → 事件流 iterator
//   get_result(task_id, timeout?)         → InferenceResult
//   cancel(task_id) / pause(task_id) / resume(task_id)
//
// 关键架构决策（AdvisorTool 指导）：
//   SessionManager 持有 ONE InferenceRuntime（Rust ownership：runtime 内含
//   Network borrow，不能多实例并存）。HashMap<TaskId, SessionMetadata> 存
//   任务元数据 + 输入快照 + 缓存结果。submit 同步跑完并缓存 → poll-based
//   无 async 无 threading（MVP 阶段）。
// ============================================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::api::{InferenceResult, now_ms};
use crate::runtime::InferenceRuntime;

// ============================================================================
// TaskId —— 会话唯一 ID（全局自增 u64，用 Atomic 保证线程安全）
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

/// 全局 TaskId 自增器（AtomicU64 避免多线程冲突）
static TASK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl TaskId {
    pub fn next() -> Self {
        Self(TASK_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task-{}", self.0)
    }
}

// ============================================================================
// SessionState —— 会话生命周期
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// 已提交，等待执行（poll-based 下 submit 后立即进入 Completed）
    Pending,
    /// 正在执行（poll-based 下瞬态，几乎看不到）
    Running,
    /// 已完成，结果可 get_result
    Completed,
    /// 已暂停（pause 调用后状态）
    Paused,
    /// 已取消（cancel 调用后）
    Cancelled,
    /// 执行中触发看门狗 kill 或安全违规 kill
    Killed,
}

// ============================================================================
// TraceEvent —— observe() 返回的事件类型（简化）
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    /// 时间戳（epoch ms）
    pub ts_ms: u64,
    /// 事件类型："submitted" / "step_completed" / "result_ready" / "killed"
    pub event_type: String,
    /// 附加数据（可选，JSON 序列化的字符串）
    pub detail: Option<String>,
}

// ============================================================================
// SessionMetadata —— 每个任务的元数据（不含 runtime 本体）
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub task_id: TaskId,
    pub state: SessionState,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    /// 输入变量快照（submit 时存）
    pub input_snapshot: Vec<(String, i32)>,
    /// 事件流（observe 返回这些）
    pub events: Vec<TraceEvent>,
    /// 缓存结果（submit 跑完就塞这里）
    pub cached_result: Option<InferenceResult>,
    /// 暂停标记（pause/resume 切这个）
    pub pause_flag: bool,
    /// kill 原因（SessionState::Killed 时填）
    pub kill_reason: Option<String>,
}

// ============================================================================
// SessionManager —— 会话管理器核心
// ============================================================================
pub struct SessionManager {
    /// 唯一 runtime（所有会话共享 — MVP poll-based 架构）
    runtime: InferenceRuntime,
    /// 所有会话元数据
    sessions: HashMap<TaskId, SessionMetadata>,
}

impl SessionManager {
    pub fn new(runtime: InferenceRuntime) -> Self {
        Self {
            runtime,
            sessions: HashMap::new(),
        }
    }

    // ========================================================================
    // §13.1 核心 6 接口
    // ========================================================================

    /// submit —— 提交输入，同步执行，缓存结果
    /// 返回 TaskId（poll-based 架构下 submit=执行=缓存一步完成）
    pub fn submit(&mut self, input: HashMap<String, i32>) -> TaskId {
        let task_id = TaskId::next();
        let ts = now_ms();

        // 1) 创建元数据（state 先 Pending）
        let mut meta = SessionMetadata {
            task_id,
            state: SessionState::Pending,
            created_at_ms: ts,
            finished_at_ms: None,
            input_snapshot: input.clone().into_iter().collect(),
            events: vec![TraceEvent {
                ts_ms: ts,
                event_type: "submitted".into(),
                detail: Some(format!("input_keys={}", input.len())),
            }],
            cached_result: None,
            pause_flag: false,
            kill_reason: None,
        };

        // 2) poll-based：直接跑完（如果 pause_flag 被外面设了则跳过执行）
        if !meta.pause_flag {
            meta.state = SessionState::Running;
            let run_ts = now_ms();
            meta.events.push(TraceEvent {
                ts_ms: run_ts,
                event_type: "running".into(),
                detail: None,
            });

            let resp = self.runtime.run_query(input);
            let result = resp.data;
            let done_ts = now_ms();

            // 检查 kill_flag（看门狗 / SecurityGate 可能触发）
            if self.runtime.controller.kill_flag {
                meta.state = SessionState::Killed;
                meta.kill_reason = self.runtime.controller.kill_reason.clone();
                meta.events.push(TraceEvent {
                    ts_ms: done_ts,
                    event_type: "killed".into(),
                    detail: meta.kill_reason.clone(),
                });
            } else {
                meta.state = SessionState::Completed;
                meta.events.push(TraceEvent {
                    ts_ms: done_ts,
                    event_type: "result_ready".into(),
                    detail: result.final_result.map(|v| format!("final_result={}", v)),
                });
            }
            meta.cached_result = Some(result);
            meta.finished_at_ms = Some(done_ts);
        }

        // 3) 存入 HashMap
        self.sessions.insert(task_id, meta);
        task_id
    }

    /// observe —— 返回该任务的事件流（迭代器）
    /// poll-based 架构下返回 cached events 的迭代器
    pub fn observe(&self, task_id: TaskId) -> Box<dyn Iterator<Item = &TraceEvent> + '_> {
        let iter = self
            .sessions
            .get(&task_id)
            .into_iter()
            .flat_map(|m| m.events.iter());
        Box::new(iter)
    }

    /// get_result —— 获取缓存的推理结果
    /// poll-based 架构下 timeout 没用（submit 已经同步跑完）
    pub fn get_result(&self, task_id: TaskId, _timeout_ms: u64) -> Option<&InferenceResult> {
        self.sessions
            .get(&task_id)
            .and_then(|m| m.cached_result.as_ref())
    }

    /// cancel —— 取消会话
    pub fn cancel(&mut self, task_id: TaskId) -> bool {
        if let Some(meta) = self.sessions.get_mut(&task_id)
            && (meta.state == SessionState::Pending || meta.state == SessionState::Paused)
        {
            meta.state = SessionState::Cancelled;
            meta.finished_at_ms = Some(now_ms());
            meta.events.push(TraceEvent {
                ts_ms: now_ms(),
                event_type: "cancelled".into(),
                detail: None,
            });
            return true;
        }
        false
    }

    /// pause —— 暂停会话（poll-based 下只是切 flag）
    pub fn pause(&mut self, task_id: TaskId) -> bool {
        if let Some(meta) = self.sessions.get_mut(&task_id)
            && (meta.state == SessionState::Pending || meta.state == SessionState::Running)
        {
            meta.state = SessionState::Paused;
            meta.pause_flag = true;
            meta.events.push(TraceEvent {
                ts_ms: now_ms(),
                event_type: "paused".into(),
                detail: None,
            });
            return true;
        }
        false
    }

    /// resume —— 恢复暂停的会话
    pub fn resume(&mut self, task_id: TaskId) -> bool {
        if let Some(meta) = self.sessions.get_mut(&task_id)
            && meta.state == SessionState::Paused
        {
            meta.state = SessionState::Pending; // 回到 pending，下次 submit / run 时会被重新调度
            meta.pause_flag = false;
            meta.events.push(TraceEvent {
                ts_ms: now_ms(),
                event_type: "resumed".into(),
                detail: None,
            });
            return true;
        }
        false
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================

    pub fn get_state(&self, task_id: TaskId) -> Option<SessionState> {
        self.sessions.get(&task_id).map(|m| m.state)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn runtime(&self) -> &InferenceRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut InferenceRuntime {
        &mut self.runtime
    }

    pub fn cleanup_finished(&mut self) -> usize {
        let before = self.sessions.len();
        self.sessions.retain(|_, m| {
            !matches!(
                m.state,
                SessionState::Completed
                    | SessionState::Cancelled
                    | SessionState::Killed
            )
        });
        before - self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_seeded_runtime() -> InferenceRuntime {
        let net = mesba_engine::seed::build_addition_table();
        InferenceRuntime::new(net)
    }

    #[test]
    fn test_submit_completes() {
        let mut sm = SessionManager::new(make_seeded_runtime());
        let task_id = sm.submit([("a".into(), 3), ("b".into(), 5)].into());
        assert_eq!(sm.get_state(task_id), Some(SessionState::Completed));
        let result = sm.get_result(task_id, 1000).unwrap();
        assert_eq!(result.final_result, Some(8));
    }

    #[test]
    fn test_observe_events() {
        let mut sm = SessionManager::new(make_seeded_runtime());
        let task_id = sm.submit([("a".into(), 2), ("b".into(), 4)].into());
        let events: Vec<_> = sm.observe(task_id).collect();
        assert!(events.iter().any(|e| e.event_type == "submitted"));
        assert!(events.iter().any(|e| e.event_type == "result_ready"));
    }

    #[test]
    fn test_cancel_pending() {
        let mut sm = SessionManager::new(make_seeded_runtime());
        let tid = TaskId::next();
        // 手动塞一个 Pending 元数据（pause_flag=true → submit 不执行）
        sm.sessions.insert(
            tid,
            SessionMetadata {
                task_id: tid,
                state: SessionState::Pending,
                created_at_ms: now_ms(),
                finished_at_ms: None,
                input_snapshot: vec![],
                events: vec![],
                cached_result: None,
                pause_flag: true,
                kill_reason: None,
            },
        );
        assert!(sm.cancel(tid));
        assert_eq!(sm.get_state(tid), Some(SessionState::Cancelled));
    }

    #[test]
    fn test_task_id_unique() {
        let a = TaskId::next();
        let b = TaskId::next();
        assert_ne!(a, b);
        assert!(b.0 > a.0);
    }

    #[test]
    fn test_session_count() {
        let mut sm = SessionManager::new(make_seeded_runtime());
        assert_eq!(sm.session_count(), 0);
        let _ = sm.submit([("a".into(), 1), ("b".into(), 2)].into());
        let _ = sm.submit([("a".into(), 3), ("b".into(), 4)].into());
        assert_eq!(sm.session_count(), 2);
        let cleaned = sm.cleanup_finished();
        assert!(cleaned >= 2);
        assert_eq!(sm.session_count(), 0);
    }

    #[test]
    fn test_pause_resume_sanity() {
        let mut sm = SessionManager::new(make_seeded_runtime());
        // 手动塞 Pending
        let tid = TaskId::next();
        sm.sessions.insert(
            tid,
            SessionMetadata {
                task_id: tid,
                state: SessionState::Pending,
                created_at_ms: now_ms(),
                finished_at_ms: None,
                input_snapshot: vec![],
                events: vec![],
                cached_result: None,
                pause_flag: false,
                kill_reason: None,
            },
        );
        assert!(sm.pause(tid));
        assert_eq!(sm.get_state(tid), Some(SessionState::Paused));
        assert!(sm.resume(tid));
        assert_eq!(sm.get_state(tid), Some(SessionState::Pending));
    }
}
