// ============================================================================
// plan.rs —— 规划调度器（环 5：PLAN → 目标分解 + 风险预估 + B计划）
// ============================================================================
// 设计文档 §9 环节 5：规划
//   ① 目标分解：读取 Plan 区块的 subgoals → 按顺序压入 GoalStack
//   ② 风险预估：查找 Experience 区块数量 → 映射风险等级
//   ③ B计划：如果 Plan.backup_plans 非空，记录备用方案数量
//   ④ 变量注入：planning_risk / backup_plan_count 供 Feedback 环节查询
// ============================================================================

use crate::controller::FreeMechanismController;
use mesba_core::{BlockContent, BlockId, BlockType};
use mesba_memory::goal_stack::{Condition, Completion};

#[derive(Debug, Clone, Default)]
pub struct Planner;

impl Planner {
    pub fn new() -> Self {
        Self
    }

    /// ★ 完整规划 tick —— 设计文档 §8.2 Planner + §9 环节 5 全部实现
    pub fn tick(&mut self, ctrl: &mut FreeMechanismController) {
        // ---- 步骤 1：发现所有 Plan 区块 ----
        let plan_blocks: Vec<(BlockId, Vec<mesba_core::PlanSubgoal>)> = self.discover_plan_blocks(ctrl);

        // ---- 步骤 2：对每个 Plan 区块 → 分解子目标，压入 GoalStack ----
        for (plan_id, subgoals) in &plan_blocks {
            self.decompose_plan_into_goals(ctrl, plan_id, subgoals);
        }

        // ---- 步骤 3：风险预估 —— 查 Experience 区块 ----
        let risk_level = self.estimate_risk(ctrl);
        ctrl.set_var("planning_risk", risk_level as i32);

        // ---- 步骤 4：统计所有 Plan 的 backup_plans ----
        self.record_backup_plans(ctrl);
    }

    /// 扫描 network 中所有 Plan 区块，返回 (plan_id, subgoals) 列表
    fn discover_plan_blocks(
        &self,
        ctrl: &FreeMechanismController,
    ) -> Vec<(BlockId, Vec<mesba_core::PlanSubgoal>)> {
        ctrl.network
            .find_by_type(BlockType::Plan)
            .into_iter()
            .filter_map(|b| match &b.content {
                BlockContent::Plan { subgoals, .. } if !subgoals.is_empty() => {
                    Some((b.id.clone(), subgoals.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// 把 Plan.subgoals 按顺序压入 GoalStack
    fn decompose_plan_into_goals(
        &mut self,
        ctrl: &mut FreeMechanismController,
        plan_id: &BlockId,
        subgoals: &[mesba_core::PlanSubgoal],
    ) {
        for (idx, sg) in subgoals.iter().enumerate() {
            let goal_name = format!("Plan[{}::{}] step{}", plan_id.as_str(), sg.name, idx);
            let plan_id_str = plan_id.as_str().to_string();

            // MVP 简化：子目标完成条件默认为 Manual（由 Feedback 环节显式 pop）
            let completion = Completion::Manual;

            // 依赖 → Condition（如依赖 "step_0" 则检查 pulse_buffer 有 plan_step_0_done=1）
            let conditions: Vec<Condition> = sg
                .dependencies
                .iter()
                .map(|dep| {
                    let dep_var = format!("plan_step_{}_done", dep);
                    Condition {
                        variable: dep_var,
                        expected_value: 1,
                    }
                })
                .collect();

            let _ = ctrl.workspace.push_goal(goal_name, plan_id_str, conditions, completion);
        }
    }

    /// 风险预估 —— Experience 区块数量 → 风险等级 0~255
    /// 0 个 Experience = 全新任务 → 低风险（20）
    /// 1~3 个 = 有少量经验 → 中风险（60）
    /// 4+ 个 = 经验多 → 高风险（100）
    pub fn estimate_risk(&self, ctrl: &FreeMechanismController) -> u8 {
        let exp_count = ctrl.network.find_by_type(BlockType::Experience).len();
        match exp_count {
            0 => 20,         // 全新任务，低风险
            1..=3 => 60,     // 有少量经验
            _ => 100,        // 有较多经验 → 可能复杂
        }
    }

    /// 统计所有 Plan.backup_plans 总数，写入 PulseBuffer 变量
    fn record_backup_plans(&mut self, ctrl: &mut FreeMechanismController) {
        let mut total_backups: u32 = 0;

        for block in ctrl.network.find_by_type(BlockType::Plan) {
            if let BlockContent::Plan { backup_plans, .. } = &block.content {
                total_backups += backup_plans.len() as u32;
            }
        }

        ctrl.set_var("backup_plan_count", total_backups as i32);
    }
}

// ============================================================================
// 单元测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use mesba_core::Block;
    use mesba_network::MeshBlockNetwork;

    fn make_plan_only_ctrl() -> FreeMechanismController {
        let mut net = MeshBlockNetwork::new();
        let plan_id = BlockId::from("plan.test");
        net.register_block(Block::plan(
            plan_id.clone(),
            "test_domain".to_string(),
            1,
            "测试规划".to_string(),
            vec![
                mesba_core::PlanSubgoal {
                    name: "步骤1".into(),
                    description: "执行第一步".into(),
                    block_ref: None,
                    dependencies: vec![],
                },
                mesba_core::PlanSubgoal {
                    name: "步骤2".into(),
                    description: "执行第二步".into(),
                    block_ref: None,
                    dependencies: vec![],
                },
            ],
        ));
        FreeMechanismController::new(net)
    }

    #[test]
    fn test_planner_tick_pushes_subgoals() {
        let mut ctrl = make_plan_only_ctrl();
        let initial_depth = ctrl.workspace.goal_depth();
        let mut planner = Planner::new();
        planner.tick(&mut ctrl);
        assert!(
            ctrl.workspace.goal_depth() > initial_depth,
            "Planner 应压入子目标：初始={} 结果={}",
            initial_depth,
            ctrl.workspace.goal_depth()
        );
    }

    #[test]
    fn test_planner_sets_risk_var() {
        let mut ctrl = make_plan_only_ctrl();
        let mut planner = Planner::new();
        planner.tick(&mut ctrl);
        let risk = ctrl.get_var("planning_risk");
        assert!(risk.is_some(), "应设置 planning_risk 变量");
        let risk_val = risk.unwrap();
        assert!(
            (0..=255).contains(&risk_val),
            "planning_risk 应在 0~255 范围，实际={}",
            risk_val
        );
    }

    #[test]
    fn test_planner_without_plan_blocks() {
        // 空网络 → Planner 不应 panic
        let net = MeshBlockNetwork::new();
        let mut ctrl = FreeMechanismController::new(net);
        let mut planner = Planner::new();
        planner.tick(&mut ctrl);
        assert_eq!(ctrl.workspace.goal_depth(), 0, "无 Plan 区块时 GoalStack 应空");
        assert!(ctrl.get_var("planning_risk").is_some());
    }

    #[test]
    fn test_planner_backup_plan_count() {
        let mut ctrl = make_plan_only_ctrl();
        let mut planner = Planner::new();
        planner.tick(&mut ctrl);
        let backup_count = ctrl.get_var("backup_plan_count");
        assert_eq!(backup_count, Some(0), "默认 backup_plans 应为空");
    }

    #[test]
    fn test_estimate_risk_levels() {
        let mut net = MeshBlockNetwork::new();
        let plan_id = BlockId::from("plan.risk_test");
        net.register_block(Block::plan(
            plan_id,
            "test".to_string(),
            1,
            "风险测试计划".to_string(),
            vec![],
        ));

        // 0 个 Experience → 低风险
        let ctrl = FreeMechanismController::new(net.clone());
        let planner = Planner::new();
        let risk = planner.estimate_risk(&ctrl);
        assert_eq!(risk, 20, "0 个 Experience → 风险=20");

        // 添加 2 个 Experience → 中风险
        let mut net2 = net.clone();
        net2.register_block(Block::experience(
            BlockId::from("exp.failure_1"),
            "test".to_string(),
            1,
            "失败情境 1".to_string(),
            "教训 1".to_string(),
        ));
        net2.register_block(Block::experience(
            BlockId::from("exp.failure_2"),
            "test".to_string(),
            1,
            "失败情境 2".to_string(),
            "教训 2".to_string(),
        ));
        let ctrl2 = FreeMechanismController::new(net2);
        let risk2 = planner.estimate_risk(&ctrl2);
        assert_eq!(risk2, 60, "2 个 Experience → 风险=60");

        // 添加 5 个 Experience → 高风险
        let mut net3 = net.clone();
        for i in 1..=5 {
            net3.register_block(Block::experience(
                BlockId::from(format!("exp.fail_{}", i)),
                "test".to_string(),
                1,
                "失败".to_string(),
                "教训".to_string(),
            ));
        }
        let ctrl3 = FreeMechanismController::new(net3);
        let risk3 = planner.estimate_risk(&ctrl3);
        assert_eq!(risk3, 100, "5 个 Experience → 风险=100");
    }

    #[test]
    fn test_plan_with_dependencies() {
        // 构造有依赖关系的 Plan 区块
        let mut net = MeshBlockNetwork::new();
        let plan_id = BlockId::from("plan.dep_test");
        net.register_block(Block::plan(
            plan_id.clone(),
            "test".to_string(),
            1,
            "有依赖的计划".to_string(),
            vec![
                mesba_core::PlanSubgoal {
                    name: "step0".into(),
                    description: "第一步".into(),
                    block_ref: None,
                    dependencies: vec![], // 无依赖
                },
                mesba_core::PlanSubgoal {
                    name: "step1".into(),
                    description: "第二步".into(),
                    block_ref: None,
                    dependencies: vec!["step0".to_string()], // 依赖 step0
                },
            ],
        ));

        let mut ctrl = FreeMechanismController::new(net);
        let mut planner = Planner::new();
        planner.tick(&mut ctrl);

        // GoalStack 应有 2 层
        assert_eq!(ctrl.workspace.goal_depth(), 2, "应有 2 个子目标");
    }
}
