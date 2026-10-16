// ============================================================================
// transfer.rs —— 迁移/学习模块（环 8：TRANSFER）
// ============================================================================
// 职责：每次推理结束后调用 mesba_learning 的四种算法，做一次完整学习
// 调用顺序：
//   1) Hebbian 连接塑形  ← trace 中的相邻 block pairs
//   2) Threshold 阈值修正 ← 根据 Feedback 给的 error 信号
//   3) Pruning 低权重修剪 ← 自然清理网络冗余
//   4) Chunking 组块化分析 ← 找出可以合并的高频共现对
// ============================================================================

use mesba_core::MemoryBackend;

use crate::controller::FreeMechanismController;

#[derive(Debug, Clone, Default)]
pub struct Transfer;

impl Transfer {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // 从 workspace.trace() 取所有 TraceEntry
        let trace: Vec<mesba_core::TraceEntry> =
            ctrl.workspace.trace().to_vec();

        // 没有 trace → 不学习
        if trace.is_empty() {
            return;
        }

        // 取出当前 feedback 值（Feedback 模块已写到 PulseBuffer）
        let feedback_value = ctrl.get_var("feedback_value").unwrap_or(0);
        let error_signal = ctrl.get_var("error").unwrap_or(0);

        // ---- 1) Hebbian 连接塑形 ----
        let hebbian_stats = mesba_learning::hebbian_reinforce(
            &mut ctrl.network,
            &trace,
            mesba_learning::DEFAULT_LEARN_RATE,
        );
        // 低共现 → 不新建连接（控制网络膨胀）
        let _created = mesba_learning::hebbian_ensure_connections(
            &mut ctrl.network,
            &trace,
            1,
        );
        let _ = hebbian_stats;

        // ---- 2) Threshold 阈值修正 ----
        // 收集 trace 中出现的所有区块 ID（每个区块至少被激活一次）
        let activated: Vec<mesba_core::BlockId> = trace
            .iter()
            .map(|e| e.block_id.clone())
            .collect();
        let _threshold_stats = mesba_learning::adjust_from_error_signal(
            &mut ctrl.network,
            &activated,
            error_signal,
            mesba_learning::THRESHOLD_STEP,
        );

        // ---- 3) Pruning 低权重修剪 ----
        let _prune_stats = mesba_learning::prune_weak_connections(
            &mut ctrl.network,
            mesba_learning::DEFAULT_PRUNE_THRESHOLD,
        );

        // ---- 4) Chunking 组块化分析 ----
        let co_counts = mesba_learning::collect_co_activation_pairs(&trace);
        let _chunk_stats = mesba_learning::chunk_network(
            &ctrl.network,
            &co_counts,
            2, // 至少出现 2 次才算候选
        );

        // 重置 workspace 为下一次推理做准备
        // 不重置 trace —— 让 Feedback 模块可以事后分析
        let _ = feedback_value; // 预留：未来根据 feedback 做更高阶学习
    }
}
