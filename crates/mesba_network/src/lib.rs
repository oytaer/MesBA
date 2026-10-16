// ============================================================================
// mesba_network —— 网状区块网络管理 crate
// ============================================================================
// 编程语言选型文档指定的目录结构（§五）：
//   network.rs  —— MeshBlockNetwork 网络主体（邻接表 + 双向索引）
//   topology.rs —— 拓扑管理（纵向树规则 / 度约束 / 权重衰减）
//   index.rs    —— 路由索引（domain/type/tag 三维快速查找）
//   storage.rs  —— mmap 存储引擎（L1/L2 分层 / bincode 快照）
// ============================================================================

pub mod network;
pub mod topology;
pub mod index;
pub mod storage;

// 重新导出核心类型
pub use network::MeshBlockNetwork;
pub use topology::{ATOMIC_MAX_DEGREE, DOMAIN_MAX_DEGREE};
pub use index::RouteIndex;
pub use storage::StorageEngine;
