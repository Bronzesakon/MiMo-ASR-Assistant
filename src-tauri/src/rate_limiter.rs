use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

use crate::cancel::CancelToken;

/// 等待限流时被取消
#[derive(Debug)]
pub struct PermitCancelled;

/// 速率控制器核心实现。
///
/// B-07：并发槽（semaphore）与 RPM 记账拆分为两个独立入口——
/// - `acquire_permit`：只占并发槽，调用方持有到本次请求结束；
/// - `wait_rpm_slot`：滑动窗口 RPM 记账，必须在每次**真实请求**发出前调用，
///   修正原先「一个 permit 覆盖多次重试请求、实际 RPM 达配置 3-4 倍」的问题。
///
/// B-08：两个入口均可被 `CancelToken` 即时打断（排队 sleep 纳入 select）。
pub struct RateLimiter {
    semaphore: Arc<Semaphore>,
    request_times: Mutex<VecDeque<std::time::Instant>>,
    rpm_limit: usize,
}

impl RateLimiter {
    pub fn new(max_concurrent: usize, rpm_limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            request_times: Mutex::new(VecDeque::new()),
            rpm_limit,
        }
    }

    /// 获取并发许可，返回 permit（调用方持有直到请求完成）。
    /// 等待期间响应取消；semaphore 仅在 app 生命周期内有效，不会被关闭。
    pub async fn acquire_permit(
        &self,
        cancel: &CancelToken,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, PermitCancelled> {
        tokio::select! {
            permit = self.semaphore.clone().acquire_owned() => {
                Ok(permit.expect("限流 semaphore 不应被关闭"))
            }
            _ = cancel.cancelled() => Err(PermitCancelled),
        }
    }

    /// RPM 滑动窗口记账：窗口内请求数达到上限时等待最旧记录过期，
    /// 获得名额后**立即记账**（记账点即真实请求发出前）。
    /// 等待期间响应取消。
    pub async fn wait_rpm_slot(&self, cancel: &CancelToken) -> Result<(), PermitCancelled> {
        loop {
            let wait_duration = {
                let mut times = self.request_times.lock().unwrap();
                let now = std::time::Instant::now();
                let one_minute = std::time::Duration::from_secs(60);

                while let Some(&front) = times.front() {
                    if now.duration_since(front) > one_minute {
                        times.pop_front();
                    } else {
                        break;
                    }
                }

                if times.len() < self.rpm_limit {
                    times.push_back(now);
                    None
                } else {
                    times.front().map(|oldest| {
                        one_minute
                            .checked_sub(now.duration_since(*oldest))
                            .unwrap_or(std::time::Duration::from_millis(100))
                    })
                }
            };

            match wait_duration {
                None => return Ok(()),
                Some(duration) => {
                    tokio::select! {
                        _ = tokio::time::sleep(duration) => {}
                        _ = cancel.cancelled() => return Err(PermitCancelled),
                    }
                }
            }
        }
    }
}

/// ASR 速率控制器（100 RPM API 上限，留 10% 余量 → 90 RPM，8 并发）
pub struct AsrRateLimiter(pub RateLimiter);

/// Chat 速率控制器（Pro 系列 100 RPM，留 10% 余量 → 90 RPM，3 并发）
pub struct ChatRateLimiter(pub RateLimiter);

pub fn create_asr_limiter() -> AsrRateLimiter {
    AsrRateLimiter(RateLimiter::new(
        crate::constants::ASR_MAX_CONCURRENT,
        crate::constants::ASR_RPM_LIMIT,
    ))
}

pub fn create_chat_limiter() -> ChatRateLimiter {
    ChatRateLimiter(RateLimiter::new(
        crate::constants::CHAT_MAX_CONCURRENT,
        crate::constants::CHAT_RPM_LIMIT,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_permit_returns_and_releases() {
        let limiter = RateLimiter::new(1, 90);
        let token = CancelToken::new();
        let permit = limiter
            .acquire_permit(&token)
            .await
            .expect("首次获取应成功");
        drop(permit);
        let permit = limiter
            .acquire_permit(&token)
            .await
            .expect("释放后应可再次获取");
        drop(permit);
    }

    #[tokio::test]
    async fn acquire_permit_interrupted_by_cancel() {
        let limiter = RateLimiter::new(1, 90);
        let token = CancelToken::new();
        let hold = limiter.acquire_permit(&token).await.expect("占满并发槽");
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        let r = limiter.acquire_permit(&token).await;
        assert!(r.is_err(), "排队中的 permit 应被取消打断");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        drop(hold);
    }

    #[tokio::test]
    async fn rpm_slot_blocks_when_limit_reached_and_cancel_interrupts() {
        // rpm=2：前两次立即通过并记账，第三次需等待约 60s，用取消打断验证
        let limiter = RateLimiter::new(8, 2);
        let token = CancelToken::new();
        limiter.wait_rpm_slot(&token).await.expect("第 1 次应通过");
        limiter.wait_rpm_slot(&token).await.expect("第 2 次应通过");
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        let r = limiter.wait_rpm_slot(&token).await;
        assert!(r.is_err(), "RPM 排队应被取消打断");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[tokio::test]
    async fn rpm_accounting_counts_each_request() {
        // rpm=3：连续记账 3 次后窗口已满，第 4 次必然进入等待分支
        let limiter = RateLimiter::new(8, 3);
        let token = CancelToken::new();
        for _ in 0..3 {
            limiter.wait_rpm_slot(&token).await.expect("窗口内应通过");
        }
        let times = limiter.request_times.lock().unwrap();
        assert_eq!(times.len(), 3, "每次真实请求都应独立记账");
    }
}
