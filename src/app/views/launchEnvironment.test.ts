import { describe, expect, it } from "vitest";

import sysRs from "../../../crates/codex-win-engine/src/sys.rs?raw";
import { CATALOG, type TFn, type TKey } from "../i18n";
import {
  BLOCKED_LAUNCH_ENV_NAMES,
  MANAGER_MANAGED_ENV_NAMES,
  MAX_LAUNCH_ENVIRONMENT_BYTES,
  launchEnvironmentError,
} from "./launchEnvironment";

const zhT: TFn = (key: TKey, vars?: Record<string, string | number>) => {
  let copy = CATALOG["zh-CN"][key];
  if (vars) {
    for (const [name, value] of Object.entries(vars)) {
      copy = copy.replaceAll(`{${name}}`, String(value));
    }
  }
  return copy;
};

function parseRustStringSlice(source: string, constName: string): string[] {
  const pattern = new RegExp(`pub const ${constName}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`);
  const match = pattern.exec(source);
  if (!match) {
    throw new Error(`Could not locate ${constName} in crates/codex-win-engine/src/sys.rs`);
  }
  return Array.from(match[1].matchAll(/"([A-Z0-9_]+)"/g), (item) => item[1]);
}

describe("launchEnvironmentError", () => {
  it("allows empty input and standard proxy/certificate variables while preserving original case", () => {
    expect(launchEnvironmentError("", zhT)).toBeNull();
    expect(launchEnvironmentError("  \r\n  \n", zhT)).toBeNull();
    expect(
      launchEnvironmentError(
        [
          "http_proxy=http://127.0.0.1:7890",
          "HTTPS_PROXY=http://127.0.0.1:7890",
          "ALL_PROXY=socks5://127.0.0.1:7890",
          "NO_PROXY=localhost,127.0.0.1,::1",
          "NODE_EXTRA_CA_CERTS=C:\\certs\\corp ca.pem",
          "SSL_CERT_FILE=C:\\certs\\ca.pem",
          "SSL_CERT_DIR=C:\\certs",
          "CURL_CA_BUNDLE=C:\\certs\\ca.pem",
          "REQUESTS_CA_BUNDLE=C:\\certs\\ca.pem",
          "Example=a=b 中文",
        ].join("\r\n"),
        zhT,
      ),
    ).toBeNull();
  });

  it("rejects invalid syntax, leading/trailing spaces on keys, control chars, and empty values", () => {
    for (const invalid of [
      "INVALID",
      "=secret",
      " HTTP_PROXY=secret",
      "HTTP_PROXY =secret",
      "1NAME=secret",
      "HTTP-PROXY=secret",
      "\uFEFFNAME=secret",
      "NAME=secret\0",
      "NAME\0X=secret",
      "NAME=secret\rvalue",
      "NAME=secret\tvalue",
      "NAME=secret\x1b[31m",
      "NAME=secret\u0085next",
      "NAME=secret\u2028next",
      "NAME=secret\u2029next",
    ]) {
      const error = launchEnvironmentError(`HTTP_PROXY=one\n${invalid}`, zhT);
      expect(error, invalid).toBe("第 2 行请使用「名称=值」格式。");
      expect(error).not.toContain("secret");
    }

    for (const empty of ["HTTP_PROXY=", "HTTP_PROXY=   "]) {
      const error = launchEnvironmentError(`HTTPS_PROXY=one\n${empty}`, zhT);
      expect(error, empty).toBe("第 2 行的值不能为空。");
    }
  });

  it("rejects case-insensitive duplicate names without leaking values", () => {
    const error = launchEnvironmentError(
      "HTTP_PROXY=http://127.0.0.1:7890\nhttp_proxy=secret-value",
      zhT,
    );
    expect(error).toBe("第 2 行的变量名重复（不区分大小写）。");
    expect(error).not.toContain("secret-value");
  });

  it("rejects all Manager-managed and blocked system/injection variables case-insensitively", () => {
    for (const managed of MANAGER_MANAGED_ENV_NAMES) {
      for (const candidate of [managed, managed.toLowerCase()]) {
        const error = launchEnvironmentError(`HTTP_PROXY=one\n${candidate}=secret`, zhT);
        expect(error, candidate).toBe("第 2 行的变量由管理器管理。");
        expect(error).not.toContain("secret");
      }
    }

    for (const blocked of BLOCKED_LAUNCH_ENV_NAMES) {
      const mixed = blocked[0].toUpperCase() + blocked.slice(1).toLowerCase();
      for (const candidate of [blocked, blocked.toLowerCase(), mixed]) {
        const error = launchEnvironmentError(`HTTP_PROXY=one\n${candidate}=secret`, zhT);
        expect(error, candidate).toBe("第 2 行的系统或安全受限变量不允许覆盖。");
        expect(error).not.toContain("secret");
      }
    }
  });

  it("enforces the 16 KiB UTF-8 byte limit", () => {
    const prefix = "A=";
    const exact = prefix + "x".repeat(MAX_LAUNCH_ENVIRONMENT_BYTES - prefix.length);
    expect(launchEnvironmentError(exact, zhT)).toBeNull();
    expect(launchEnvironmentError(`${exact}y`, zhT)).toBe(
      `启动环境变量总长度不能超过 ${MAX_LAUNCH_ENVIRONMENT_BYTES} 字节。`,
    );

    const multiByteTooLong = prefix + "中".repeat(Math.floor(MAX_LAUNCH_ENVIRONMENT_BYTES / 3) + 1);
    expect(launchEnvironmentError(multiByteTooLong, zhT)).toBe(
      `启动环境变量总长度不能超过 ${MAX_LAUNCH_ENVIRONMENT_BYTES} 字节。`,
    );
  });

  it("stays in exact parity with the Rust engine blocklists and size limit", () => {
    expect([...MANAGER_MANAGED_ENV_NAMES]).toEqual(
      parseRustStringSlice(sysRs, "MANAGER_MANAGED_ENV_NAMES"),
    );
    expect([...BLOCKED_LAUNCH_ENV_NAMES]).toEqual(
      parseRustStringSlice(sysRs, "BLOCKED_LAUNCH_ENV_NAMES"),
    );
    expect(sysRs).toContain("pub const MAX_LAUNCH_ENVIRONMENT_BYTES: usize = 16 * 1024;");
  });
});
