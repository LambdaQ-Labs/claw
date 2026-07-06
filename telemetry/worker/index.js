// Achuk telemetry ingest — a Cloudflare Worker writing to R2.
//
// Cost profile (the whole point): Workers free tier = 100k requests/day,
// R2 free tier = 10 GB storage + 1M writes/month. Telemetry uploads are
// one gzipped JSONL batch per `achuk telemetry share`, a few KiB each —
// this runs at $0 until the project has thousands of active users.
//
// Deploy:
//   cd telemetry/worker
//   wrangler r2 bucket create achuk-telemetry
//   wrangler deploy
// then point a route (telemetry.achuk.dev/v1/ingest) at it.

const MAX_BODY = 6 * 1024 * 1024; // one full client log, gzipped, with margin

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    // Read-only stats: how much has landed. No PII — counts + bytes only.
    if (request.method === "GET" && url.pathname === "/stats") {
      let count = 0, bytes = 0, days = new Set(), cursor = undefined;
      do {
        const page = await env.TELEMETRY.list({ prefix: "v1/", cursor, limit: 1000 });
        for (const o of page.objects) {
          count++; bytes += o.size;
          const d = o.key.split("/")[1];
          if (d) days.add(d);
        }
        cursor = page.truncated ? page.cursor : undefined;
      } while (cursor);
      return Response.json({ uploads: count, bytes, days: days.size, live: true });
    }
    if (request.method !== "POST" || url.pathname !== "/v1/ingest") {
      return new Response("achuk telemetry ingest\n", { status: 404 });
    }
    const len = Number(request.headers.get("content-length") || 0);
    if (len > MAX_BODY) {
      return new Response("too large", { status: 413 });
    }
    const body = await request.arrayBuffer();
    if (body.byteLength === 0 || body.byteLength > MAX_BODY) {
      return new Response("bad body", { status: 400 });
    }
    // Key by day/uuid: append-only, no reads on the hot path, trivially
    // batch-downloadable for training runs (rclone/aws-cli over R2).
    const day = new Date().toISOString().slice(0, 10);
    const key = `v1/${day}/${crypto.randomUUID()}.jsonl.gz`;
    await env.TELEMETRY.put(key, body, {
      httpMetadata: { contentType: "application/jsonl", contentEncoding: "gzip" },
    });
    return new Response("ok", { status: 200 });
  },
};
