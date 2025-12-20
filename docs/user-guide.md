
# CAN Bridge Daemon – User & Client Guide

This document explains how to **integrate clients** with the CAN Bridge Daemon
using **TCP**, **WebSocket**, or **gRPC**, and using **JSONL (text)** or **binary**
protocols.

This document covers:

- protocol matrix and when to use which transport
- common concepts (CAN frames, operations)
- detailed protocol specifications for each transport mode
- concrete **examples** (websocat, grpcurl, Python)
- client integration patterns and best practices
- common errors and fixes
- compatibility guarantees

---

## 1. Protocol Matrix

| Transport | Mode | Endpoint |
|---------|----------|----------|
| TCP | JSONL | tcp://HOST:9500 |
| TCP | Binary | tcp://HOST:9500 |
| WebSocket | JSONL | ws://HOST:9501/ws/jsonl |
| WebSocket | Binary | ws://HOST:9501/ws/binary |
| gRPC | Protobuf | HOST:9502 |

* TCP Transport supports both JSONL and Binary protocols on the same port (9500). The
client must perform a handshake to select the protocol. If the magic header for binary
is detected, the binary protocol is used; otherwise, JSONL is assumed. 
Magic header is: `CBD1` (0x43 0x42 0x44 0x31) to start the binary protocol else JSONL is assumed.
* WebSocket Transport uses different paths for JSONL and Binary protocols.
* gRPC Transport uses Protobuf messages over HTTP/2.

## When to use which transport mode

* **JSONL** → debugging, CLI tools, early development
* **Binary WS/TCP** → UI apps, high-rate monitoring
* **gRPC** → typed clients, long-running services

---

## 2. Common Concepts

### CAN Frames

All transports expose the same logical operations:

- `list_ifaces`
- `subscribe`
- `unsubscribe`
- `send_frame`
- `frame` events
- `ping / pong`

Only the **encoding and framing differ**.

---

## 3. TCP & WebSocket – JSONL (Text)

All JSONL messages are single-line JSON objects (JSONL = one JSON object per line). For WebSocket, each frame contains one JSON object; for TCP JSONL, each line is one JSON object.

| Field  | Type   | Required | Constraints | Notes                                                               |
| ------ | ------ | -------: | ----------- | ------------------------------------------------------------------- |
| `type` | string |        ✅ | non-empty   | Message discriminator (e.g. `client_hello`, `list_ifaces`, `frame`) |

---

### 1.2 Handshake (JSONL)

#### `client_hello` (client → daemon)

| Field      | Type   | Required | Constraints                         | Example          |
| ---------- | ------ | -------: | ----------------------------------- | ---------------- |
| `type`     | string |        ✅ | must be `client_hello`              | `"client_hello"` |
| `client`   | string |        ❌ | non-empty recommended               | `"py-ws-test"`   |
| `protocol` | string |        ❌ | recommended values: `json`, `jsonl` | `"json"`         |

**Example**

```json
{"type":"client_hello","client":"py-ws-test","protocol":"json"}
```

#### `hello_ack` (daemon → client)

| Field         | Type          | Required | Constraints         | Example                           |
| ------------- | ------------- | -------: | ------------------- | --------------------------------- |
| `type`        | string        |        ✅ | must be `hello_ack` | `"hello_ack"`                     |
| `version`     | string        |        ❌ | semver recommended  | `"1.2.3"`                         |
| `server_name` | string        |        ❌ |                     | `"can-bridge-daemon"`             |
| `features`    | array[string] |        ❌ |                     | `["ws-jsonl","ws-binary","grpc"]` |

**Example**

```json
{"type":"hello_ack","version":"1.2.3","server_name":"can-bridge-daemon","features":["ws-jsonl","ws-binary","grpc"]}
```



#### Handshake rule (important)

For TCP and WS transports:

* Client MUST send `client_hello` first
* Daemon replies `hello_ack`

For gRPC:

* `Hello(ClientHello)->HelloAck` exists, but it is not required there.

---

### 1.3 Health (JSONL)

#### `ping` (client → daemon)

| Field  | Type    | Required | Constraints        | Example  |
| ------ | ------- | -------: | ------------------ | -------- |
| `type` | string  |        ✅ | `ping`             | `"ping"` |
| `id`   | integer |        ✅ | uint64 recommended | `42`     |

#### `pong` (daemon → client)

| Field  | Type    | Required | Constraints        | Example  |
| ------ | ------- | -------: | ------------------ | -------- |
| `type` | string  |        ✅ | `pong`             | `"pong"` |
| `id`   | integer |        ✅ | must equal ping id | `42`     |

---

### 1.4 Interface enumeration (JSONL)

#### `list_ifaces` (client → daemon)

| Field  | Type   | Required | Constraints   | Example         |
| ------ | ------ | -------: | ------------- | --------------- |
| `type` | string |        ✅ | `list_ifaces` | `"list_ifaces"` |

#### `ifaces` (daemon → client)

| Field   | Type          | Required | Constraints  | Example           |
| ------- | ------------- | -------: | ------------ | ----------------- |
| `type`  | string        |        ✅ | `ifaces`     | `"ifaces"`        |
| `items` | array[string] |        ✅ | may be empty | `["can0","can1"]` |

---

### 1.5 Subscriptions (JSONL)

#### `subscribe` (client → daemon)

| Field    | Type          | Required | Constraints           | Example       |
| -------- | ------------- | -------: | --------------------- | ------------- |
| `type`   | string        |        ✅ | `subscribe`           | `"subscribe"` |
| `ifaces` | array[string] |        ✅ | non-empty recommended | `["can0"]`    |

#### `subscribed` (daemon → client)

| Field    | Type          | Required | Constraints            | Example        |
| -------- | ------------- | -------: | ---------------------- | -------------- |
| `type`   | string        |        ✅ | `subscribed`           | `"subscribed"` |
| `ifaces` | array[string] |        ✅ | echoes accepted ifaces | `["can0"]`     |

#### `unsubscribe` (client → daemon)

| Field  | Type   | Required | Constraints   | Example         |
| ------ | ------ | -------: | ------------- | --------------- |
| `type` | string |        ✅ | `unsubscribe` | `"unsubscribe"` |

#### `unsubscribed` (daemon → client)

| Field  | Type   | Required | Constraints    | Example          |
| ------ | ------ | -------: | -------------- | ---------------- |
| `type` | string |        ✅ | `unsubscribed` | `"unsubscribed"` |

---

### 1.6 Send frame (JSONL)

#### `send_frame` (client → daemon)

| Field      | Type    | Required | Constraints                         | Notes                         |
| ---------- | ------- | -------: | ----------------------------------- | ----------------------------- |
| `type`     | string  |        ✅ | `send_frame`                        |                               |
| `iface`    | string  |        ✅ | must exist on daemon                |                               |
| `id`       | integer |        ✅ | 0..=0x1FFFFFFF (29-bit) recommended | depends on your daemon policy |
| `is_fd`    | boolean |        ✅ |                                     | CAN-FD vs classic             |
| `brs`      | boolean |        ✅ |                                     | FD bitrate switch             |
| `esi`      | boolean |        ✅ |                                     | FD error state indicator      |
| `data_hex` | string  |        ✅ | hex string, even length             | payload bytes                 |

**Payload constraints**

* Classic CAN: `len(data) <= 8`
* CAN-FD: `len(data) <= 64`
* `data_hex` length must be `2 * payload_len`

**Example**

```json
{"type":"send_frame","iface":"can0","id":801,"is_fd":false,"brs":false,"esi":false,"data_hex":"deadbeef"}
```

#### `send_ack` (daemon → client)

| Field   | Type    | Required | Constraints           | Example         |
| ------- | ------- | -------: | --------------------- | --------------- |
| `type`  | string  |        ✅ | `send_ack`            | `"send_ack"`    |
| `ok`    | boolean |        ✅ |                       | `true`          |
| `error` | string  |        ❌ | present when ok=false | `"invalid hex"` |

---

### 1.7 Frame events (JSONL)

#### `frame` (daemon → client)

| Field      | Type    | Required | Constraints      | Notes                                                      |
| ---------- | ------- | -------: | ---------------- | ---------------------------------------------------------- |
| `type`     | string  |        ✅ | `frame`          |                                                            |
| `ts_ms`    | integer |        ✅ | uint64           | milliseconds since epoch or monotonic—document your choice |
| `iface`    | string  |        ✅ |                  | interface name                                             |
| `dir`      | string  |        ✅ | `"rx"` or `"tx"` |                                                            |
| `id`       | integer |        ✅ |                  | arbitration id                                             |
| `is_fd`    | boolean |        ✅ |                  |                                                            |
| `data_hex` | string  |        ✅ | hex, even length |                                                            |

---

### 1.8 Error (JSONL)

#### `error` (daemon → client)

| Field     | Type   | Required | Constraints | Example         |
| --------- | ------ | -------: | ----------- | --------------- |
| `type`    | string |        ✅ | `error`     | `"error"`       |
| `message` | string |        ✅ | non-empty   | `"bad request"` |

---

## 1.9 Binary protocol (TCP/WS binary)

### Binary frame layout

| Segment | Size | Type   | Notes               |
| ------- | ---: | ------ | ------------------- |
| Magic   |    4 | bytes  | ASCII `CBD1`        |
| Header  |   12 | struct | little-endian       |
| Payload |    N | bytes  | `payload_len` bytes |

### Binary header layout (12 bytes)

| Field         | Size | Type | Notes                 |
| ------------- | ---: | ---- | --------------------- |
| `msg_type`    |    2 | u16  | message discriminator |
| `flags`       |    2 | u16  | reserved / future     |
| `payload_len` |    4 | u32  | bytes that follow     |
| `reserved`    |    4 | u32  | set 0                 |

---

### 3.6 Minimal Example Client (Python + websocket-client)

This example connects to the JSONL WebSocket endpoint, performs the handshake,
lists interfaces, and prints the results.

```python
import json
import websocket

WS_URL = "ws://127.0.0.1:9501/ws/jsonl"

ws = websocket.create_connection(WS_URL, timeout=3)

# client speaks first
ws.send(json.dumps({"type":"client_hello","client":"py-demo","protocol":"json"}))
hello_ack = json.loads(ws.recv())
print("hello_ack:", hello_ack)

ws.send(json.dumps({"type":"list_ifaces"}))
print("ifaces:", json.loads(ws.recv()))

ws.close()
```

---

## 4. TCP & WebSocket – Binary Protocol

### 4.1 Binary Framing

All binary transports share the same framing:

| Field    | Size                |
| -------- | ------------------- | 
| Magic    | 4 bytes ("CBD1")    |
| Header   | 12 bytes            |
| Payload  | N bytes             |

Header field is further detailed as(little-endian):

| Field       | Type |
| ----------- | ---- |
| msg_type    | u16  |
| flags       | u16  |
| payload_len | u32  |
| reserved    | u32  |

---

### 4.2 Handshake (Binary)

Client → Daemon:

* `ClientHello (msg_type = 101)`

Daemon → Client:

* `HelloAck (msg_type = 1)`

The handshake payload contains UTF-8 strings with length prefixes.

---

### 4.3 Message Types

| Direction       | Message      | ID  |
| --------------- | ------------ | --- |
| client → daemon | ClientHello  | 101 |
| client → daemon | Ping         | 102 |
| client → daemon | ListIfaces   | 103 |
| client → daemon | Subscribe    | 104 |
| client → daemon | Unsubscribe  | 105 |
| client → daemon | SendFrame    | 106 |
| daemon → client | HelloAck     | 1   |
| daemon → client | Pong         | 2   |
| daemon → client | Ifaces       | 3   |
| daemon → client | Subscribed   | 4   |
| daemon → client | Unsubscribed | 5   |
| daemon → client | SendAck      | 6   |
| daemon → client | FrameEvent   | 7   |
| daemon → client | Error        | 8   |

---

## 4.4 Example: List Interfaces (Binary via websocat)

```bash
websocat -b ws://localhost:9501/ws/binary --binary-mode=bytes \
  <(echo -n -e 'CBD1'$(printf '\x65\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00')$(printf '\x09\x00')'my-app''\x04''json')
```

**Daemon Response (hex dump):**

```
00000000  43 42 44 31  01 00 00 00  00 00 00 00  1a 00 00 00  |CBD1............|
00000010  00 00 00 00  0f 00 00 00  63 61 6e 2d  62 72 69 64 67  |........can-bridg|
00000020  65 2d 64 61 65  6d 6f 6e  00                    |e-daemon.        |
``` 

The response contains a `HelloAck` message with server name `can-bridge-daemon`.

## 4.5 Example Python Client (Binary via websocket-client)

This example connects to the binary WebSocket endpoint, performs the handshake,
and prints the `HelloAck` response.

```python
import struct
import websocket
WS_URL = "ws://127.0.0.1:9501/ws/binary"
def build_message(msg_type, payload_bytes):
    magic = b'CBD1'
    payload_len = len(payload_bytes)
    header = struct.pack('<HHII', msg_type, 0, payload_len, 0)
    return magic + header + payload_bytes
ws = websocket.create_connection(WS_URL, timeout=3)

# ClientHello
client_name = b'my-app'
protocol = b'json'
payload = struct.pack('<H', len(client_name)) + client_name + struct.pack('<H', len(protocol)) + protocol
msg = build_message(101, payload)
ws.send(msg)

# Receive HelloAck
response = ws.recv()
print("Received:", response)
ws.close()
```

---

## 5. gRPC API

### 5.1 Hello (Optional Handshake)

```proto
rpc Hello(ClientHello) returns (HelloAck);
```

Example (grpcurl):

```bash
grpcurl -plaintext localhost:9502 \
  canbridge.v1.CanBridge/Hello \
  '{"client":"grpc-test","protocol":"grpc"}'
```

---

### 5.2 List Interfaces

```bash
grpcurl -plaintext localhost:9502 \
  canbridge.v1.CanBridge/ListIfaces
```

---

### 5.3 Subscribe (Streaming)

```proto
rpc Subscribe(SubscribeReq) returns (stream FrameEvent);
```

The server streams CAN frames as protobuf messages.

---

## 6. Client Integration Patterns (Recommended)

Always do this sequence in client code:

1. Connect Connect to transport
2. Handshake (WS/TCP) or Hello (gRPC)
3. `list_ifaces`
4. let user select iface(s)
5. Subscribe
6. Start reader loop
7. Send frames from a separate task/thread

### Subscription threading model (important for WS JSONL)

Use a **reader thread** that continuously drains frames and pushes them to a queue. That’s a good model for real apps.

Rule of thumb:

- One thread/task always reading
- Another thread/task sending commands
- Communicate via queues/channels
- One subscription per connection (recommended)

## “Best practice” client checklist

* Always send handshake immediately on WS
* Always enforce CAN DLC rules client-side (8 for classic, 64 for FD) to avoid round trips (your tests cover these).
* For high-rate traffic, use WS Binary or gRPC streaming
* JSONL is best for debugging and tooling
* Batch UI updates (don’t repaint on every frame)
* **Always have a dedicated reader** for WS/TCP connections

---

## 7. Common Errors & Fixes

| Error                      | Cause                                | Fix                             |
| -------------------------- | ------------------------------------ | ------------------------------- |
| HTTP 404 on WS             | You’re connecting to the wrong path. | Use `/ws/jsonl` or `/ws/binary` |
| WS returns HTTP 200        | Not a WS upgrade handler          | Check routing                   |
| Handshake timeout          | Client didn’t send `client_hello` first | Send `client_hello` as the very first message|
| Binary: “bad magic”         | Your client and server disagree on framing | Ensure the first 4 bytes are `CBD1`|

---

## 8. Compatibility Promise

All transports expose the **same logical CAN operations**.
Only the **encoding and framing differ**.

This guarantees:

* easy migration between transports
* consistent behavior across clients

---
