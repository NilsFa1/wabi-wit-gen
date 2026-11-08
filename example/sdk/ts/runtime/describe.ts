// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: readable messages for contract error values.

/**
 * `{ tag: "server", value: { status: 503, code: "x" } }` -> `server: {"status":503,"code":"x"}`,
 * `{ tag: "network", value: "timeout" }` -> `network: timeout`, `{ tag: "not-paired" }` -> `not-paired`.
 */
export function describeValue(v: unknown): string {
  if (v !== null && typeof v === "object" && "tag" in v) {
    const { tag, value } = v as { tag: unknown; value?: unknown };
    if (value === undefined) return String(tag);
    return `${String(tag)}: ${typeof value === "string" ? value : json(value)}`;
  }
  return typeof v === "string" ? v : json(v);
}

function json(v: unknown): string {
  try {
    return JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x instanceof Uint8Array ? `<${x.length} bytes>` : x));
  } catch {
    return String(v);
  }
}
