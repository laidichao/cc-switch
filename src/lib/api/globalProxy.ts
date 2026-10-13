/**
 * 全局出站代理 API
 *
 * 提供获取、设置和测试全局代理的功能。
 */

import { invoke } from "@tauri-apps/api/core";

/**
 * 代理测试结果
 */
export interface ProxyTestResult {
  success: boolean;
  latencyMs: number;
  error: string | null;
}

/**
 * 出站代理状态
 */
export interface UpstreamProxyStatus {
  enabled: boolean;
  proxyUrl: string | null;
  stats: {
    successful: number;
    networkFailures: number;
    httpErrors: number;
    tunnelSuccessful: number;
    tunnelFailures: number;
  };
}

/**
 * 检测到的代理
 */
export interface DetectedProxy {
  url: string;
  proxyType: string;
  port: number;
}

/** 本地服务和系统 PAC 状态，不表示官方登录已经通过验证。 */
export interface CodexDesktopProxyStatus {
  supported: boolean;
  enabled: boolean;
  running: boolean;
  systemProxyApplied: boolean;
  backendConfigApplied: boolean;
  websocketForwardingApplied: boolean;
  error: string | null;
}

/** 查询桌面域名代理的实际运行状态。 */
export async function getCodexDesktopProxyStatus(): Promise<CodexDesktopProxyStatus> {
  return invoke<CodexDesktopProxyStatus>("get_codex_desktop_proxy_status");
}

/** 修改桌面域名代理开关，由后端完成设置备份及恢复。 */
export async function setCodexDesktopProxyEnabled(
  enabled: boolean,
): Promise<void> {
  return invoke("set_codex_desktop_proxy_enabled", { enabled });
}

/**
 * 获取全局代理 URL
 *
 * @returns 保存的代理 URL，关闭开关时仍保留；null 表示未配置
 */
export async function getGlobalProxyUrl(): Promise<string | null> {
  return invoke<string | null>("get_global_proxy_url");
}

/** 获取持久化的出站代理开关，地址独立保留。 */
export async function getGlobalProxyEnabled(): Promise<boolean> {
  // 读取后端兼容旧配置后的启用状态。
  return invoke<boolean>("get_global_proxy_enabled");
}

/** 切换出站代理，后端热更新客户端并保留认证配置。 */
export async function setGlobalProxyEnabled(enabled: boolean): Promise<void> {
  // 将开关交给后端持久化并应用，失败由调用方显示。
  return invoke("set_global_proxy_enabled", { enabled });
}

/**
 * 设置全局代理 URL
 *
 * @param url - 代理 URL（如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080）
 *              空字符串表示清除代理（直连）
 */
export async function setGlobalProxyUrl(url: string): Promise<void> {
  try {
    return await invoke("set_global_proxy_url", { url });
  } catch (error) {
    // Tauri invoke 错误可能是字符串
    throw new Error(typeof error === "string" ? error : String(error));
  }
}

/**
 * 测试代理连接
 *
 * @param url - 要测试的代理 URL
 * @returns 测试结果，包含是否成功、延迟和错误信息
 */
export async function testProxyUrl(url: string): Promise<ProxyTestResult> {
  return invoke<ProxyTestResult>("test_proxy_url", { url });
}

/** 测量已保存且启用的代理，探测流量不计入业务成功率。 */
export async function measureGlobalProxyLatency(
  url: string,
): Promise<ProxyTestResult> {
  // 后端验证地址仍匹配当前配置，再复用共享客户端探测固定目标。
  return invoke<ProxyTestResult>("measure_global_proxy_latency", { url });
}

/**
 * 获取当前出站代理状态
 *
 * @returns 代理状态，包含是否启用和代理 URL
 */
export async function getUpstreamProxyStatus(): Promise<UpstreamProxyStatus> {
  return invoke<UpstreamProxyStatus>("get_upstream_proxy_status");
}

/**
 * 扫描本地代理
 *
 * @returns 检测到的代理列表
 */
export async function scanLocalProxies(): Promise<DetectedProxy[]> {
  return invoke<DetectedProxy[]>("scan_local_proxies");
}
