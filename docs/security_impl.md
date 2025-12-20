

# Authentication / anti-hijack strategy (best practical approach)

We want to prevent:

* random devices on the network connecting
* malicious clients subscribing to data
* malicious clients sending frames (most dangerous)
* hijacking / MITM sniffing or modifying messages

The best strategy depends on whether this runs:

* only on localhost
* on a LAN
* across the internet / cloud

Here’s a strategy that fits our architecture.

## 1) Baseline hardening (do this regardless)

* **Bind to localhost by default**: `127.0.0.1`
* Add `--bind 0.0.0.0` only when you explicitly want remote access.
* Separate privileges:

  * if possible run daemon as non-root and grant CAP_NET_RAW / CAP_NET_ADMIN only if needed
* Add clear policies:

  * default deny sending frames unless authenticated/authorized

## 2) Prefer TLS with mutual authentication (mTLS) for remote use

This is the “best” general security posture.

### TCP

* Use **TLS** (rustls) around the TCP transport.
* Best: **mTLS**: both server and client present certs
* Benefits:

  * encryption (no sniffing)
  * identity (client cert identifies user/device)
  * prevents MITM (proper PKI)

### WebSocket

* Use **WSS** (WebSocket over TLS) with rustls.
* We can also do mTLS at the TLS layer.

### gRPC

* gRPC **natively supports TLS/mTLS** with tonic.
* This is the cleanest one to secure properly.

**Authorization** (after identity):

* map client cert Subject/URI SAN to roles:

  * “read-only” (subscribe/list)
  * “tx-allowed” (send_frame)
  * “admin” (candump replay / config)

## 3) Lightweight option: HMAC token handshake (fast, simple)

If you want minimal overhead and easy Python clients:

* On connect, client must send `AUTH` message containing:

  * `client_id`
  * `timestamp`
  * `nonce`
  * `signature = HMAC(secret, client_id|timestamp|nonce)`
* Server verifies:

  * timestamp within window (e.g., ±30s)
  * nonce not reused (store last N nonces per client)
* Then server marks session authenticated.

This works for:

* JSONL
* Binary protocol
* WebSocket JSON/Binary

**But**: without TLS, tokens can be sniffed and replayed.
So HMAC handshake should be used **with TLS** in non-local environments.

## 4) Recommended layered model (best balance)

### Identity: mTLS or token

* Local-only: token may be enough
* LAN/remote: mTLS

### Authorization: per-RPC / per-message

Even authenticated clients should not automatically get TX permission.

Introduce an **AuthContext** in our application layer:

* `client_id`
* `roles: {Read, Tx, Admin}`
* `transport: tcp/ws/grpc`

Then enforce:

* `Subscribe/List` requires `Read`
* `SendFrame` requires `Tx`
* later “replay mode” requires `Admin`

This fits clean architecture:

* Transport extracts auth and builds `AuthContext`
* App layer enforces permissions
* Domain stays pure

## 5) Concrete best practice for our daemon

What can be the “best strategy” for our project:

### Development (easy)

* Default bind: `127.0.0.1`
* Require a **static token** via env var:

  * `CAN_BRIDGE_TOKEN=...`
* Client sends token in first handshake:

  * JSON: `client_hello` includes `token`
  * Binary: `HELLO_ACK` includes token field
  * WS: first message includes token
  * gRPC: token via metadata `authorization: Bearer ...`

### Production (secure)

* Enable **TLS/mTLS**:

  * gRPC: mTLS first
  * WS/TCP: TLS
* Use client cert role mapping for authorization.
* Disable TX by default unless client identity is in allowlist.

## 6) Prevent “hijack” even from valid clients

Add controls:

* **Rate limits**

  * per-connection frame send rate, per-second bytes
* **Command allowlist**

  * forbid `send_frame` unless explicitly enabled
* **Audit logs**

  * log `client_id`, peer addr, and every `send_frame` summary
* **Replay protection**

  * if using tokens, include nonce and short TTL

---

## If you want, next step I can implement:

**Step 11.5: Authentication**

* Add `AuthConfig` (token / mTLS hooks)
* Add `AuthContext` to `BridgeService::handle(...)`
* Enforce roles for SendFrame
* gRPC metadata auth
* TCP/WS handshake auth (JSON + binary)

Preferred auth mode in different scenarios:

* **Token only** (fastest)
* **mTLS** (strongest)
* **Both** (recommended: token in dev, mTLS in prod)
