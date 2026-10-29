import { describe, expect, it, vi } from "vitest";

import {
  codeErrorMessage,
  isConnectivityFailure,
  messageFailure,
  resolveFailure,
  userErrorMessage,
} from "./errorCopy";
import type { TKey } from "./i18n";

const copy: Partial<Record<TKey, string>> = {
  "error.install": "安装未完成。已保留现有 Codex，请关闭 Codex 后重试。",
  "error.busy": "已有操作正在进行，请稍后再试。",
  "error.rate_limit": "GitHub API 请求额度已用完，请等待额度恢复后重试。",
  "error.network": "无法连接更新服务器。请检查网络后重试。",
  "error.timeout": "连接超时。请检查网络后重试。",
  "error.disk_space": "磁盘空间不足，请清理后重试。",
  "error.disk_write": "无法写入磁盘。请检查磁盘权限与可用空间后重试。",
  "error.permission": "权限不足。请以合适的权限重试，或更换安装位置。",
  "error.install_directory_unavailable": "安装目录暂时无法替换。请完全退出 Codex（含托盘），确认目录可写后重试。具体路径见详情。",
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

  it("shows portable-directory recovery while keeping both paths in the details", () => {
    const detail = 'portable install directory unavailable: source="C:\\Codex"; destination="C:\\Codex.rollback"; os error 5';
    const failure = resolveFailure({ code: "install_directory_unavailable", message: detail }, t);
    expect(failure.message).toBe(copy["error.install_directory_unavailable"]);
    expect(failure.message).not.toContain("C:\\Codex");
    expect(failure.detail).toBe(detail);
    expect(failure.recoverable).toBe(true);
  });

  it("maps every classified backend kind to localized copy", () => {
    const cases: Array<[string, string]> = [
      ["rate_limit", "GitHub API 请求额度已用完，请等待额度恢复后重试。"],
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

  it("formats the backend reset timestamp separately from raw diagnostics", () => {
    const translate = vi.fn().mockReturnValue("等待重置后重试");
    const failure = resolveFailure({ code: "rate_limit", message: "raw diagnostic", retryAfterUnix: 1800000000 }, translate);
    expect(translate).toHaveBeenLastCalledWith("error.rate_limit_reset", { time: expect.any(String) });
    expect(failure.recoverable).toBe(true);
    expect(failure.detail).toBe("raw diagnostic");
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

describe("codeErrorMessage", () => {
  it("localizes a bare stable code from a backend snapshot", () => {
    expect(codeErrorMessage("network", t)).toBe(copy["error.network"]);
    expect(codeErrorMessage("signature", t)).toBe(copy["error.signature"]);
    expect(codeErrorMessage("operation_busy", t)).toBe(copy["error.busy"]);
  });

  it("falls back to the generic copy for a missing or unknown code", () => {
    expect(codeErrorMessage(null, t)).toBe(copy["error.generic"]);
    expect(codeErrorMessage("something_new", t)).toBe(copy["error.generic"]);
  });
});
