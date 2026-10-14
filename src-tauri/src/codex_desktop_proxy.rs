//! Codex 官方域名的本地 CONNECT 转发与系统 PAC 生命周期。
//! PAC 覆盖系统代理请求；固定 hosts 映射补充公共云 WebSocket 的原生连接。

use crate::{
    codex_system_proxy::{self, SystemProxy},
    database::Database,
    store::AppState,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{oneshot, Mutex},
    task::{JoinHandle, JoinSet},
};
use toml_edit::{DocumentMut, Item, Table};

pub(crate) const ENABLED_KEY: &str = "codex_desktop_proxy_enabled";
const FEATURE: &str = "respect_system_proxy";
const JOURNAL: &str = "codex-desktop-proxy-backup.json";
const DOMAINS: &[&str] = &[
    "openai.com",
    "chatgpt.com",
    "oaistatic.com",
    "oaiusercontent.com",
];
const HEAD_LIMIT: usize = 16384;
// 每个方向 64 KiB，减少大正文转发时的读写次数；128 个隧道的转发缓冲区最多占用 16 MiB。
const TUNNEL_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProxyStatus {
    pub supported: bool,
    pub enabled: bool,
    pub running: bool,
    pub system_proxy_applied: bool,
    pub backend_config_applied: bool,
    pub websocket_forwarding_applied: bool,
    pub error: Option<String>,
}

/// 不保存认证或整个 Codex 配置，只记录恢复所需的系统设置和原功能键。
#[derive(Serialize, Deserialize)]
struct Backup {
    previous: SystemProxy,
    installed: SystemProxy,
    config_path: PathBuf,
    config_existed: bool,
    features_existed: bool,
    original_feature: Option<String>,
    #[serde(default)]
    hosts_owner: Option<String>,
}

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
    installed: SystemProxy,
    hosts_owner: String,
}

#[derive(Default)]
struct Runtime {
    running: Option<Running>,
    error: Option<String>,
    stopping: bool,
}

/// 开关、启动恢复、退出和上游配置保存共享 gate，防止多次启停互相覆盖。
#[derive(Default)]
pub(crate) struct DesktopProxy {
    pub gate: Mutex<()>,
    runtime: Mutex<Runtime>,
}

impl DesktopProxy {
    pub async fn status(&self, db: &Database) -> Result<DesktopProxyStatus, String> {
        // 从本机设置读取期望状态；运行状态不作为远程连通成功的证据。
        let requested = db
            .get_bool_flag(ENABLED_KEY)
            .map_err(|error| error.to_string())?;
        let runtime = self.runtime.lock().await;
        // 恢复失败仍保留端口或日志时，开关保持可关闭，允许用户重试清理。
        let journal = crate::config::get_app_config_dir().join(JOURNAL);
        let enabled = requested || runtime.running.is_some() || journal.exists();
        let mut running = false;
        let mut system_proxy_applied = false;
        let mut backend_config_applied = false;
        let mut websocket_forwarding_applied = false;
        if let Some(service) = &runtime.running {
            running = !service.task.is_finished();
            // 外部 VPN 修改后只显示配置失效，不自动覆盖其新设置。
            let current = codex_system_proxy::read()?;
            system_proxy_applied = current.flags == service.installed.flags
                && current.pac_url == service.installed.pac_url;
            // 映射失效时不把 PAC 局部就绪显示为完整转发就绪。
            websocket_forwarding_applied = crate::codex_hosts::applied(&service.hosts_owner)?;
            // 检查功能键仍存在，避免用户或供应商编辑后继续显示配置就绪。
            if let Ok(config) = crate::codex_config::read_codex_config_text() {
                backend_config_applied =
                    crate::codex_config::codex_config_feature_enabled(&config, FEATURE);
            }
        }
        Ok(DesktopProxyStatus {
            supported: cfg!(target_os = "windows"),
            enabled,
            running,
            system_proxy_applied,
            backend_config_applied,
            websocket_forwarding_applied,
            error: runtime.error.clone(),
        })
    }

    pub async fn initialize(
        &self,
        state: &AppState,
        recovery_error: Option<String>,
    ) -> Result<(), String> {
        // 与供应商配置写入共用 Codex 锁，避免启动投影与功能键更新互相覆盖。
        let _switch = state.proxy_service.lock_switch_for_app("codex").await;
        let _gate = self.gate.lock().await;
        let mut runtime = self.runtime.lock().await;
        // 设置页可能先于后台启动操作；已经启用或正在退出时不重复恢复。
        if runtime.running.is_some() || runtime.stopping {
            return Ok(());
        }
        if let Some(error) = recovery_error {
            // 启动前已经拒绝恢复权限，不重复弹 UAC；保留错误供用户主动重试。
            runtime.error = Some(error.clone());
            return Err(error);
        }
        // 先恢复上次异常退出留下的设置，再决定是否重启本地转发。
        let result = async {
            restore()?;
            let enabled = state
                .db
                .get_bool_flag(ENABLED_KEY)
                .map_err(|error| error.to_string())?;
            if enabled {
                activate(state.db.clone(), &mut runtime).await?;
            }
            Ok(())
        }
        .await;
        runtime.error = result.as_ref().err().cloned();
        result
    }

    pub async fn set_enabled(&self, state: &AppState, enabled: bool) -> Result<(), String> {
        // 1. 串行化配置与服务状态；已经启用时不重建监听端口。
        let _switch = state.proxy_service.lock_switch_for_app("codex").await;
        let _gate = self.gate.lock().await;
        let mut runtime = self.runtime.lock().await;
        if runtime.stopping {
            return Err("CCS 正在退出 / CCS is shutting down".into());
        }
        let result = async {
            if enabled && runtime.running.is_none() {
                // 恢复未完成的启动或上次异常退出，再安装当前配置。
                restore()?;
                activate(state.db.clone(), &mut runtime).await?;
            } else if !enabled {
                // 系统设置恢复失败时保留服务与备份，不留下指向死端口的 PAC。
                restore()?;
                if let Some(service) = runtime.running.take() {
                    let _ = service.stop.send(());
                    let _ = service.task.await;
                }
            }
            // 2. 运行态操作成功才持久化开关；数据库写失败时撤销新启动。
            let saved = state
                .db
                .set_setting(ENABLED_KEY, if enabled { "true" } else { "false" });
            if let Err(error) = saved {
                if enabled {
                    restore()?;
                    if let Some(service) = runtime.running.take() {
                        let _ = service.stop.send(());
                        let _ = service.task.await;
                    }
                }
                return Err(error.to_string());
            }
            Ok(())
        }
        .await;
        runtime.error = result.as_ref().err().cloned();
        result
    }

    pub async fn stop(&self, state: &AppState) -> Result<(), String> {
        // 退出只停止运行态，保留用户的启用选项供下次启动使用。
        let _switch = state.proxy_service.lock_switch_for_app("codex").await;
        let _gate = self.gate.lock().await;
        let mut runtime = self.runtime.lock().await;
        runtime.stopping = true;
        // 先恢复系统与 Codex 配置，再关闭监听器和所有存量隧道。
        if let Err(error) = restore() {
            runtime.stopping = false;
            runtime.error = Some(error.clone());
            return Err(error);
        }
        if let Some(service) = runtime.running.take() {
            let _ = service.stop.send(());
            let _ = service.task.await;
        }
        Ok(())
    }
}

/// 首版只使用已有 HTTP/HTTPS CONNECT 上游；连接阶段另校验本地端口环路。
pub(crate) fn validate_upstream(value: Option<&str>) -> Result<(), String> {
    let proxy =
        value.ok_or("请先保存 HTTP/HTTPS 出站代理 / Save an HTTP/HTTPS upstream proxy first")?;
    let parsed = url::Url::parse(proxy).map_err(|_| "出站代理地址无效 / Invalid upstream proxy")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(
            "Codex 桌面域名代理首版需要 HTTP/HTTPS 上游 / HTTP/HTTPS upstream required".into(),
        );
    }
    let host = parsed.host_str().unwrap_or("");
    let loopback = host == "localhost"
        || host == "[::1]"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if host.eq_ignore_ascii_case(crate::codex_hosts::CLOUD_HOST)
        || (loopback && parsed.port_or_known_default() == Some(443))
    {
        return Err(
            "出站代理会绕回云端本地转发 / Upstream loops into local cloud forwarding".into(),
        );
    }
    if !parsed.username().is_empty() && parsed.username().contains(':') {
        return Err("代理用户名不能包含冒号 / Proxy username cannot contain a colon".into());
    }
    Ok(())
}

async fn activate(db: Arc<Database>, runtime: &mut Runtime) -> Result<(), String> {
    // 1. 检查上游与已有系统代理；不替换 VPN 的手动代理或已有 PAC。
    let upstream = db
        .get_effective_global_proxy_url()
        .map_err(|error| error.to_string())?;
    validate_upstream(upstream.as_deref())?;
    let previous = codex_system_proxy::read()?;
    if previous.flags & (2 | 4) != 0 {
        return Err("已有系统代理/PAC 正在使用，请先关闭后再启用 / An existing system proxy or PAC is active".into());
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    // 固定域名连接使用原端口 443；先绑定两个地址，失败时尚未修改 hosts 或 PAC。
    let cloud_v4 = TcpListener::bind((crate::codex_hosts::CLOUD_ADDRESS, 443))
        .await
        .map_err(|_| "本机 127.0.0.2:443 不可用，未启用 / Local 127.0.0.2:443 unavailable")?;
    let cloud_v6 = TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 443))
        .await
        .map_err(|_| "本机 [::1]:443 不可用，未启用 / Local [::1]:443 unavailable")?;
    let hosts_owner = uuid::Uuid::new_v4().to_string();
    let path = format!("/codex-{}.pac", uuid::Uuid::new_v4());
    let mut installed = previous.clone();
    installed.flags = 1 | 4;
    installed.pac_url = Some(format!("http://127.0.0.1:{port}{path}"));

    // 2. 仅备份功能键；保留供应商、认证及其他 Codex 设置。
    let config_path = crate::codex_config::get_codex_config_path();
    let config_existed = config_path.exists();
    let config =
        crate::codex_config::read_codex_config_text().map_err(|error| error.to_string())?;
    let mut doc = config
        .parse::<DocumentMut>()
        .map_err(|_| "Codex config.toml 无法解析 / Invalid Codex config.toml")?;
    let features_existed = doc.contains_key("features");
    let mut original_feature = None;
    if let Some(features) = doc.get("features") {
        if features.as_table_like().is_none() {
            return Err("Codex features 必须为表 / Codex features must be a table".into());
        }
        if let Some(feature) = features.get(FEATURE) {
            let mut original = DocumentMut::new();
            original["features"] = Item::Table(Table::new());
            original["features"][FEATURE] = feature.clone();
            original_feature = Some(original.to_string());
        }
    } else {
        doc["features"] = Item::Table(Table::new());
    }
    doc["features"][FEATURE] = toml_edit::value(true);
    let backup = Backup {
        previous,
        installed: installed.clone(),
        config_path: config_path.clone(),
        config_existed,
        features_existed,
        original_feature,
        hosts_owner: Some(hosts_owner.clone()),
    };
    let journal = crate::config::get_app_config_dir().join(JOURNAL);
    let bytes = serde_json::to_vec_pretty(&backup).map_err(|error| error.to_string())?;
    // 写前日志先落盘，任意一步失败或异常退出均可恢复。
    crate::config::atomic_write_private(&journal, &bytes).map_err(|error| error.to_string())?;

    // 3. 监听器先运行，再发布 PAC；上游凭据始终留在 CCS 内存和现有设置中。
    let (stop, receiver) = oneshot::channel();
    let task = tokio::spawn(serve(
        listener,
        [cloud_v4, cloud_v6],
        db,
        path,
        port,
        receiver,
    ));
    runtime.running = Some(Running {
        stop,
        task,
        installed: installed.clone(),
        hosts_owner: hosts_owner.clone(),
    });
    let result = (|| {
        crate::config::write_text_file_private(&config_path, &doc.to_string())
            .map_err(|error| error.to_string())?;
        codex_system_proxy::write(&installed)?;
        let current = codex_system_proxy::read()?;
        if current.flags != installed.flags || current.pac_url != installed.pac_url {
            return Err("系统 PAC 未生效 / System PAC was not applied".into());
        }
        // 最后通过固定辅助操作写 hosts；拒绝权限将恢复先前 PAC 和功能键。
        crate::codex_hosts::change(&hosts_owner, true)?;
        // DNS 地址和监听器都已准备好后才允许返回开启成功。
        if !crate::codex_hosts::applied(&hosts_owner)? {
            return Err("云端 hosts 映射未生效 / Cloud hosts mapping was not applied".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        // 恢复失败保留日志以便重启继续恢复；返回真实失败，禁止显示就绪。
        let restored = restore();
        if restored.is_ok() {
            if let Some(service) = runtime.running.take() {
                let _ = service.stop.send(());
                let _ = service.task.await;
            }
        }
        return Err(match restored {
            Ok(()) => error,
            Err(failure) => format!("{error}; 恢复失败 / Restore failed: {failure}"),
        });
    }
    Ok(())
}

pub(crate) fn restore() -> Result<(), String> {
    // 1. 无日志时无副作用；从日志读取固定恢复对象，不输出其中内容。
    let journal = crate::config::get_app_config_dir().join(JOURNAL);
    let bytes = match std::fs::read(&journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let backup: Backup = serde_json::from_slice(&bytes)
        .map_err(|_| "代理恢复记录无效 / Invalid proxy recovery record")?;
    if let Some(owner) = &backup.hosts_owner {
        // 先移除固定映射，再恢复 PAC；权限拒绝时保留监听和恢复记录。
        crate::codex_hosts::change(owner, false)?;
    }
    let current = codex_system_proxy::read()?;
    if current.flags == backup.installed.flags && current.pac_url == backup.installed.pac_url {
        // 仅撤销仍属于 CCS 的 PAC；VPN 后来写入的配置保持原样。
        codex_system_proxy::write(&backup.previous)?;
    }

    // 2. 只恢复仍为本功能写入值的功能键，不覆盖其余 Codex 配置。
    if backup.config_path.exists() {
        let config =
            std::fs::read_to_string(&backup.config_path).map_err(|error| error.to_string())?;
        let mut doc = config.parse::<DocumentMut>().map_err(|_| {
            "Codex 配置已损坏，保留恢复记录 / Invalid Codex config; recovery record retained"
        })?;
        let current_feature = doc
            .get("features")
            .and_then(|features| features.get(FEATURE))
            .and_then(Item::as_bool);
        if current_feature == Some(true) {
            if let Some(original) = backup.original_feature {
                let original = original
                    .parse::<DocumentMut>()
                    .map_err(|error| error.to_string())?;
                doc["features"][FEATURE] = original["features"][FEATURE].clone();
            } else if let Some(features) = doc.get_mut("features").and_then(Item::as_table_like_mut)
            {
                features.remove(FEATURE);
            }
            let empty_features = doc
                .get("features")
                .and_then(Item::as_table_like)
                .is_some_and(|features| features.is_empty());
            if empty_features && !backup.features_existed {
                doc.remove("features");
            }
            if !backup.config_existed && doc.is_empty() {
                std::fs::remove_file(&backup.config_path).map_err(|error| error.to_string())?;
            } else {
                crate::config::write_text_file_private(&backup.config_path, &doc.to_string())
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    // 3. 所有需要恢复的写入完成后才删除日志。
    std::fs::remove_file(journal).map_err(|error| error.to_string())?;
    Ok(())
}

async fn serve(
    listener: TcpListener,
    cloud: [TcpListener; 2],
    db: Arc<Database>,
    pac_path: String,
    port: u16,
    mut stop: oneshot::Receiver<()>,
) {
    // PAC 与 CONNECT 共用独立回环监听，不改变已有模型路由端口。
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            Some(_) = connections.join_next() => {},
            accepted = cloud[0].accept(), if connections.len() < 128 => {
                if let Ok((stream, _)) = accepted {
                    let db = db.clone();
                    connections.spawn(async move {
                        // 原生 TLS 连接仅转发固定公共云目标，不读取或解密协议内容。
                        if let Err(error) = cloud_connection(stream, &db, port).await {
                            log::warn!("[CodexDesktopProxy] cloud tunnel failed: {error}");
                        }
                    });
                } else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            accepted = cloud[1].accept(), if connections.len() < 128 => {
                if let Ok((stream, _)) = accepted {
                    let db = db.clone();
                    connections.spawn(async move {
                        // IPv6 与 IPv4 使用同一个固定目标和失败行为。
                        if let Err(error) = cloud_connection(stream, &db, port).await {
                            log::warn!("[CodexDesktopProxy] cloud tunnel failed: {error}");
                        }
                    });
                } else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            accepted = listener.accept(), if connections.len() < 128 => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => {
                        log::error!("[CodexDesktopProxy] listener accept failed: {error}");
                        // 与现有路由服务一致：临时系统资源不足时保留监听，退出信号仍可结束。
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let db = db.clone();
                let pac_path = pac_path.clone();
                connections.spawn(async move {
                    if connection(stream, &db, &pac_path, port).await.is_err() {
                        log::debug!("[CodexDesktopProxy] local connection closed");
                    }
                });
            }
        }
    }
    // 停止时终止本功能创建的所有隧道，避免退出后残留上游连接。
    connections.shutdown().await;
}

async fn connection(
    mut stream: TcpStream,
    db: &Database,
    pac_path: &str,
    port: u16,
) -> Result<(), String> {
    // 本地 TLS 握手和交互请求立即发送，避免小包等待合并。
    stream
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;

    // 1. 有界读取本地 HTTP 头，保留客户端提前发送的 TLS 字节。
    let mut bytes = Vec::new();
    let head_size = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut buffer = [0; 2048];
            let length = stream
                .read(&mut buffer)
                .await
                .map_err(|error| error.to_string())?;
            if length == 0 {
                return Err("Incomplete local request".to_string());
            }
            bytes.extend_from_slice(&buffer[..length]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                return Ok(end + 4);
            }
            if bytes.len() >= HEAD_LIMIT {
                return Err("Local request headers too large".into());
            }
        }
    })
    .await
    .map_err(|_| "Local request timeout")??;
    if head_size > HEAD_LIMIT {
        return Err("Local request headers too large".into());
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    request
        .parse(&bytes[..head_size])
        .map_err(|_| "Invalid local request")?;
    let method = request.method.unwrap_or("");
    let path = request.path.unwrap_or("");
    if method == "GET" && path == pac_path {
        // 白名单与 CONNECT 校验共享域名集；其他域名 DIRECT，不占远程带宽。
        let mut rules = Vec::new();
        for domain in DOMAINS {
            rules.push(format!(
                "host === '{domain}' || dnsDomainIs(host, '.{domain}')"
            ));
        }
        let body = format!("function FindProxyForURL(url, host) {{ host = host.toLowerCase(); if ((url.indexOf('https:') === 0 || url.indexOf('wss:') === 0) && ({})) return 'PROXY 127.0.0.1:{port}'; return 'DIRECT'; }}", rules.join(" || "));
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream
            .write_all(response.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let authority = path.parse::<http::uri::Authority>().ok();
    let allowed = authority.as_ref().is_some_and(|value| {
        let host = value.host().to_ascii_lowercase();
        value.port_u16() == Some(443)
            && DOMAINS
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    });
    if method != "CONNECT" || !allowed {
        stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let host = authority.unwrap().host().to_owned();

    // 2. 两种本地入口共用当前出站配置、连接超时和环路校验。
    let mut upstream = match open_upstream(db, &host, port).await {
        Ok(stream) => stream,
        Err(error) => {
            stream
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .map_err(|error| error.to_string())?;
            log::warn!("[CodexDesktopProxy] upstream CONNECT failed: {host}:443: {error}");
            return Ok(());
        }
    };
    // 3. 上游成功后才报告 200；透传目标 TLS，不解密、不读取请求正文。
    stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .map_err(|error| error.to_string())?;
    upstream
        .write_all(&bytes[head_size..])
        .await
        .map_err(|error| error.to_string())?;
    // 使用有界缓冲透传大响应，保留双向并发和半关闭行为。
    tokio::io::copy_bidirectional_with_sizes(
        &mut stream,
        &mut upstream,
        TUNNEL_BUFFER_SIZE,
        TUNNEL_BUFFER_SIZE,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// PAC 和固定域名转发共用 CONNECT；上游失败绝不回退直连。
async fn open_upstream(
    db: &Database,
    host: &str,
    pac_port: u16,
) -> Result<crate::proxy::hyper_client::ProxyStream, String> {
    // 桌面入口以 CONNECT 建立结果单独统计，不与 HTTP 请求成功率混算。
    let mut attempt = crate::proxy::outbound_stats::OutboundAttempt::new(true);
    // 1. 每次连接读取现有代理设置，不在 hosts 或参数中保存认证。
    let proxy = db
        .get_effective_global_proxy_url()
        .map_err(|error| error.to_string())?;
    validate_upstream(proxy.as_deref())?;
    let proxy = proxy.ok_or("Missing upstream proxy")?;
    let parsed = url::Url::parse(&proxy).map_err(|_| "Invalid upstream proxy")?;
    let address = parsed.host_str().unwrap_or("");
    let local = address == "localhost"
        || address == "[::1]"
        || address
            .parse::<std::net::IpAddr>()
            .is_ok_and(|value| value.is_loopback());
    if local
        && (parsed.port_or_known_default() == Some(pac_port)
            || parsed.port_or_known_default() == Some(443))
    {
        return Err("Upstream proxy loop".into());
    }
    // 2. 目标始终由远程代理解析，避免 hosts 映射令 CCS 自己绕回本机。
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        crate::proxy::hyper_client::connect_via_proxy(&proxy, host, 443),
    )
    .await
    .map_err(|_| "Upstream CONNECT timed out")?;
    if result.is_ok() {
        // 隧道建立成功；后续登录或长连接传输结果不由此指标推断。
        attempt.finish(Some(200));
    }
    result.map_err(|error| error.to_string())
}

async fn cloud_connection(mut stream: TcpStream, db: &Database, port: u16) -> Result<(), String> {
    // 固定域名入口同样立即发送 TLS 和 WebSocket 小包。
    stream
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;

    // 固定转发公共云域名；HTTPS/TLS 和 WebSocket 帧都保持原字节。
    let mut upstream = open_upstream(db, crate::codex_hosts::CLOUD_HOST, port).await?;
    // 两个入口采用相同有界缓冲，避免公共云长连接仍使用默认小缓冲。
    tokio::io::copy_bidirectional_with_sizes(
        &mut stream,
        &mut upstream,
        TUNNEL_BUFFER_SIZE,
        TUNNEL_BUFFER_SIZE,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}
