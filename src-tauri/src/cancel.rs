use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

/// 组合式取消原语（B-08）：替代原先分散的 `Arc<AtomicBool>`。
///
/// 同时提供两种消费方式：
/// - 异步等待：`cancelled()` 可在 `tokio::select!` 中与任意 future 竞争，
///   SSE 空闲等待、限流排队、重试退避 sleep 均通过它即时打断；
/// - 同步轮询：内部维护 `AtomicBool` 标志，`atomic()` 取出后交给
///   阻塞域代码（ffmpeg_exe 转码轮询、切片循环），其接口保持不变。
///
/// 基于 `tokio::sync::watch` 实现（清单 B-08：优先不新增依赖的方案），
/// 无 tokio-util CancellationToken 依赖；watch 的「接收方可克隆 +
/// 状态可查询」语义天然避免 Notify 的丢唤醒竞态。
#[derive(Clone)]
pub struct CancelToken {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
    flag: Arc<AtomicBool>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx,
            rx,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 置位取消。幂等；取消后所有 `cancelled()` 等待者立即被唤醒。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        let _ = self.tx.send(true);
    }

    /// 同步查询是否已取消（阻塞域/循环头检查用）。
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// 供阻塞域代码轮询的标志副本（驱动 ffmpeg_exe 的 cancel 参数等）。
    pub fn atomic(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    /// 异步等待取消信号；已取消时立即就绪。
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        loop {
            if *rx.borrow_and_update() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
            if *rx.borrow() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_wakes_waiter() {
        let token = CancelToken::new();
        let t2 = token.clone();
        let join = tokio::spawn(async move {
            t2.cancelled().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), join)
            .await
            .expect("cancelled() 应在取消后 2 秒内返回")
            .unwrap();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_returns_immediately_when_already_cancelled() {
        let token = CancelToken::new();
        token.cancel();
        let start = std::time::Instant::now();
        token.cancelled().await;
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[tokio::test]
    async fn cancel_is_idempotent_and_flag_shared() {
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        let flag = token.atomic();
        assert!(flag.load(Ordering::Acquire));
        let clone = token.clone();
        assert!(clone.is_cancelled());
    }

    #[tokio::test]
    async fn select_interrupts_long_sleep() {
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        tokio::select! {
            _ = token.cancelled() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                panic!("长 sleep 应被取消打断");
            }
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
}
