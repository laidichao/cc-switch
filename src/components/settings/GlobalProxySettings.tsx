/**
 * 全局出站代理设置组件
 *
 * 提供配置全局代理的输入界面，支持用户名密码认证。
 */

import { useState, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { HoverTip } from "@/components/ui/hover-tip";
import { HelpTip } from "@/components/ui/help-tip";
import { GlobalProxyLatencyChart } from "./GlobalProxyLatencyChart";
import { Loader2, TestTube2, Search, Eye, EyeOff, X } from "lucide-react";
import {
  useGlobalProxyUrl,
  useSetGlobalProxyUrl,
  useGlobalProxyEnabled,
  useSetGlobalProxyEnabled,
  useUpstreamProxyStatus,
  useTestProxy,
  useScanProxies,
  useCodexDesktopProxyStatus,
  useSetCodexDesktopProxyEnabled,
  type DetectedProxy,
} from "@/hooks/useGlobalProxy";

/** 从完整 URL 提取认证信息 */
function extractAuth(url: string): {
  baseUrl: string;
  username: string;
  password: string;
} {
  if (!url.trim()) return { baseUrl: "", username: "", password: "" };

  try {
    const parsed = new URL(url);
    const username = decodeURIComponent(parsed.username || "");
    const password = decodeURIComponent(parsed.password || "");
    // 移除认证信息，获取基础 URL
    parsed.username = "";
    parsed.password = "";
    return { baseUrl: parsed.toString(), username, password };
  } catch {
    return { baseUrl: url, username: "", password: "" };
  }
}

/** 将认证信息合并到 URL */
function mergeAuth(
  baseUrl: string,
  username: string,
  password: string,
): string {
  if (!baseUrl.trim()) return "";
  if (!username.trim()) return baseUrl;

  try {
    const parsed = new URL(baseUrl);
    // URL 对象的 username/password setter 会自动进行 percent-encoding
    // 不要使用 encodeURIComponent，否则会导致双重编码
    parsed.username = username.trim();
    if (password) {
      parsed.password = password;
    }
    return parsed.toString();
  } catch {
    // URL 解析失败，尝试手动插入（此时需要手动编码）
    const match = baseUrl.match(/^(\w+:\/\/)(.+)$/);
    if (match) {
      const auth = password
        ? `${encodeURIComponent(username.trim())}:${encodeURIComponent(password)}@`
        : `${encodeURIComponent(username.trim())}@`;
      return `${match[1]}${auth}${match[2]}`;
    }
    return baseUrl;
  }
}

export function GlobalProxySettings() {
  const { t } = useTranslation();
  const { data: savedUrl, isLoading } = useGlobalProxyUrl();
  const setMutation = useSetGlobalProxyUrl();
  // 读取后端保存的独立开关，切换时只刷新状态而不重填表单。
  const enabledQuery = useGlobalProxyEnabled();
  // 使用后端热更新入口保留地址及认证信息。
  const enabledMutation = useSetGlobalProxyEnabled();
  // 在设置页显示实际请求计数，刷新只读取本地状态。
  const upstreamQuery = useUpstreamProxyStatus();
  const stats = upstreamQuery.data?.stats;
  const total = stats
    ? stats.successful + stats.networkFailures + stats.httpErrors
    : 0;
  const successRate =
    total && stats
      ? `${((stats.successful / total) * 100).toFixed(1)}%`
      : t("settings.globalProxy.noRequests");
  const testMutation = useTestProxy();
  const scanMutation = useScanProxies();
  const desktopQuery = useCodexDesktopProxyStatus();
  const desktopMutation = useSetCodexDesktopProxyEnabled();

  const [url, setUrl] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [detected, setDetected] = useState<DetectedProxy[]>([]);

  // 计算完整 URL（含认证信息）
  const fullUrl = useMemo(
    () => mergeAuth(url, username, password),
    [url, username, password],
  );

  // 同步远程配置
  useEffect(() => {
    if (savedUrl !== undefined && !dirty && !setMutation.isPending) {
      // 只同步未编辑的表单，避免开关或窗口刷新覆盖尚未保存的内容。
      const { baseUrl, username: u, password: p } = extractAuth(savedUrl || "");
      setUrl(baseUrl);
      setUsername(u);
      setPassword(p);
      setDirty(false);
    }
  }, [savedUrl, dirty, setMutation.isPending]);

  const handleSave = async () => {
    // 保存失败时保留表单，错误提示由 mutation 统一显示。
    try {
      await setMutation.mutateAsync(fullUrl);
      setDirty(false);
    } catch {
      // 后端未确认新配置，保持可继续编辑和保存。
    }
  };

  const handleTest = async () => {
    if (fullUrl) {
      await testMutation.mutateAsync(fullUrl);
    }
  };

  const handleScan = async () => {
    const result = await scanMutation.mutateAsync();
    setDetected(result);
  };

  const handleSelect = (proxyUrl: string) => {
    const { baseUrl, username: u, password: p } = extractAuth(proxyUrl);
    setUrl(baseUrl);
    setUsername(u);
    setPassword(p);
    setDirty(true);
    setDetected([]);
  };

  const handleClear = () => {
    setUrl("");
    setUsername("");
    setPassword("");
    setDirty(true);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (
      e.key === "Enter" &&
      dirty &&
      !setMutation.isPending &&
      !enabledMutation.isPending &&
      !desktopMutation.isPending
    ) {
      // 与保存按钮使用同样的互斥条件，避免快捷键绕过切换状态。
      handleSave();
    }
  };

  const handleDesktopChange = (enabled: boolean) => {
    // 后端返回成功后再更新开关，避免系统代理失败却显示已经开启。
    desktopMutation.mutate(enabled);
  };

  /** 以后端结果更新开关，关闭时不清空表单。 */
  const handleEnabledChange = (enabled: boolean) => {
    // 使用已保存地址切换；失败后重新读取实际状态。
    enabledMutation.mutate(enabled);
  };

  // 帮助卡与无障碍状态共用文案，保留原有代理就绪判断。
  const desktopStatusText = desktopQuery.isLoading
    ? t("common.loading")
    : desktopQuery.isError
      ? t("settings.globalProxy.desktopReadFailed")
      : !desktopQuery.data?.supported
        ? t("settings.globalProxy.desktopUnsupported")
        : desktopQuery.data?.error
          ? desktopQuery.data.error
          : !desktopQuery.data?.enabled
            ? t("settings.globalProxy.desktopDisabled")
            : desktopQuery.data.running &&
                desktopQuery.data.systemProxyApplied &&
                desktopQuery.data.backendConfigApplied &&
                desktopQuery.data.websocketForwardingApplied
              ? t("settings.globalProxy.desktopReady")
              : t("settings.globalProxy.desktopInactive");

  // 只在首次加载且无数据时显示加载状态
  if (isLoading && savedUrl === undefined) {
    return (
      <div className="flex items-center justify-center p-4">
        <Loader2 className="h-5 w-5 animate-spin text-fg-2" />
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {/* 描述 */}
      <p className="text-sm text-fg-2">{t("settings.globalProxy.hint")}</p>

      <div className="flex min-h-11 items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-1">
          <label
            htmlFor="global-outbound-proxy"
            className="text-sm font-medium"
          >
            {t("settings.globalProxy.enableLabel")}
          </label>
          <HelpTip
            title={t("settings.globalProxy.enableLabel")}
            className="h-11 w-11"
          >
            <p className="text-xs text-fg-2">
              {t("settings.globalProxy.enableHint")}
            </p>
          </HelpTip>
        </div>
        <Switch
          id="global-outbound-proxy"
          checked={enabledQuery.data ?? false}
          disabled={
            enabledQuery.isLoading ||
            enabledQuery.isError ||
            enabledQuery.isFetching ||
            enabledMutation.isPending ||
            setMutation.isPending ||
            desktopMutation.isPending ||
            Boolean(desktopQuery.data?.enabled) ||
            (!enabledQuery.data && (dirty || !savedUrl))
          }
          onCheckedChange={handleEnabledChange}
        />
      </div>

      <div
        className="rounded-md border p-3 text-sm"
        role="status"
        aria-live="polite"
      >
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <span>
            {t("settings.globalProxy.successRate")}:{" "}
            {upstreamQuery.isError
              ? t("settings.globalProxy.statsReadFailed")
              : upstreamQuery.isLoading
                ? t("common.loading")
                : successRate}
          </span>
          <HelpTip
            title={t("settings.globalProxy.successRate")}
            className="h-11 w-11"
          >
            <p className="text-xs text-fg-2">
              {t("settings.globalProxy.statsHint")}
            </p>
          </HelpTip>
        </div>
        {stats && (
          <p className="text-xs text-fg-2">
            {t("settings.globalProxy.statsCounts", {
              total,
              successful: stats.successful,
              network: stats.networkFailures,
              http: stats.httpErrors,
            })}
          </p>
        )}
        {stats && stats.tunnelSuccessful + stats.tunnelFailures > 0 && (
          <p className="mt-1 text-xs text-fg-2">
            {t("settings.globalProxy.tunnelCounts", {
              successful: stats.tunnelSuccessful,
              failed: stats.tunnelFailures,
            })}
          </p>
        )}
      </div>

      {/* 代理地址输入框和按钮 */}
      <GlobalProxyLatencyChart
        key={`${savedUrl ?? ""}:${enabledQuery.data ?? false}`}
        proxyUrl={savedUrl ?? ""}
        enabled={
          Boolean(enabledQuery.data) &&
          !setMutation.isPending &&
          !enabledMutation.isPending
        }
      />

      <div className="flex gap-2">
        <Input
          placeholder="http://127.0.0.1:7890 / socks5://127.0.0.1:1080"
          value={url}
          disabled={setMutation.isPending || enabledMutation.isPending}
          onChange={(e) => {
            setUrl(e.target.value);
            setDirty(true);
          }}
          onKeyDown={handleKeyDown}
          className="font-mono text-sm flex-1"
        />
        <HoverTip content={t("settings.globalProxy.scan")}>
          <Button
            variant="outline"
            size="icon"
            disabled={
              scanMutation.isPending ||
              setMutation.isPending ||
              enabledMutation.isPending
            }
            onClick={handleScan}
            aria-label={t("settings.globalProxy.scan")}
          >
            {scanMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Search className="h-4 w-4" />
            )}
          </Button>
        </HoverTip>
        <HoverTip content={t("settings.globalProxy.test")}>
          <Button
            variant="outline"
            size="icon"
            disabled={!fullUrl || testMutation.isPending}
            onClick={handleTest}
            aria-label={t("settings.globalProxy.test")}
          >
            {testMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <TestTube2 className="h-4 w-4" />
            )}
          </Button>
        </HoverTip>
        <HoverTip content={t("settings.globalProxy.clear")}>
          <Button
            variant="outline"
            size="icon"
            disabled={
              (!url && !username && !password) ||
              setMutation.isPending ||
              enabledMutation.isPending
            }
            onClick={handleClear}
            aria-label={t("settings.globalProxy.clear")}
          >
            <X className="h-4 w-4" />
          </Button>
        </HoverTip>
        <Button
          onClick={handleSave}
          disabled={
            !dirty ||
            setMutation.isPending ||
            desktopMutation.isPending ||
            enabledMutation.isPending
          }
          size="sm"
        >
          {setMutation.isPending && (
            <Loader2 className="h-4 w-4 animate-spin" />
          )}
          {t("common.save")}
        </Button>
      </div>

      {/* 认证信息：用户名 + 密码（可选） */}
      <div className="flex gap-2">
        <Input
          placeholder={t("settings.globalProxy.username")}
          value={username}
          disabled={setMutation.isPending || enabledMutation.isPending}
          onChange={(e) => {
            setUsername(e.target.value);
            setDirty(true);
          }}
          onKeyDown={handleKeyDown}
          className="font-mono text-sm flex-1"
        />
        <div className="relative flex-1">
          <Input
            type={showPassword ? "text" : "password"}
            placeholder={t("settings.globalProxy.password")}
            value={password}
            disabled={setMutation.isPending || enabledMutation.isPending}
            onChange={(e) => {
              setPassword(e.target.value);
              setDirty(true);
            }}
            onKeyDown={handleKeyDown}
            className="font-mono text-sm pr-10"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
            onClick={() => setShowPassword(!showPassword)}
            tabIndex={-1}
          >
            {showPassword ? (
              <EyeOff className="h-4 w-4 text-fg-2" />
            ) : (
              <Eye className="h-4 w-4 text-fg-2" />
            )}
          </Button>
        </div>
      </div>

      {/* 复用现有出站配置；不增加 Codex 启动按钮。 */}
      <div className="rounded-md border p-3">
        <div className="flex min-h-11 items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-1">
            <label
              htmlFor="codex-desktop-proxy"
              className="text-sm font-medium"
            >
              {t("settings.globalProxy.desktopLabel")}
            </label>
            <HelpTip
              title={t("settings.globalProxy.desktopLabel")}
              className="h-11 w-11"
            >
              <p className="text-xs text-fg-2">
                {t("settings.globalProxy.desktopHint")}
              </p>
              <p className="mt-2 break-words text-xs text-fg-2">
                {desktopStatusText}
              </p>
            </HelpTip>
          </div>
          <Switch
            id="codex-desktop-proxy"
            checked={desktopQuery.data?.enabled ?? false}
            disabled={
              desktopQuery.isLoading ||
              (!desktopQuery.data?.supported && !desktopQuery.data?.enabled) ||
              desktopMutation.isPending ||
              setMutation.isPending ||
              enabledMutation.isPending ||
              enabledQuery.isFetching ||
              (!desktopQuery.data?.enabled &&
                (dirty || !savedUrl || !enabledQuery.data))
            }
            onCheckedChange={handleDesktopChange}
          />
        </div>
        <p className="sr-only" role="status" aria-live="polite">
          {desktopStatusText}
        </p>
      </div>

      {/* 扫描结果 */}
      {detected.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {detected.map((p) => (
            <Button
              key={p.url}
              variant="secondary"
              size="sm"
              onClick={() => handleSelect(p.url)}
              disabled={setMutation.isPending || enabledMutation.isPending}
              className="font-mono text-xs"
            >
              {p.url}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}
