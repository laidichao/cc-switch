import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  LineChart,
  Line,
  CartesianGrid,
  ResponsiveContainer,
  XAxis,
  YAxis,
  Tooltip,
} from "recharts";
import { useSettingsQuery } from "@/lib/query";
import { normalizeRefreshInterval } from "@/lib/usageRefreshInterval";
import { measureGlobalProxyLatency } from "@/lib/api/globalProxy";
import { HelpTip } from "@/components/ui/help-tip";
import { Button } from "@/components/ui/button";

interface LatencyPoint {
  time: number;
  latencyMs: number | null;
  error: string | null;
}

/** 设置页可见期间测量当前配置，最近 60 次结果只保留在页面内存。 */
export function GlobalProxyLatencyChart({
  proxyUrl,
  enabled,
}: {
  proxyUrl: string;
  enabled: boolean;
}) {
  const { t } = useTranslation();
  // 复用软件已保存的刷新设置，不另设代理刷新间隔。
  const settingsQuery = useSettingsQuery();
  // 与用量页共用合法间隔和默认值，关闭刷新时只允许手动检测。
  const interval = normalizeRefreshInterval(
    settingsQuery.data?.usageDashboardRefreshIntervalMs,
  );
  const [history, setHistory] = useState<LatencyPoint[]>([]);
  const latest = history[history.length - 1];
  // query 自动避免同一检测重叠；读取 signal 后，卸载或配置切换会取消结果消费。
  const probe = useQuery({
    queryKey: ["globalProxyLatency", proxyUrl, enabled],
    enabled:
      enabled && Boolean(proxyUrl) && settingsQuery.isSuccess && interval > 0,
    queryFn: async ({ signal }) => {
      try {
        // 只检测已保存代理，后台命令不影响业务请求成功率。
        const result = await measureGlobalProxyLatency(proxyUrl);
        if (!signal.aborted) {
          // 失败点使用 null 让折线断开，固定容量避免长期开页增长。
          setHistory((previous) => [
            ...previous.slice(-59),
            {
              time: Date.now(),
              latencyMs: result.success ? result.latencyMs : null,
              error: result.error,
            },
          ]);
        }
        return result;
      } catch (error) {
        if (!signal.aborted) {
          // IPC 或配置读取失败同样记录失败点，保留统一的可见错误状态。
          setHistory((previous) => [
            ...previous.slice(-59),
            {
              time: Date.now(),
              latencyMs: null,
              error: t("settings.globalProxy.latencyReadFailed"),
            },
          ]);
        }
        throw error;
      }
    },
    refetchInterval: enabled && interval > 0 ? interval : false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
    gcTime: 0,
  });

  /** 手动检测沿用同一队列，不启动额外计时器。 */
  const measureNow = () => {
    // 不取消尚未结束的检测，避免两次探测并发。
    void probe.refetch({ cancelRefetch: false });
  };

  const status = !enabled
    ? t("settings.globalProxy.latencyDisabled")
    : latest
      ? latest.latencyMs === null
        ? t("settings.globalProxy.latencyFailed", { error: latest.error })
        : `${latest.latencyMs} ms`
      : t("settings.globalProxy.latencyEmpty");

  return (
    <div className="min-w-0 rounded-md border p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1">
          <span className="text-sm font-medium">
            {t("settings.globalProxy.latencyLabel")}
          </span>
          <HelpTip
            title={t("settings.globalProxy.latencyLabel")}
            className="h-11 w-11"
          >
            <p className="text-xs text-fg-2">
              {t("settings.globalProxy.latencyHint")}
            </p>
          </HelpTip>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="min-h-11"
          onClick={measureNow}
          disabled={!enabled || !proxyUrl || probe.isFetching}
        >
          {probe.isFetching
            ? t("settings.globalProxy.latencyMeasuring")
            : t("settings.globalProxy.latencyMeasure")}
        </Button>
      </div>
      <p className="break-words text-sm" role="status" aria-live="polite">
        {status}
      </p>
      <p className="mt-1 text-xs text-fg-2">
        {interval > 0
          ? t("settings.globalProxy.latencyInterval", {
              seconds: interval / 1000,
            })
          : t("settings.globalProxy.latencyPaused")}
      </p>
      {history.length > 0 && (
        <div
          className="mt-3 h-40 min-w-0"
          role="img"
          aria-label={t("settings.globalProxy.latencyChartLabel")}
        >
          <ResponsiveContainer width="100%" height="100%">
            <LineChart
              data={history}
              margin={{ top: 8, right: 12, bottom: 4, left: 0 }}
              accessibilityLayer
            >
              <CartesianGrid
                stroke="hsl(var(--border))"
                strokeDasharray="3 3"
                vertical={false}
              />
              <XAxis
                dataKey="time"
                tickFormatter={(value: number) =>
                  new Date(value).toLocaleTimeString()
                }
                tick={{ fill: "var(--text-3)", fontSize: 10 }}
                minTickGap={30}
              />
              <YAxis
                width={52}
                unit=" ms"
                domain={[0, "auto"]}
                tick={{ fill: "var(--text-3)", fontSize: 10 }}
              />
              <Tooltip
                filterNull={false}
                content={({ active, label }) => {
                  // 按时间定位已保存点，失败提示与主状态使用同一结果。
                  const point = history.find(
                    (item) => item.time === Number(label),
                  );
                  if (!active || !point) return null;
                  return (
                    <div className="max-w-64 rounded-md border bg-card p-2 text-xs text-fg-2 shadow-sm">
                      <p>{new Date(point.time).toLocaleTimeString()}</p>
                      <p className="mt-1 break-words">
                        {point.latencyMs === null
                          ? t("settings.globalProxy.latencyFailed", {
                              error: point.error,
                            })
                          : `${point.latencyMs} ms`}
                      </p>
                    </div>
                  );
                }}
              />
              <Line
                type="linear"
                dataKey="latencyMs"
                stroke="var(--chart-1)"
                strokeWidth={2}
                dot={{ r: 3 }}
                connectNulls={false}
                isAnimationActive={false}
              />
            </LineChart>
          </ResponsiveContainer>
        </div>
      )}
    </div>
  );
}
