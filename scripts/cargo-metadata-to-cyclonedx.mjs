#!/usr/bin/env node
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const [, , input, output] = process.argv;
if (!input || !output) {
  console.error("usage: cargo-metadata-to-cyclonedx.mjs <cargo-metadata.json> <output.cdx.json>");
  process.exit(2);
}

const metadata = JSON.parse(readFileSync(input, "utf8"));
const rootId = metadata.resolve?.root;
const rootPackage = metadata.packages?.find((pkg) => pkg.id === rootId);

const purl = (pkg) => `pkg:cargo/${pkg.name}@${pkg.version}`;
const licenseInfo = (pkg) => (pkg.license ? [{ expression: pkg.license }] : undefined);
const externalReferences = (pkg) =>
  [pkg.repository ? { type: "vcs", url: pkg.repository } : null, pkg.homepage ? { type: "website", url: pkg.homepage } : null].filter(
    Boolean,
  );

const components = (metadata.packages || []).map((pkg) => {
  const component = {
    type: pkg.id === rootId ? "application" : "library",
    "bom-ref": purl(pkg),
    name: pkg.name,
    version: pkg.version,
    purl: purl(pkg),
  };
  const licenses = licenseInfo(pkg);
  const refs = externalReferences(pkg);
  if (licenses) component.licenses = licenses;
  if (refs.length > 0) component.externalReferences = refs;
  return component;
});

const bom = {
  bomFormat: "CycloneDX",
  specVersion: "1.5",
  serialNumber: `urn:uuid:${randomUUID()}`,
  version: 1,
  metadata: {
    timestamp: new Date().toISOString(),
    tools: [
      {
        vendor: "Codex App Manager",
        name: "cargo-metadata-to-cyclonedx",
      },
    ],
    component: rootPackage
      ? {
          type: "application",
          "bom-ref": purl(rootPackage),
          name: rootPackage.name,
          version: rootPackage.version,
          purl: purl(rootPackage),
        }
      : undefined,
  },
  components,
};

writeFileSync(output, JSON.stringify(bom, null, 2) + "\n");
