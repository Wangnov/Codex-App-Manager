import { describe, expect, it } from "vitest";

import { userErrorMessage } from "./errorCopy";
import type { TKey } from "./i18n";

const copy: Partial<Record<TKey, string>> = {
  "error.install": "安装未完成。已保留现有 Codex，请关闭 Codex 后重试。",
  "error.busy": "已有操作正在进行，请稍后再试。",
  "home.error.network.sub": "请检查网络连接后重试。",
  "home.stale.rechecked": "安装状态已变化,已重新检查——请再次确认。",
  "progress.cancelled": "下载已取消。",
};

const t = (key: TKey) => copy[key] ?? key;

describe("userErrorMessage", () => {
  it("uses the stable backend code instead of leaking raw install errors", () => {
    expect(
      userErrorMessage(
        {
          code: "install",
          message:
            "update engine error: install error: target Codex process is still running",
        },
        t,
      ),
    ).toBe("安装未完成。已保留现有 Codex，请关闭 Codex 后重试。");
  });

  it("falls back to the raw message for unknown errors", () => {
    expect(userErrorMessage({ code: "engine_error", message: "raw detail" }, t)).toBe(
      "raw detail",
    );
  });
});
