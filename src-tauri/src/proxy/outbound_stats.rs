//! 实际出站请求统计；仅保留计数，不保存地址、请求内容或认证信息。

use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::{Arc, Mutex, RwLock};

/// 当前配置周期的 HTTP 响应头结果与桌面隧道连接结果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundStats {
    pub successful: u64,
    pub network_failures: u64,
    pub http_errors: u64,
    pub tunnel_successful: u64,
    pub tunnel_failures: u64,
}

static CURRENT: Lazy<RwLock<Arc<Mutex<OutboundStats>>>> =
    Lazy::new(|| RwLock::new(Arc::new(Mutex::new(OutboundStats::default()))));

/// 配置应用成功后开启新统计周期，旧请求继续写入旧周期。
pub fn reset() {
    // 更换计数实例，使切换前未完成的请求不污染新配置的统计。
    if let Ok(mut current) = CURRENT.write() {
        *current = Arc::new(Mutex::new(OutboundStats::default()));
    }
}

/// 读取当前计数；没有完成请求时由前端显示暂无数据。
pub fn snapshot() -> OutboundStats {
    // 只复制计数，不读取请求内容或地址。
    let current = CURRENT.read().unwrap_or_else(|error| error.into_inner());
    let stats = current.lock().unwrap_or_else(|error| error.into_inner());
    stats.clone()
}

/// 请求开始时绑定统计周期；外层超时取消也计入未取得响应的失败。
pub struct OutboundAttempt {
    stats: Arc<Mutex<OutboundStats>>,
    tunnel: bool,
    finished: bool,
}

impl OutboundAttempt {
    /// 绑定开始发送时的统计周期，区分 HTTP 请求与桌面 CONNECT。
    pub fn new(tunnel: bool) -> Self {
        // 复制当前周期的引用，避免完成时混入后续配置。
        let stats = CURRENT
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        Self {
            stats,
            tunnel,
            finished: false,
        }
    }

    /// 按响应头分类；无响应表示连接、TLS、超时或发送失败。
    pub fn finish(&mut self, status: Option<u16>) {
        // 每次尝试仅结算一次，不改变原有请求返回结果。
        if self.finished {
            return;
        }
        self.finished = true;
        let mut stats = self.stats.lock().unwrap_or_else(|error| error.into_inner());
        match (self.tunnel, status) {
            (true, Some(_)) => stats.tunnel_successful += 1,
            (true, None) => stats.tunnel_failures += 1,
            (false, Some(code)) if (200..400).contains(&code) => stats.successful += 1,
            (false, Some(_)) => stats.http_errors += 1,
            (false, None) => stats.network_failures += 1,
        }
    }
}

impl Drop for OutboundAttempt {
    fn drop(&mut self) {
        // 外层首包超时丢弃 future 时仍记录一次失败；已结算的尝试不重复计数。
        self.finish(None);
    }
}

/// 为共享客户端的发送入口增加统计，保留 reqwest 的响应与错误类型。
pub trait TrackedSend {
    fn send_tracked(
        self,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send;
}

impl TrackedSend for reqwest::RequestBuilder {
    async fn send_tracked(self) -> Result<reqwest::Response, reqwest::Error> {
        // 1. 本地构建失败没有发起出站请求，不纳入网络指标。
        let (client, request) = self.build_split();
        let request = request?;
        // 2. 绑定本次请求周期，执行原客户端的网络发送。
        let mut attempt = OutboundAttempt::new(false);
        let result = client.execute(request).await;
        // 3. 收到响应头时按 HTTP 状态分类，不保存错误体或认证数据。
        attempt.finish(
            result
                .as_ref()
                .ok()
                .map(|response| response.status().as_u16()),
        );
        result
    }
}
