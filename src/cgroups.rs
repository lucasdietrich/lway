use cgroups_rs::{CgroupPid, fs::*};
use cgroups_rs::fs::cgroup_builder::*;

const LWAY_CGROUP_NAME: &str = "lway";
const LWAY_MEMORY_HARD_LIMIT: i64 = 1024 * 1024 * 1024; // 500 MiB
const LWAY_MEMORY_SOFT_LIMIT: i64 = 0;
const LWAY_MEMORY_SWAP_LIMIT: i64 = 1024 * 1024 * 256; // 256 MiB
const DEFAULT_CPU_WEIGHT: u64 = 100;

pub fn init_main_cgroup() -> Cgroup {
    let hier = cgroups_rs::fs::hierarchies::auto();

    let main: Cgroup = CgroupBuilder::new(LWAY_CGROUP_NAME)
        .cpu()
        .shares(DEFAULT_CPU_WEIGHT)
        .done()
        .memory()
        .memory_hard_limit(LWAY_MEMORY_HARD_LIMIT) 
        .memory_soft_limit(LWAY_MEMORY_SOFT_LIMIT)
        .memory_swap_limit(LWAY_MEMORY_SWAP_LIMIT)
        .done()
        .blkio()
        .weight(100)
        .done()
        .build(hier)
        .expect("Failed to build cgroup");

    main
}

/// Create a child cgroup nested under the main cgroup and move `pid` into it.
///
/// Tasks must live in leaf cgroups: cgroup v2's "no internal process constraint" forbids a
/// cgroup from holding tasks directly once it delegates controllers to children.
pub fn init_app_cgroup(name: &str, pid: u32, cpu_weight: Option<u64>) -> Cgroup {
    let path = format!("{}/ly-{}", LWAY_CGROUP_NAME, name);
    let hier = cgroups_rs::fs::hierarchies::auto();

    let app: Cgroup = CgroupBuilder::new(&path)
        .cpu()
        .shares(cpu_weight.unwrap_or(DEFAULT_CPU_WEIGHT))
        .done()
        .build(hier)
        .expect("Failed to build app cgroup");

    app.add_task_by_tgid(CgroupPid::from(pid as u64))
        .expect("Failed to add task to app cgroup");

    app
}