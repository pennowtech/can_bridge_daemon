# CAN Bridge Daemon

The CAN Bridge Daemon exposes Linux SocketCAN interfaces (`can0`, `vcan0`, etc.)
over multiple network protocols so that **local or remote clients** can:

## Features:

- List CAN interfaces
- Send CAN / CAN-FD frames
- Subscribe to RX/TX frame events
- Monitor and debug CAN traffic remotely
- basic health check (`ping` → `pong`)

## It is designed to be:

- transport-agnostic
- efficient for high-rate CAN traffic
- easy to integrate from Python, Rust, JS, or gRPC clients

## Typical Use Cases

* Remote CAN monitoring dashboards
* Hardware-in-the-loop testing
* Headless CAN gateways
* Distributed automotive tooling
* UI apps (Web / Mobile / Desktop)

---

## Supported Protocols

The daemon supports **three transports** and **two encodings/modes**:

| Transport | Mode | Use case |
|---------|----------|---------|
| TCP | JSONL (text) | Simple scripting, debugging |
| TCP | Binary | High-performance streaming |
| WebSocket | JSONL (text) | Browser & tooling friendly |
| WebSocket | Binary | High-performance remote UI |
| gRPC | Protobuf | Typed APIs, streaming, tooling |

* Text / JSONL (human friendly, easy to debug)
* Binary (fast, high throughput, low CPU overhead, stable framing)

Use gRPC when you want:

- typed API
- easy client generation
- built-in streaming

---

## Documentation

Documents Overview for CAN Bridge Daemon

1. Project overview + quick start: `README.md`
2. Full user & client integration guide: [user guide](docs/user-guide.md)
3. Protocol schemas: see source & [protocol docs](docs/protocol.md)
4. grpc API reference: [gRPC reference](proto/can_bridge.proto)
5. Example clients: `test/`

---

## Handshake Model (Important)

For **TCP and WebSocket** transports:

> **Client MUST send `client_hello` first**. This establishes protocol version, encoding/mode, and client identity.

```txt
client → daemon : client_hello
daemon → client : hello_ack
```

For gRPC, it's not needed as gRPC is already binary because of protobuf.

Complete detail of various operations is in the [user guide](docs/user-guide.md).

---

## Setup & Run Instructions

This section explains how to **build, configure, and run** the CAN Bridge Daemon end-to-end.

### Operating System

* **Linux only** (uses SocketCAN)

  * Ubuntu 20.04+
  * Debian
  * Arch
  * WSL2 **with systemd + CAN support** (advanced)

### Required system packages

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  clang \
  protobuf-compiler \
  iproute2
```

> `protobuf-compiler` is required for gRPC code generation.

### Rust Toolchain

Install Rust (stable):

```bash
curl https://sh.rustup.rs -sSf | sh
source ~/.cargo/env
```

Verify:

```bash
rustc --version
cargo --version
```

### CAN Interface Setup

On Linux you usually have:

* physical: `can0`, `can1`
* virtual (for _dev_): `vcan0`

#### Option A: Physical CAN

Ensure your CAN interface exists:

```bash
ip link show can0
```

Bring it up if needed:

```bash
sudo ip link set can0 up type can bitrate 500000
```

#### Option B: Virtual CAN (Recommended for development)

If you don’t have a physical CAN, create vcan:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

Verify:

```bash
ip link show vcan0
```

---

## Build the Project

Build from source using `cargo`.

### Debug build

```bash
cargo build
```

### Release build (recommended)

```bash
cargo build --release
```

Binary will be at:

```text
target/release/can-bridge-daemon
```

---

## Run the daemon

Run it with whatever bind addresses/ports you use.
example ports below are: TCP `9500`, WS `9501`, gRPC `9502`.

### Minimal run (all transports enabled)

```bash
can-bridge-daemon \
  --tcp-bind 0.0.0.0:9500 \
  --ws-bind  0.0.0.0:9501 \
  --grpc-bind 0.0.0.0:9502
```

> **Why sudo?**
> SocketCAN access usually requires elevated privileges.

## Connect a client

* WebSocket JSONL: `ws://HOST:9501/ws/jsonl`
* WebSocket Binary: `ws://HOST:9501/ws/binary`
* gRPC: `HOST:9502`

---

### Expected startup logs

```text
INFO tcp server listening on 0.0.0.0:9500
INFO ws server listening (/ws/jsonl, /ws/binary) on 0.0.0.0:9501
INFO grpc server listening on 0.0.0.0:9502
INFO detected CAN interfaces: can0, vcan0
```

---

## Verify with CLI Tools

### WebSocket JSONL (text)

Using `websocat`:

```bash
websocat ws://127.0.0.1:9501/ws/jsonl
```

Then type:

```json
{"type":"client_hello","client":"websocat","protocol":"json"}
{"type":"list_ifaces"}
```

### gRPC

Using `grpcurl`:

```bash
grpcurl -plaintext localhost:9502 list
```

Handshake:

```bash
grpcurl -plaintext localhost:9502 \
  canbridge.v1.CanBridge/Hello \
  '{"client":"cli","protocol":"grpc"}'
```

List interfaces:

```bash
grpcurl -plaintext localhost:9502 \
  canbridge.v1.CanBridge/ListIfaces
```

---

## Run the Test Suite

Use the provided Python tests.

To run them, first install `poetry`. Refer to [Poetry installation guide](https://python-poetry.org/docs/#installation).
Instrunctions are also provided under `test/README.md`.

Then tests can be run as follows:

```bash
poetry run python test/test_can_bridge_ws_binary.py \
  --url ws://127.0.0.1:9501/ws/binary \
  --iface vcan0
```

> Ensure the daemon is already running before executing tests.

There are many test scripts available for different transports and encodings. Refer to `test/README.md` for details.

---

## Common Setup Issues

### ❌ `permission denied` on CAN socket

Run daemon as root:

```bash
sudo ./can-bridge-daemon ...
```

Or grant capabilities:

```bash
sudo setcap cap_net_raw,cap_net_admin+eip ./can-bridge-daemon
```

---

### ❌ `No such device: can0`

You don’t have a CAN interface.

Create `vcan0`:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

---

### ❌ gRPC fails to start

Ensure:

```bash
protoc --version
```

If missing:

```bash
sudo apt install protobuf-compiler
```

---

## 🔁 Recommended Development Workflow

1. Use **vcan0**
2. Start daemon in one terminal
3. Run Python tests in another
4. Use TCP/WS **JSONL** mode for debugging
5. Switch to **TCP Binary/ WS Binary / gRPC** for performance

---

## Architecture Reminder

* **TCP & WebSocket**

  * Support **JSONL (text)** and **Binary**
  * Require `client_hello → hello_ack` as a first message
  * Streaming used for CAN frames

* **gRPC**

  * Uses protobuf
  * Streaming used for CAN frames

## License

GPL-3.0 License
