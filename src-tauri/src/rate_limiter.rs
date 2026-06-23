use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// 速率控制器核心实现
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

    /// 获取请求许可，返回 permit（调用方持有直到请求完成）
    pub async fn acquire_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
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
                None => break,
                Some(duration) => {
                    tokio::time::sleep(duration).await;
                }
            }
        }

        self.semaphore.clone().acquire_owned().await.unwrap()
    }

    pub fn current_rpm(&self) -> usize {
        let times = self.request_times.lock().unwrap();
        let now = std::time::Instant::now();
        let one_minute = std::time::Duration::from_secs(60);
        times.iter().filter(|t| now.duration_since(**t) <= one_minute).count()
    }
}

/// ASR 速率控制器（100 RPM API 上限，留 10% 余量 → 90 RPM，3 并发）
pub struct AsrRateLimiter(pub RateLimiter);

/// Chat 速率控制器（Pro 系列 100 RPM，留 10% 余量 → 90 RPM，3 并发）
pub struct ChatRateLimiter(pub RateLimiter);

pub fn create_asr_limiter() -> AsrRateLimiter {
    AsrRateLimiter(RateLimiter::new(3, 90))
}

pub fn create_chat_limiter() -> ChatRateLimiter {
    ChatRateLimiter(RateLimiter::new(3, 90))
}
