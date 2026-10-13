/**
 * 全局出站代理 React Hooks
 *
 * 提供获取、设置和测试全局代理的 React Query hooks。
 */

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "@/lib/toast";
import { useTranslation } from "react-i18next";
import {
  getGlobalProxyUrl,
  setGlobalProxyUrl,
  getGlobalProxyEnabled,
  setGlobalProxyEnabled,
  testProxyUrl,
  getUpstreamProxyStatus,
  scanLocalProxies,
  getCodexDesktopProxyStatus,
  setCodexDesktopProxyEnabled,
  type ProxyTestResult,
  type UpstreamProxyStatus,
  type DetectedProxy,
} from "@/lib/api/globalProxy";

/**
 * 获取全局代理 URL
 */
export function useGlobalProxyUrl() {
  return useQuery({
    queryKey: ["globalProxyUrl"],
    queryFn: getGlobalProxyUrl,
    staleTime: 30 * 1000, // 30秒内不重新获取，避免展开时闪烁
  });
}

/** 获取全局出站开关，保存地址后重新读取兼容状态。 */
export function useGlobalProxyEnabled() {
  return useQuery({
    queryKey: ["globalProxyEnabled"],
    queryFn: getGlobalProxyEnabled,
  });
}

/** 切换后重新读取持久化和运行态；失败时保留原开关。 */
export function useSetGlobalProxyEnabled() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    mutationFn: setGlobalProxyEnabled,
    onSuccess: () => {
      toast.success(t("settings.globalProxy.saved"));
    },
    onError: (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      toast.error(t("settings.globalProxy.saveFailed", { error: message }));
    },
    onSettled: () => {
      // 切换失败也重新读取，避免前端显示未经后端确认的状态。
      queryClient.invalidateQueries({ queryKey: ["globalProxyEnabled"] });
      // 共享客户端更新后刷新实际出站状态。
      queryClient.invalidateQueries({ queryKey: ["upstreamProxyStatus"] });
      // 桌面转发依赖出站开关，同步其就绪状态。
      queryClient.invalidateQueries({ queryKey: ["codexDesktopProxyStatus"] });
    },
  });
}

/**
 * 设置全局代理 URL
 */
export function useSetGlobalProxyUrl() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: setGlobalProxyUrl,
    onSuccess: () => {
      toast.success(t("settings.globalProxy.saved"));
      queryClient.invalidateQueries({ queryKey: ["globalProxyUrl"] });
      // 首次保存或清空地址可能改变启用状态。
      queryClient.invalidateQueries({ queryKey: ["globalProxyEnabled"] });
      queryClient.invalidateQueries({ queryKey: ["upstreamProxyStatus"] });
      queryClient.invalidateQueries({ queryKey: ["codexDesktopProxyStatus"] });
    },
    onError: (error: unknown) => {
      const message =
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : "Unknown error";
      toast.error(t("settings.globalProxy.saveFailed", { error: message }));
    },
  });
}

/**
 * 测试代理连接
 */
export function useTestProxy() {
  const { t } = useTranslation();

  return useMutation({
    mutationFn: testProxyUrl,
    onSuccess: (result: ProxyTestResult) => {
      if (result.success) {
        toast.success(
          t("settings.globalProxy.testSuccess", { latency: result.latencyMs }),
        );
      } else {
        toast.error(
          t("settings.globalProxy.testFailed", { error: result.error }),
        );
      }
    },
    onError: (error: Error) => {
      toast.error(error.message);
    },
  });
}

/**
 * 获取当前出站代理状态
 */
export function useUpstreamProxyStatus() {
  return useQuery<UpstreamProxyStatus>({
    queryKey: ["upstreamProxyStatus"],
    queryFn: getUpstreamProxyStatus,
    // 仅刷新本地计数，不发送出站探测请求；设置页卸载后停止。
    refetchInterval: 5000,
  });
}

/**
 * 扫描本地代理
 */
export function useScanProxies() {
  const { t } = useTranslation();

  return useMutation({
    mutationFn: scanLocalProxies,
    onError: (error: Error) => {
      toast.error(
        t("settings.globalProxy.scanFailed", { error: error.message }),
      );
    },
  });
}

export type { DetectedProxy };

/** 查询本地转发状态，设置页显示期间检测外部系统代理变更。 */
export function useCodexDesktopProxyStatus() {
  return useQuery({
    queryKey: ["codexDesktopProxyStatus"],
    queryFn: getCodexDesktopProxyStatus,
    refetchInterval: 5000,
  });
}

/** 保存开关后重新读取状态，失败时保留原开关并显示后端原因。 */
export function useSetCodexDesktopProxyEnabled() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: setCodexDesktopProxyEnabled,
    onSuccess: () => {
      toast.success(t("settings.globalProxy.desktopSaved"));
    },
    onError: (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      toast.error(t("settings.globalProxy.saveFailed", { error: message }));
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["codexDesktopProxyStatus"] });
    },
  });
}
