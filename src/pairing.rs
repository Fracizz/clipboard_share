use std::{future::Future, sync::OnceLock};

use anyhow::{Result, bail};
use tokio::sync::{Mutex, watch};

/// Serialize pairing and let an explicit user request cancel startup auto-pairing.
pub(crate) struct PairingCoordinator {
    active: Mutex<()>,
    manual: Mutex<()>,
    requested: watch::Sender<bool>,
}

impl PairingCoordinator {
    fn new() -> Self {
        Self {
            active: Mutex::new(()),
            manual: Mutex::new(()),
            requested: watch::channel(false).0,
        }
    }

    pub(crate) async fn automatic(&self, work: impl Future<Output = Result<()>>) -> Result<()> {
        let mut requested = self.requested.subscribe();
        let _active = self.active.lock().await;
        if *requested.borrow() {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = requested.changed() => Ok(()),
            result = work => result,
        }
        // Dropping work releases its socket before the active guard is released.
    }

    pub(crate) async fn manual<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        let Ok(_manual) = self.manual.try_lock() else {
            bail!("已有手动配对正在进行，请等待本次配对完成或超时后重试");
        };
        self.requested.send_replace(true);
        let _reset = ResetRequest(&self.requested);
        let _active = self.active.lock().await;
        work.await
    }
}

struct ResetRequest<'a>(&'a watch::Sender<bool>);

impl Drop for ResetRequest<'_> {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

pub(crate) fn coordinator() -> &'static PairingCoordinator {
    static COORDINATOR: OnceLock<PairingCoordinator> = OnceLock::new();
    COORDINATOR.get_or_init(PairingCoordinator::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};
    use tokio::{net::TcpListener, sync::oneshot};

    #[tokio::test]
    async fn manual_pairing_reclaims_automatic_listener_and_allows_retry() {
        let coordinator = Arc::new(PairingCoordinator::new());
        let (ready, address) = oneshot::channel();
        let background = coordinator.clone();
        let automatic = tokio::spawn(async move {
            background
                .automatic(async {
                    let listener = TcpListener::bind("127.0.0.1:0").await?;
                    ready.send(listener.local_addr()?).unwrap();
                    listener.accept().await?;
                    Ok(())
                })
                .await
        });
        let address = address.await.unwrap();
        assert!(TcpListener::bind(address).await.is_err());
        tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.manual(async {
                let _listener = TcpListener::bind(address).await?;
                Ok(())
            }),
        )
        .await
        .unwrap()
        .unwrap();
        automatic.await.unwrap().unwrap();
        coordinator
            .manual(async {
                let _listener = TcpListener::bind(address).await?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn duplicate_manual_request_is_rejected_and_cancellation_releases_state() {
        let coordinator = Arc::new(PairingCoordinator::new());
        let (ready, started) = oneshot::channel();
        let first = coordinator.clone();
        let pending = tokio::spawn(async move {
            first
                .manual(async {
                    ready.send(()).unwrap();
                    std::future::pending::<Result<()>>().await
                })
                .await
        });
        started.await.unwrap();
        assert!(
            coordinator
                .manual(async { Ok(()) })
                .await
                .unwrap_err()
                .to_string()
                .contains("已有手动配对")
        );
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert!(!*coordinator.requested.borrow());
        coordinator.manual(async { Ok(()) }).await.unwrap();
    }
}
