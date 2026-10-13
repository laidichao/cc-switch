//! 全局出站代理相关命令
//!
//! 提供获取、设置和测试全局代理的 Tauri 命令。

use crate::proxy::http_client;
use crate::store::AppState;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

/// 获取全局代理 URL
///
/// 返回保存的代理 URL，关闭开关时仍返回配置；null 表示未配置。
#[tauri::command]
pub fn get_global_proxy_url(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    let result = state.db.get_global_proxy_url().map_err(|e| e.to_string())?;
    log::debug!(
        "[GlobalProxy] [GP-010] Read from database: {}",
        result
            .as_ref()
            .map(|u| http_client::mask_url(u))
            .unwrap_or_else(|| "None".to_string())
    );
    Ok(result)
}

/// 获取持久化的全局出站代理开关。
#[tauri::command]
pub fn get_global_proxy_enabled(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    // 读取开关并兼容尚未保存开关的旧配置。
    state
        .db
        .get_global_proxy_enabled()
        .map_err(|error| error.to_string())
}

/// 切换全局出站代理，保留地址及认证；运行态失败时恢复开关和客户端。
#[tauri::command]
pub async fn set_global_proxy_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    // 1. 与地址保存及桌面代理启停共用锁，避免依赖状态在检查后改变。
    let _gate = state.codex_desktop_proxy.gate.lock().await;
    // 桌面入口开启时必须保留其上游，先要求关闭桌面入口。
    let desktop_enabled = state
        .db
        .get_bool_flag(crate::codex_desktop_proxy::ENABLED_KEY)
        .map_err(|error| error.to_string())?;
    if !enabled && desktop_enabled {
        return Err("请先关闭 Codex 桌面代理 / Disable the Codex desktop proxy first".into());
    }
    // 读取已保存地址，启用前要求存在可用配置。
    let saved = state
        .db
        .get_global_proxy_url()
        .map_err(|error| error.to_string())?;
    if enabled && saved.is_none() {
        return Err("请先保存代理地址 / Save a proxy address first".into());
    }
    let effective = if enabled { saved.as_deref() } else { None };
    // 构建客户端前验证配置，不执行网络请求。
    http_client::validate_proxy(effective)?;
    // 2. 保存原状态供运行态更新失败时恢复。
    let previous = state
        .db
        .get_global_proxy_enabled()
        .map_err(|error| error.to_string())?;
    // 只写开关，保留代理地址和认证配置。
    state
        .db
        .set_setting(
            "global_proxy_enabled",
            if enabled { "true" } else { "false" },
        )
        .map_err(|error| error.to_string())?;
    // 3. 热更新共享客户端；失败时恢复持久化状态及旧客户端。
    if let Err(error) = http_client::apply_proxy(effective) {
        // 恢复原开关，避免重启后应用失败的状态。
        state
            .db
            .set_setting(
                "global_proxy_enabled",
                if previous { "true" } else { "false" },
            )
            .map_err(|failure| format!("{error}; restore failed: {failure}"))?;
        let previous_url = if previous { saved.as_deref() } else { None };
        // 恢复可能已部分更新的共享客户端。
        http_client::apply_proxy(previous_url)
            .map_err(|failure| format!("{error}; client restore failed: {failure}"))?;
        return Err(error);
    }
    Ok(())
}

/// 设置全局代理 URL
///
/// - 传入非空字符串：保存代理，独立开关关闭时不启用
/// - 传入空字符串：清除代理（直连）
///
/// 执行顺序：先验证 → 写 DB → 再应用
/// 这样确保 DB 写失败时不会出现运行态与持久化不一致的问题
#[tauri::command]
pub async fn set_global_proxy_url(
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<(), String> {
    // 保存上游与桌面域名代理开关串行执行，防止启用期间丢失上游配置。
    let _gate = state.codex_desktop_proxy.gate.lock().await;
    // 调试：显示接收到的 URL 信息（不包含敏感内容）
    let has_auth = url.contains('@') && (url.starts_with("http://") || url.starts_with("socks"));
    log::debug!(
        "[GlobalProxy] [GP-011] Received URL: length={}, has_auth={}",
        url.len(),
        has_auth
    );

    let url_opt = if url.trim().is_empty() {
        None
    } else {
        Some(url.as_str())
    };

    // 桌面入口首版依赖 CONNECT；普通出站设置仍支持原有 SOCKS 协议。
    let desktop_enabled = state
        .db
        .get_bool_flag(crate::codex_desktop_proxy::ENABLED_KEY)
        .map_err(|error| error.to_string())?;
    if desktop_enabled {
        crate::codex_desktop_proxy::validate_upstream(url_opt)?;
    }

    // 1. 先验证代理配置是否有效（不应用）
    http_client::validate_proxy(url_opt)?;

    // 2. 验证成功后保存到数据库
    // 留存旧值，仅在运行态更新失败时恢复数据库，避免新配置与客户端不一致。
    let previous = state
        .db
        .get_global_proxy_url()
        .map_err(|error| error.to_string())?;
    // 读取原开关，关闭时保存地址不启用显式出站代理。
    let previous_enabled = state
        .db
        .get_global_proxy_enabled()
        .map_err(|error| error.to_string())?;
    // 首次保存且没有显式关闭记录时沿用原有自动启用行为。
    let enabled_flag = state
        .db
        .get_setting("global_proxy_enabled")
        .map_err(|error| error.to_string())?;
    let effective_url =
        url_opt.filter(|_| matches!(enabled_flag.as_deref(), None | Some("true") | Some("1")));
    // 写入新地址，但保留独立开关。
    state
        .db
        .set_global_proxy_url(url_opt)
        .map_err(|e| e.to_string())?;

    // 3. DB 写入成功后再应用到运行态
    if let Err(error) = http_client::apply_proxy(effective_url) {
        // 回滚数据库；共享客户端可能已完成部分更新，因此也恢复原客户端配置。
        state
            .db
            .set_global_proxy_url(previous.as_deref())
            .map_err(|failure| format!("{error}; restore failed: {failure}"))?;
        // 确保 CCS 转发与桌面入口重新使用同一个上游。
        http_client::apply_proxy(previous.as_deref().filter(|_| previous_enabled))
            .map_err(|failure| format!("{error}; client restore failed: {failure}"))?;
        return Err(error);
    }

    log::info!(
        "[GlobalProxy] [GP-009] Configuration updated: {}",
        effective_url
            .map(http_client::mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 查询本地转发与系统 PAC 状态；不将就绪等同于远程或桌面登录成功。
#[tauri::command]
pub async fn get_codex_desktop_proxy_status(
    state: tauri::State<'_, AppState>,
) -> Result<crate::codex_desktop_proxy::DesktopProxyStatus, String> {
    // 读取持久化意图和当前服务状态供设置页显示。
    state.codex_desktop_proxy.status(&state.db).await
}

/// 在现有出站设置旁启用桌面官方域名代理，不启动或重启 Codex。
#[tauri::command]
pub async fn set_codex_desktop_proxy_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    // 生命周期负责串行化、配置所有权和失败回滚。
    state.codex_desktop_proxy.set_enabled(&state, enabled).await
}

/// 代理测试结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    /// 是否连接成功
    pub success: bool,
    /// 延迟（毫秒）
    pub latency_ms: u64,
    /// 错误信息
    pub error: Option<String>,
}

/// 测试代理连接
///
/// 通过指定的代理 URL 发送测试请求，返回连接结果和延迟。
/// 使用多个测试目标，任一成功即认为代理可用。
#[tauri::command]
pub async fn test_proxy_url(url: String) -> Result<ProxyTestResult, String> {
    if url.trim().is_empty() {
        return Err("Proxy URL is empty".to_string());
    }

    let start = Instant::now();

    // 构建带代理的临时客户端
    let proxy = reqwest::Proxy::all(&url).map_err(|e| format!("Invalid proxy URL: {e}"))?;

    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {e}"))?;

    // 使用多个测试目标，提高兼容性
    // 优先使用 httpbin（专门用于 HTTP 测试），回退到其他公共端点
    let test_urls = [
        "https://httpbin.org/get",
        "https://www.google.com",
        "https://api.anthropic.com",
    ];

    let mut last_error = None;

    for test_url in test_urls {
        match client.head(test_url).send().await {
            Ok(resp) => {
                let latency = start.elapsed().as_millis() as u64;
                log::debug!(
                    "[GlobalProxy] Test successful: {} -> {} via {} ({}ms)",
                    http_client::mask_url(&url),
                    test_url,
                    resp.status(),
                    latency
                );
                return Ok(ProxyTestResult {
                    success: true,
                    latency_ms: latency,
                    error: None,
                });
            }
            Err(e) => {
                log::debug!("[GlobalProxy] Test to {test_url} failed: {e}");
                last_error = Some(e);
            }
        }
    }

    // 所有测试目标都失败
    let latency = start.elapsed().as_millis() as u64;
    let error_msg = last_error
        .map(|e| e.to_string())
        .unwrap_or_else(|| "All test targets failed".to_string());

    log::debug!(
        "[GlobalProxy] Test failed: {} -> {} ({}ms)",
        http_client::mask_url(&url),
        error_msg,
        latency
    );

    Ok(ProxyTestResult {
        success: false,
        latency_ms: latency,
        error: Some(error_msg),
    })
}

/// 获取当前出站代理状态
///
/// 返回当前是否启用了出站代理以及代理 URL。
#[tauri::command]
pub fn get_upstream_proxy_status() -> UpstreamProxyStatus {
    let url = http_client::get_current_proxy_url();
    UpstreamProxyStatus {
        enabled: url.is_some(),
        proxy_url: url,
        // 返回当前配置周期的计数，前端据此区分网络失败与 HTTP 错误。
        stats: crate::proxy::outbound_stats::snapshot(),
    }
}

/// 测量已启用代理到固定检测地址的响应头延迟，不计入业务请求成功率。
#[tauri::command]
pub async fn measure_global_proxy_latency(
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<ProxyTestResult, String> {
    // 1. 与代理切换共用锁，只在配置匹配时取得当前客户端。
    let client = {
        let _gate = state.codex_desktop_proxy.gate.lock().await;
        // 校验前端图表对应的地址仍是已启用的配置，避免切换后混入旧结果。
        let current = state
            .db
            .get_effective_global_proxy_url()
            .map_err(|_| "Unable to read proxy configuration")?;
        if current.as_deref() != Some(url.as_str()) {
            return Err("Proxy configuration changed or disabled".into());
        }
        // 复用全局客户端连接池，不新建代理配置或持有锁进行网络等待。
        http_client::get()
    };
    // 2. 固定 HEAD 请求测量连接及响应头耗时，最多等待 10 秒。
    let start = Instant::now();
    let result = client
        .head("https://httpbin.org/get")
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let latency_ms = start.elapsed().as_millis() as u64;
    // 3. 探测失败不填入虚假的零延迟，也不输出代理地址和认证数据。
    let (success, error) = match result {
        Ok(response) if response.status().is_success() => (true, None),
        Ok(response) => (false, Some(format!("HTTP {}", response.status().as_u16()))),
        Err(error) if error.is_timeout() => (false, Some("Request timed out".into())),
        Err(_) => (false, Some("Connection or TLS failure".into())),
    };
    Ok(ProxyTestResult {
        success,
        latency_ms,
        error,
    })
}

/// 出站代理状态信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamProxyStatus {
    /// 是否启用代理
    pub enabled: bool,
    /// 代理 URL
    pub proxy_url: Option<String>,
    /// 当前配置周期内实际完成的出站请求统计。
    pub stats: crate::proxy::outbound_stats::OutboundStats,
}

/// 检测到的代理信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProxy {
    /// 代理 URL
    pub url: String,
    /// 代理类型 (http/socks5)
    pub proxy_type: String,
    /// 端口
    pub port: u16,
}

/// 常见代理端口配置
/// 格式：(端口, 主要类型, 是否同时支持 http 和 socks5)
/// 对于 mixed 端口，会同时返回两种协议供用户选择
const PROXY_PORTS: &[(u16, &str, bool)] = &[
    (7890, "http", true),     // Clash (mixed mode)
    (7891, "socks5", false),  // Clash SOCKS only
    (1080, "socks5", false),  // 通用 SOCKS5
    (8080, "http", false),    // 通用 HTTP
    (8888, "http", false),    // Charles/Fiddler
    (3128, "http", false),    // Squid
    (10808, "socks5", false), // V2Ray SOCKS
    (10809, "http", false),   // V2Ray HTTP
];

/// 扫描本地代理
///
/// 检测常见端口是否有代理服务在运行。
/// 使用异步任务避免阻塞 UI 线程。
#[tauri::command]
pub async fn scan_local_proxies() -> Vec<DetectedProxy> {
    // 使用 spawn_blocking 避免阻塞主线程
    tokio::task::spawn_blocking(|| {
        let mut found = Vec::new();

        for &(port, primary_type, is_mixed) in PROXY_PORTS {
            let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            if TcpStream::connect_timeout(&addr.into(), Duration::from_millis(100)).is_ok() {
                // 添加主要类型
                found.push(DetectedProxy {
                    url: format!("{primary_type}://127.0.0.1:{port}"),
                    proxy_type: primary_type.to_string(),
                    port,
                });
                // 对于 mixed 端口，同时添加另一种协议
                if is_mixed {
                    let alt_type = if primary_type == "http" {
                        "socks5"
                    } else {
                        "http"
                    };
                    found.push(DetectedProxy {
                        url: format!("{alt_type}://127.0.0.1:{port}"),
                        proxy_type: alt_type.to_string(),
                        port,
                    });
                }
            }
        }

        found
    })
    .await
    .unwrap_or_default()
}
