use std::time::Duration;

use anyhow::{Result, bail};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::Threading::{
            CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, OpenMutexW, ReleaseMutex,
            ResetEvent, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
        },
    },
    core::HSTRING,
};

/// UI 和命令行可能各自读到不同的 config.json / device_id。
/// 锁如果按设备拆开，两套程序会同时改同一块系统剪贴板，A、B 两端就会来回同步。
const SYNC_MUTEX_NAME: &str = "Global\\ClipboardShare-Sync";
const STOP_EVENT_NAME: &str = "Global\\ClipboardShare-Stop";

pub const ALREADY_RUNNING: &str =
    "ClipboardShare 已在运行。托盘版和命令行版不能同时同步，否则两端会反复互相同步";

pub struct InstanceControl {
    mutex: HANDLE,
    stop_event: HANDLE,
}

impl InstanceControl {
    pub fn create() -> Result<Self> {
        unsafe {
            let mutex_name = HSTRING::from(SYNC_MUTEX_NAME);
            let mutex = CreateMutexW(None, false, &mutex_name)?;
            // 用等待结果判断归属，不依赖 Create* 之后的 GetLastError。
            let acquired = matches!(
                WaitForSingleObject(mutex, 0),
                WAIT_OBJECT_0 | WAIT_ABANDONED
            );
            if !acquired {
                let _ = CloseHandle(mutex);
                bail!("{ALREADY_RUNNING}");
            }
            let event_name = HSTRING::from(STOP_EVENT_NAME);
            let stop_event = match CreateEventW(None, true, false, &event_name) {
                Ok(handle) => handle,
                Err(error) => {
                    release_mutex(mutex);
                    return Err(error.into());
                }
            };
            // 崩溃或上一次停止可能留下已触发的事件，新实例先清掉。
            if let Err(error) = ResetEvent(stop_event) {
                let _ = CloseHandle(stop_event);
                release_mutex(mutex);
                return Err(error.into());
            }
            Ok(Self { mutex, stop_event })
        }
    }

    pub async fn wait_for_stop(&self) {
        loop {
            let signaled = unsafe { WaitForSingleObject(self.stop_event, 0) == WAIT_OBJECT_0 };
            if signaled {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Drop for InstanceControl {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.stop_event);
            release_mutex(self.mutex);
        }
    }
}

pub fn is_running() -> bool {
    let name = HSTRING::from(SYNC_MUTEX_NAME);
    unsafe {
        let Ok(handle) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &name) else {
            return false;
        };
        let _ = CloseHandle(handle);
    }
    true
}

pub fn request_stop() -> Result<bool> {
    let name = HSTRING::from(STOP_EVENT_NAME);
    unsafe {
        let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, &name) else {
            return Ok(false);
        };
        let result = SetEvent(handle);
        let _ = CloseHandle(handle);
        result?;
    }
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if !is_running() {
            return Ok(true);
        }
    }
    bail!(
        "{}",
        crate::config::with_log_hint(
            "同步未在 5 秒内停止，或在任务管理器中结束 clipboard_share.exe 和 clipboard_share_ui.exe"
        )
    )
}

fn release_mutex(mutex: HANDLE) {
    unsafe {
        let _ = ReleaseMutex(mutex);
        let _ = CloseHandle(mutex);
    }
}
