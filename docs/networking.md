# Networking in Claw

Claw programs do I/O through a **platform** — a host that provides the
effects (print, sockets, files). The default `claw run` uses a print-only
platform, but the toolchain ships richer platforms you can target with an
explicit app header. This page shows a **real HTTP server written in Claw**,
verified end-to-end with `curl`.

> Status: **experimental (macOS).** One command scaffolds a networked
> project:
>
> ```sh
> claw new myapi --platform http
> cd myapi && claw run     # prints the port, then serves a request
> ```
>
> This copies the bundled HTTP platform into your project and generates the
> handler below. Prebuilt hosts are macOS (arm64); the Linux host is a
> roadmap item. Everything here is real output.

## A Claw HTTP auth gateway

The host listens on a loopback socket, hands your `main!` the raw HTTP header
block, and sends back whatever `U64` you return as the response body. Here is
a complete auth gateway:

```claw
app [main!] { pf: platform "./platform/main.roc" }

# valid token -> 200, wrong token -> 403, no token -> 401
is_authorized : Str -> Bool
is_authorized = |headers| Str.contains(headers, "X-Auth-Token: let-me-in")

has_token : Str -> Bool
has_token = |headers| Str.contains(headers, "X-Auth-Token:")

status_for : Str -> U64
status_for = |headers| {
    if is_authorized(headers) 200
    else if has_token(headers) 403
    else 401
}

main! : Str => U64
main! = |headers| status_for(headers)
```

Run it (the platform prints the port it bound), then hit it:

```console
$ clawc app.roc &
60234

$ curl -s localhost:60234 -H "Content-Length: 0" -H "X-Auth-Token: let-me-in"
200
$ curl -s localhost:60234 -H "Content-Length: 0" -H "X-Auth-Token: hunter2"
403
$ curl -s localhost:60234 -H "Content-Length: 0"
401
```

All three verified. The server binds a real TCP socket, accepts a real HTTP
request, routes on the header content in pure Claw, and returns a proper
`HTTP/1.1 200 OK` response.

## Typed header parsing (decoders derived from a record)

The bundled `http-headers` platform goes further: it can derive an HTTP
header **parser from a record type** at compile time.

```claw
main! : Str => U64
main! = |headers| {
    decoded = parse_headers(headers)?        # parser_for() derived from the record
    decoded.content_length
        + decoded.request_count
        + Str.count_utf8_bytes(decoded.cache_control)
        + ...
}
```

A request with `Content-Length: 0`, `Request-Count: 7`, `Cache-Control:
no-cache`, `Foo: hello`, `X-Auth-Token: secret` returns `26`
(`0 + 7 + 8 + 5 + 6`) — the fields decoded straight off the wire into a typed
record, no manual string munging.

## What this means for the roadmap

Network I/O is **not** blocked on new language work — a working socket/HTTP
host already exists. The remaining work is productization: bundling the HTTP
(and a file/stdin) platform as first-class `claw` targets, building the Linux
host, and a friendly `claw new --platform http` scaffold. Tracked for v0.1.1.
