import { describe, expect, it } from "vitest";
import worker from "../src/index.js";

const latestManifest = JSON.stringify({
  version: "0.1.18",
  platforms: {
    "darwin-aarch64": {
      signature: "mac-sig",
      url: "https://codexapp.agentsmirror.com/manager/0.1.18/CodexAppManager_aarch64.app.tar.gz",
    },
    "windows-x86_64": {
      signature: "win-sig",
      url: "https://codexapp.agentsmirror.com/manager/0.1.18/CodexAppManager_0.1.18_x64-setup.exe",
    },
    "windows-aarch64": {
      signature: "win-arm64-sig",
      url: "https://codexapp.agentsmirror.com/manager/0.1.18/CodexAppManager_0.1.18_arm64-setup.exe",
    },
  },
});

function r2Object(body, { contentType = "application/octet-stream", invalidJson = false } = {}) {
  return {
    body,
    httpEtag: '"test-etag"',
    writeHttpMetadata(headers) {
      headers.set("Content-Type", contentType);
    },
    async json() {
      if (invalidJson) throw new Error("invalid json");
      return JSON.parse(body);
    },
  };
}

function bucket(entries, calls = []) {
  return {
    async get(key) {
      calls.push(key);
      return entries[key] || null;
    },
  };
}

function request(path, init = {}, cf) {
  const req = new Request(`https://codexapp.agentsmirror.com${path}`, init);
  if (cf) {
    Object.defineProperty(req, "cf", { value: cf });
  }
  return req;
}

function secondaryEnv(extra = {}) {
  return {
    SECONDARY_S3_ENDPOINT: "https://s3.example.test/root",
    SECONDARY_S3_BUCKET: "mirror-bucket",
    SECONDARY_S3_REGION: "auto",
    SECONDARY_S3_PREFIX: "manager",
    SECONDARY_S3_ACCESS_KEY_ID: "AKIATEST",
    SECONDARY_S3_SECRET_ACCESS_KEY: "secret",
    ...extra,
  };
}

describe("manager download router", () => {
  it("rewrites latest Windows links to the current versioned installer key", async () => {
    const calls = [];
    const env = {
      BUCKET: bucket(
        {
          "latest.json": r2Object(latestManifest, { contentType: "application/json" }),
          "0.1.18/CodexAppManager_0.1.18_x64-setup.exe": r2Object("installer"),
        },
        calls,
      ),
    };

    const res = await worker.fetch(request("/manager/latest/CodexAppManager_x64-setup.exe"), env);

    expect(res.status).toBe(200);
    expect(await res.text()).toBe("installer");
    expect(calls).toEqual(["latest.json", "0.1.18/CodexAppManager_0.1.18_x64-setup.exe"]);
  });

  it("rewrites latest Windows ARM64 links to the current versioned installer key", async () => {
    const calls = [];
    const env = {
      BUCKET: bucket(
        {
          "latest.json": r2Object(latestManifest, { contentType: "application/json" }),
          "0.1.18/CodexAppManager_0.1.18_arm64-setup.exe": r2Object("arm64 installer"),
        },
        calls,
      ),
    };

    const res = await worker.fetch(request("/manager/latest/CodexAppManager_arm64-setup.exe"), env);

    expect(res.status).toBe(200);
    expect(await res.text()).toBe("arm64 installer");
    expect(calls).toEqual(["latest.json", "0.1.18/CodexAppManager_0.1.18_arm64-setup.exe"]);
  });

  it("rewrites latest macOS links without injecting a version into the filename", async () => {
    const calls = [];
    const env = {
      BUCKET: bucket(
        {
          "latest.json": r2Object(latestManifest, { contentType: "application/json" }),
          "0.1.18/CodexAppManager_aarch64.dmg": r2Object("dmg"),
        },
        calls,
      ),
    };

    const res = await worker.fetch(request("/manager/latest/CodexAppManager_aarch64.dmg"), env);

    expect(res.status).toBe(200);
    expect(calls).toEqual(["latest.json", "0.1.18/CodexAppManager_aarch64.dmg"]);
  });

  it("returns 404 for latest links when latest.json is missing or invalid", async () => {
    const missing = await worker.fetch(request("/manager/latest/CodexAppManager_x64-setup.exe"), {
      BUCKET: bucket({}),
    });
    expect(missing.status).toBe(404);

    const invalid = await worker.fetch(request("/manager/latest/CodexAppManager_x64-setup.exe"), {
      BUCKET: bucket({ "latest.json": r2Object("not json", { invalidJson: true }) }),
    });
    expect(invalid.status).toBe(404);
  });

  it("serves non-CN requests from R2 with JSON and installer cache controls", async () => {
    const jsonEnv = {
      BUCKET: bucket({
        "0.1.18/latest.json": r2Object("{}", { contentType: "application/json" }),
      }),
    };
    const jsonRes = await worker.fetch(request("/manager/0.1.18/latest.json"), jsonEnv);
    expect(jsonRes.status).toBe(200);
    expect(jsonRes.headers.get("Cache-Control")).toBe("public, max-age=120, s-maxage=120");

    const installerEnv = {
      BUCKET: bucket({
        "0.1.18/CodexAppManager_0.1.18_x64-setup.exe": r2Object("installer"),
      }),
    };
    const installerRes = await worker.fetch(
      request("/manager/0.1.18/CodexAppManager_0.1.18_x64-setup.exe"),
      installerEnv,
    );
    expect(installerRes.status).toBe(200);
    expect(installerRes.headers.get("Cache-Control")).toBe("public, max-age=86400, s-maxage=86400");
  });

  it("redirects CN requests to a presigned IHEP URL when secondary S3 is configured", async () => {
    const env = {
      BUCKET: bucket({}),
      ...secondaryEnv(),
    };

    const res = await worker.fetch(
      request("/manager/0.1.18/CodexAppManager_0.1.18_x64-setup.exe", {}, { country: "CN" }),
      env,
    );

    expect(res.status).toBe(302);
    const location = new URL(res.headers.get("Location"));
    expect(location.origin).toBe("https://s3.example.test");
    expect(location.pathname).toBe("/root/mirror-bucket/manager/0.1.18/CodexAppManager_0.1.18_x64-setup.exe");
    expect(location.searchParams.get("X-Amz-Algorithm")).toBe("AWS4-HMAC-SHA256");
    expect(location.searchParams.has("X-Amz-Signature")).toBe(true);
  });

  it("falls back to R2 for CN requests when secondary S3 config is incomplete", async () => {
    const calls = [];
    const env = {
      BUCKET: bucket(
        {
          "0.1.18/CodexAppManager_0.1.18_x64-setup.exe": r2Object("installer"),
        },
        calls,
      ),
      ...secondaryEnv({ SECONDARY_S3_SECRET_ACCESS_KEY: "" }),
    };

    const res = await worker.fetch(
      request("/manager/0.1.18/CodexAppManager_0.1.18_x64-setup.exe", {}, { country: "CN" }),
      env,
    );

    expect(res.status).toBe(200);
    expect(await res.text()).toBe("installer");
    expect(calls).toEqual(["0.1.18/CodexAppManager_0.1.18_x64-setup.exe"]);
  });

  it("rejects empty keys, directory keys, and traversal-looking keys", async () => {
    const calls = [];
    const env = { BUCKET: bucket({}, calls) };

    for (const path of ["/manager/", "/manager/0.1.18/", "/manager/%2e%2e/secrets", "/manager/foo..bar"]) {
      const res = await worker.fetch(request(path), env);
      expect(res.status).toBe(404);
    }
    expect(calls).toEqual([]);
  });
});
