/** 用量统计与代理延迟共用的自动刷新间隔。 */
export const REFRESH_INTERVAL_OPTIONS_MS = [
  0, 5000, 10000, 30000, 60000,
] as const;

/** 未配置或无效配置沿用软件默认的 30 秒刷新。 */
export function normalizeRefreshInterval(value: number | undefined): number {
  // 仅采用设置菜单支持的间隔，0 表示关闭自动刷新。
  return REFRESH_INTERVAL_OPTIONS_MS.some((option) => option === value)
    ? (value ?? 30000)
    : 30000;
}
