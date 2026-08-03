use std::{mem::size_of, time::Duration};

use anyhow::Result;
use windows::{
    System::DispatcherQueueController,
    Win32::{
        System::WinRT::{
            CreateDispatcherQueueController, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT,
            DispatcherQueueOptions, RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize,
        },
        UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
        },
    },
};

pub struct ClipboardApartment {
    controller: DispatcherQueueController,
    initialized: bool,
}

impl ClipboardApartment {
    pub fn initialize() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_SINGLETHREADED)?;
            let options = DispatcherQueueOptions {
                dwSize: size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_STA,
            };
            match CreateDispatcherQueueController(options) {
                Ok(controller) => Ok(Self {
                    controller,
                    initialized: true,
                }),
                Err(error) => {
                    RoUninitialize();
                    Err(error.into())
                }
            }
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let shutdown = self.controller.ShutdownQueueAsync()?;
        match tokio::time::timeout(Duration::from_secs(2), shutdown).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "DispatcherQueue 关闭失败"),
            Err(_) => tracing::warn!("DispatcherQueue 关闭超时，继续退出"),
        }
        if self.initialized {
            unsafe { RoUninitialize() };
            self.initialized = false;
        }
        Ok(())
    }
}

impl Drop for ClipboardApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { RoUninitialize() };
            self.initialized = false;
        }
    }
}

pub async fn pump_messages() {
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
