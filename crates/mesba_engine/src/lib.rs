// ============================================================================
// mesba_engine —— 脉冲传播引擎（涟漪算法 / 事件队列 / 衰减 / 调度器 / MVP 种子）
// ============================================================================
// 设计文档 §6.3 脉冲传播算法（涟漪扩散）核心实现
//
// 模块划分：
//   scheduler —— 双队列脉冲调度器 + step 计数 + MAX_PULSES_PER_STEP 限流
//   propagator —— 双 Pass 涟漪传播引擎（先转发后衰减）
//   seed —— MVP 种子区块初始化器（单数字 + 完整加法表 + 测试）
//
// 注意：本文件只做模块引入，不实现任何功能代码
// ============================================================================

pub mod activator;
pub mod decay;
pub mod propagator;
pub mod scheduler;
pub mod seed;

pub use propagator::{PulsePropagator, StepResult};
pub use scheduler::PulseScheduler;
