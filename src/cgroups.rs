use cgroups_rs::fs::blkio::BlkIoController;
use cgroups_rs::fs::cgroup_builder::*;
use cgroups_rs::fs::cpu::CpuController;
use cgroups_rs::fs::memory::MemController;
use cgroups_rs::fs::*;
use serde::Deserialize;

const LWAY_CGROUP_NAME: &str = "lway";
const LWAY_MEMORY_HARD_LIMIT: i64 = 1024 * 1024 * 1024; // 500 MiB
const LWAY_MEMORY_SOFT_LIMIT: i64 = 0;
const LWAY_MEMORY_SWAP_LIMIT: i64 = 1024 * 1024 * 256; // 256 MiB

const DEFAULT_CPU_WEIGHT: u64 = 100;
const DEFAULT_IO_WEIGHT: u16 = 100;

#[derive(Debug, Clone, Deserialize)]
pub struct MainCgroupConfig {}

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
        .weight(DEFAULT_IO_WEIGHT)
        .done()
        .build(hier)
        .expect("Failed to build cgroup");

    main
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AppCgroupConfig {
    pub cpu_weight: Option<u64>,
    pub io_weight: Option<u16>,
    pub memory_hard_limit: Option<i64>,
    pub memory_soft_limit: Option<i64>,
    pub memory_swap_limit: Option<i64>,
}

const LWAY_APP_CGROUP_PREFIX: &str = "";

/// Create a child cgroup nested under the main cgroup and move `pid` into it.
///
/// Tasks must live in leaf cgroups: cgroup v2's "no internal process constraint" forbids a
/// cgroup from holding tasks directly once it delegates controllers to children.
pub fn init_app_cgroup(name: &str, config: &AppCgroupConfig) -> Cgroup {
    let path = format!("{}/{}{}", LWAY_CGROUP_NAME, LWAY_APP_CGROUP_PREFIX, name);
    let hier = cgroups_rs::fs::hierarchies::auto();

    let app: Cgroup = CgroupBuilder::new(&path)
        .cpu()
        .shares(config.cpu_weight.unwrap_or(DEFAULT_CPU_WEIGHT))
        .done()
        .blkio()
        .weight(config.io_weight.unwrap_or(DEFAULT_IO_WEIGHT))
        .done()
        .memory()
        .memory_hard_limit(config.memory_hard_limit.unwrap_or(LWAY_MEMORY_HARD_LIMIT))
        .memory_soft_limit(config.memory_soft_limit.unwrap_or(LWAY_MEMORY_SOFT_LIMIT))
        .memory_swap_limit(config.memory_swap_limit.unwrap_or(LWAY_MEMORY_SWAP_LIMIT))
        .done()
        .build(hier)
        .expect("Failed to build app cgroup");

    // app.add_task_by_tgid(CgroupPid::from(pid as u64))
    //     .expect("Failed to add task to app cgroup");

    app
}

use std::ffi::c_int;
use std::fs::File;
use std::os::unix::io::AsRawFd;

use crate::support::to_ioresult;

const CLONE_INTO_CGROUP: u64 = 0x200000000;

#[repr(C)]
struct CloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

pub fn spawn_into_cgroup(cgroup_path: &str) -> std::io::Result<libc::pid_t> {
    let cg_file = File::open(cgroup_path)?;
    let mut args = CloneArgs {
        flags: CLONE_INTO_CGROUP,
        pidfd: 0,
        child_tid: 0,
        parent_tid: 0,
        exit_signal: libc::SIGCHLD as u64,
        stack: 0,
        stack_size: 0,
        tls: 0,
        set_tid: 0,
        set_tid_size: 0,
        cgroup: cg_file.as_raw_fd() as u64,
    };

    let ret = unsafe {
        libc::syscall(
            libc::SYS_clone3,
            &mut args as *mut CloneArgs,
            std::mem::size_of::<CloneArgs>(),
        )
    };
    let pid = to_ioresult(ret as c_int)?;

    Ok(pid as libc::pid_t)
}

/// Point-in-time resource usage read from a cgroup's controllers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CgroupUsage {
    pub cpu_usage_usec: u64,
    pub memory_current: u64,
    pub memory_peak: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
}

/// Best-effort snapshot of `cgroup`'s resource usage; missing controllers report 0.
/// Assumes a cgroup v2 (unified) hierarchy.
pub fn read_cgroup_usage(cgroup: &Cgroup) -> CgroupUsage {
    let cpu_usage_usec = cgroup
        .controller_of::<CpuController>()
        .map(|c| parse_cpu_stat_usec(&c.cpu().stat))
        .unwrap_or(0);

    let (memory_current, memory_peak) = cgroup
        .controller_of::<MemController>()
        .map(|c| {
            let mem = c.memory_stat();
            (mem.usage_in_bytes, mem.max_usage_in_bytes)
        })
        .unwrap_or((0, 0));

    let (io_read_bytes, io_write_bytes) = cgroup
        .controller_of::<BlkIoController>()
        .map(|c| {
            // cgroup v2: io.stat
            c.blkio()
                .io_stat
                .iter()
                .fold((0, 0), |(r, w), s| (r + s.rbytes, w + s.wbytes))
        })
        .unwrap_or((0, 0));

    CgroupUsage {
        cpu_usage_usec,
        memory_current,
        memory_peak,
        io_read_bytes,
        io_write_bytes,
    }
}

fn parse_cpu_stat_usec(stat: &str) -> u64 {
    stat.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}
