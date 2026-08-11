//! 进程树管理：Windows Job Object 绑定与整树终止。
//!
//! `run_command` 与 MCP server 子进程共用同一套"不遗留孤儿进程"的基建：
//! spawn 后立即把子进程绑进带 `KILL_ON_JOB_CLOSE` 的 Job Object，之后无论是
//! 显式终止还是句柄随 Drop 关闭，整棵进程树(含 npx/shell 派生的后代)都会被回收。
//! 绑定失败时回退 `taskkill /T`,非 Windows 平台只杀直接子进程。

use std::process::{Child, Command, Stdio};

#[cfg(windows)]
pub(crate) struct ProcessJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: Job Object 句柄是进程级内核对象;TerminateJobObject 与 CloseHandle
// 都允许从任意线程调用。MCP 连接把它放在 Mutex 保护的 teardown 里跨线程关停。
#[cfg(windows)]
unsafe impl Send for ProcessJob {}
#[cfg(windows)]
unsafe impl Sync for ProcessJob {}

#[cfg(windows)]
impl ProcessJob {
    pub(crate) fn attach(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self { handle };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let process = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        if unsafe { AssignProcessToJobObject(job.handle, process) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(crate) fn terminate(&self) -> bool {
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1) != 0 }
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
pub(crate) struct ProcessJob;

#[cfg(not(windows))]
impl ProcessJob {
    pub(crate) fn attach(_child: &Child) -> std::io::Result<Self> {
        Ok(Self)
    }

    pub(crate) fn terminate(&self) -> bool {
        false
    }
}

/// 终止整棵进程树。Windows 优先终止 Job Object，无法绑定时回退 taskkill /T。
pub(crate) fn kill_tree(child: &mut Child, process_job: Option<&ProcessJob>) {
    let job_terminated = process_job.is_some_and(ProcessJob::terminate);

    if cfg!(windows) && !job_terminated {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}
