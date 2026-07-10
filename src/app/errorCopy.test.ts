import { describe, expect, it } from "vitest";

import {
  isConnectivityFailure,
  messageFailure,
  resolveFailure,
  userErrorMessage,
} from "./errorCopy";
import type { TKey } from "./i18n";

const copy: Partial<Record<TKey, string>> = {
  "error.install": "安装未完成。已保留现有 Codex，请关闭 Codex 后重试。",
  "error.busy": "已有操作正在进行，请稍后再试。",
  "error.network": "无法连接更新服务器。请检查网络后重试。",
  "error.timeout": "连接超时。请检查网络后重试。",
  "error.disk_space": "磁盘空间不足，请清理后重试。",
  "error.disk_write": "无法写入磁盘。请检查磁盘权限与可用空间后重试。",
  "error.permission": "权限不足。请以合适的权限重试，或更换安装位置。",
  "error.signature": "更新包签名校验失败。已保留现有 Codex，请稍后重试或更换更新源。",
  "error.artifact": "更新包无效或已过期。请稍后重试，或在设置中更换更新源。",
  "error.incompatible": "当前系统环境不支持此安装方式。请尝试其他安装选项。",
  "error.unsupported": "当前平台暂不支持此操作。",
  "error.generic": "操作未完成。请稍后重试；若持续失败，可复制诊断信息反馈。",
  "home.stale.rechecked": "安装状态已变化,已重新检查——请再次确认。",
  "progress.cancelled": "下载已取消。",
};

const t = (key: TKey) => copy[key] ?? key;

describe("userErrorMessage / resolveFailure", () => {
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

  it("never leaks raw engine detail as the primary message", () => {
    const failure = resolveFailure({ code: "engine_error", message: "raw detail" }, t);
    expect(failure.message).toBe(
      "操作未完成。请稍后重试；若持续失败，可复制诊断信息反馈。",
    );
    expect(failure.detail).toBe("raw detail");
    expect(failure.code).toBe("engine_error");
    expect(failure.recoverable).toBe(true);
  });

  it("maps every classified backend kind to localized copy", () => {
    const cases: Array<[string, string]> = [
      ["network", "无法连接更新服务器。请检查网络后重试。"],
      ["timeout", "连接超时。请检查网络后重试。"],
      ["disk_space", "磁盘空间不足，请清理后重试。"],
      ["disk_write", "无法写入磁盘。请检查磁盘权限与可用空间后重试。"],
      ["permission", "权限不足。请以合适的权限重试，或更换安装位置。"],
      ["signature", "更新包签名校验失败。已保留现有 Codex，请稍后重试或更换更新源。"],
      ["artifact", "更新包无效或已过期。请稍后重试，或在设置中更换更新源。"],
      ["incompatible", "当前系统环境不支持此安装方式。请尝试其他安装选项。"],
      ["cancelled", "下载已取消。"],
      ["operation_busy", "已有操作正在进行，请稍后再试。"],
      ["stale_expectation", "安装状态已变化,已重新检查——请再次确认。"],
      ["unsupported_platform", "当前平台暂不支持此操作。"],
      ["internal_error", "操作未完成。请稍后重试；若持续失败，可复制诊断信息反馈。"],
      ["contract_mismatch", "操作未完成。请稍后重试；若持续失败，可复制诊断信息反馈。"],
    ];
    for (const [code, message] of cases) {
      expect(userErrorMessage({ code, message: `opaque ${code}` }, t)).toBe(message);
    }
  });

  it("treats unknown throwables as a generic recoverable failure", () => {
    const failure = resolveFailure(new Error("boom from render"), t);
    expect(failure.code).toBe("unknown");
    expect(failure.message).toBe(
      "操作未完成。请稍后重试；若持续失败，可复制诊断信息反馈。",
    );
    expect(failure.detail).toBe("boom from render");
    expect(failure.recoverable).toBe(true);
  });

  it("classifies connectivity codes for hero presentation", () => {
    expect(isConnectivityFailure({ code: "network", message: "x" })).toBe(true);
    expect(isConnectivityFailure({ code: "timeout", message: "x" })).toBe(true);
    expect(isConnectivityFailure({ code: "disk_write", message: "x" })).toBe(false);
  });

  it("builds a message-only surface without raw detail", () => {
    expect(messageFailure("already localized", "cancelled")).toEqual({
      code: "cancelled",
      message: "already localized",
      detail: null,
      recoverable: true,
    });
  });
});
