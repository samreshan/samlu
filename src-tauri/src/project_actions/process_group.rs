//! A step's whole process tree, so stopping a task also stops whatever it
//! started: a process group on Unix, a job object on Windows.

use std::process::{Child, Command, ExitStatus};

#[cfg(unix)]
pub struct ProcessGroup(libc::pid_t);

#[cfg(unix)]
impl ProcessGroup {
    /// Makes the process `command` starts the leader of a new group.
    pub fn prepare(command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    pub fn adopt(child: &Child) -> Option<Self> {
        Some(Self(child.id() as libc::pid_t))
    }

    /// Asks everything in the group to end.
    pub fn terminate(&self) {
        self.signal(libc::SIGTERM);
    }

    /// Ends everything in the group now.
    pub fn kill(&self) {
        self.signal(libc::SIGKILL);
    }

    fn signal(&self, signal: libc::c_int) {
        // A group that has already exited just returns ESRCH.
        unsafe {
            libc::killpg(self.0, signal);
        }
    }
}

#[cfg(windows)]
pub struct ProcessGroup(windows_sys::Win32::Foundation::HANDLE);

// The job handle is only ever used to terminate or close the job, which
// Windows allows from any thread.
#[cfg(windows)]
unsafe impl Send for ProcessGroup {}

#[cfg(windows)]
impl ProcessGroup {
    /// Console programs get no window of their own; their output is read
    /// through pipes.
    pub fn prepare(command: &mut Command) {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    /// Puts `child` in a new job. Whatever it starts from then on joins the
    /// job too.
    pub fn adopt(child: &Child) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            if AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Self(job))
        }
    }

    /// Windows has no polite stop for console programs that don't share
    /// Samlu's console, so stopping ends the tree at once.
    pub fn terminate(&self) {
        self.kill();
    }

    pub fn kill(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// `CREATE_NO_WINDOW`: a console program started without a console window.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The exit code, or 128 plus the signal that ended the process on Unix.
pub fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1)
    }
}
