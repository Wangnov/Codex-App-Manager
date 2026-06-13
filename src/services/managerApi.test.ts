import { describe, expect, it } from "vitest";

import { isNetworkError } from "./managerApi";

describe("isNetworkError", () => {
  it("classifies transport and TLS failures as connectivity errors", () => {
    expect(
      isNetworkError(
        "update engine error: io error: curl failed for host=codexapp.agentsmirror.com exit=35: stderr='curl: (35) schannel: failed to receive handshake, SSL/TLS connection failed'",
      ),
    ).toBe(true);
    expect(isNetworkError("curl: (6) Could not resolve host: codexapp.agentsmirror.com")).toBe(
      true,
    );
    expect(isNetworkError("curl: (28) Operation timed out after 20000 milliseconds")).toBe(true);
  });

  it("classifies the macOS auto-source fallback failure as connectivity", () => {
    expect(
      isNetworkError("both the mirror and OpenAI official appcast are unreachable"),
    ).toBe(true);
  });

  it("does not treat server responses or verification failures as connectivity", () => {
    expect(
      isNetworkError(
        "update engine error: curl failed for https://example.test/appcast.xml: curl: (22) The requested URL returned error: 404",
      ),
    ).toBe(false);
    expect(isNetworkError("appcast enclosure missing edSignature")).toBe(false);
    expect(isNetworkError("EdDSA signature does not match")).toBe(false);
  });
});
