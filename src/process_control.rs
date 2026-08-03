use std::time::Duration;

use anyhow::{Result, bail};
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0},
        System::Threading::{
            CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
        },
    },
    core::HSTRING,
};

pub struct InstanceControl {
    handle: HANDLE,
}

impl InstanceControl {
    pub fn create(device_id: Uuid) -> Result<Self> {
        let name = event_name(device_id);
        unsafe {
            let handle = CreateEventW(None, true, false, &name)?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                bail!("ClipboardShare 已在后台运行");
            }
            Ok(Self { handle })
        }
    }

    pub async fn wait_for_stop(&self) {
        loop {
            let signaled = unsafe { WaitForSingleObject(self.handle, 0) == WAIT_OBJECT_0 };
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
            let _ = CloseHandle(self.handle);
        }
    }
}

pub fn is_running(device_id: Uuid) -> bool {
    let name = event_name(device_id);
    unsafe {
        let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, &name) else {
            return false;
        };
        let _ = CloseHandle(handle);
    }
    true
}

pub fn request_stop(device_id: Uuid) -> Result<bool> {
    let name = event_name(device_id);
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
        if !is_running(device_id) {
            return Ok(true);
        }
    }
    bail!("后台进程未在 5 秒内停止，请检查 data\\logs 或结束任务管理器中的 clipboard_share.exe")
}

fn event_name(device_id: Uuid) -> HSTRING {
    // 使用 Global，避免桌面会话与 SSH/服务会话互相看不到 Local 命名对象。
    HSTRING::from(format!("Global\\ClipboardShare-{device_id}"))
}
