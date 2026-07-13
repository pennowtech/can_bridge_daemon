# CAN Bridge Daemon

CAN Bridge Daemon is a small Linux service that exposes SocketCAN interfaces over network transports. It lets a desktop app, script, test runner, or remote tool list CAN interfaces, subscribe to traffic, and transmit CAN or CAN-FD frames without running directly on the machine that owns the CAN device.

The daemon is usually paired with Rusty CAN Studio, but it is not tied to that app. It speaks simple JSONL over TCP/WebSocket, a binary framing mode for higher throughput, and gRPC for typed clients.

## What It Does

- Lists available SocketCAN interfaces such as `can0`, `can1`, and `vcan0`.
- Subscribes clients to CAN/CAN-FD frame streams.
- Sends CAN/CAN-FD frames to a selected SocketCAN interface.
- Reports send acknowledgements after the daemon attempts to send on the selected interface.
- Supports daemon-side capture filters so clients can receive only matching raw CAN IDs.
- Provides TCP, WebSocket, and gRPC transports.
- Works well from WSL, Linux test machines, and headless CAN gateways.

## When To Use It

Use this daemon when the CAN interface is not on the same machine as your UI or automation code.

Common setups:

- Rusty CAN Studio on Windows talking to SocketCAN in WSL.
- A test laptop connected to a Linux CAN gateway.
- Hardware-in-the-loop scripts that need live CAN traffic over the network.
- Browser or desktop tools that cannot open SocketCAN directly.
- Remote debugging of CAN/CAN-FD traffic.

Typical flow:

```text
client app or script
        |
        | TCP, WebSocket, or gRPC
        v
can_bridge_daemon on Linux or WSL
        |
        | SocketCAN
        v
vcan0 / can0 / can1
```

## Supported Transports

| Transport | Mode | Good for |
| --- | --- | --- |
| TCP | JSONL text | Scripting and manual debugging |
| TCP | Binary | Higher throughput clients |
| WebSocket | JSONL text | Desktop apps, browser-friendly tools, debugging |
| WebSocket | Binary | Higher throughput remote UI clients |
| gRPC | Protobuf | Typed clients and generated APIs |

For TCP and WebSocket, the client must send `client_hello` first. This tells the daemon which protocol mode the client expects.

```text
client -> daemon: client_hello
daemon -> client: hello_ack
```

For gRPC, the protobuf service handles the API shape, so there is no JSONL handshake.

## Requirements

The daemon is Linux-first because it uses SocketCAN.

Tested and expected environments:

- Ubuntu or Debian
- WSL2 with working CAN or virtual CAN support
- Linux machines with physical CAN adapters
- Virtual CAN development with `vcan0`

Install system packages:

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

`protobuf-compiler` provides `protoc`, which is required during the Rust build because the gRPC code is generated from `proto/can_bridge.proto`.

Install Rust if needed:

```bash
curl https://sh.rustup.rs -sSf | sh
source ~/.cargo/env
```

Verify the tools:

```bash
rustc --version
cargo --version
protoc --version
```

## Prepare A CAN Interface

For development, `vcan0` is the easiest option:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
ip link show vcan0
```

For a physical interface, bring it up with the required bitrate:

```bash
sudo ip link set can0 up type can bitrate 500000
ip link show can0
```

For CAN-FD hardware, configure the data bitrate as required by your adapter and bus:

```bash
sudo ip link set can0 up type can bitrate 500000 dbitrate 2000000 fd on
```

## Build

If you just need a ready-built package, download the latest release from GitHub:

- CAN Bridge Daemon releases: https://github.com/pennowtech/can_daemon_rust/releases

Debug build:

```bash
cargo build
```

Release build:

```bash
cargo build --release
```

The release binary is written to:

```text
target/release/can_bridge_daemon
```

## Run

Run all transports with the common local development ports:

```bash
cargo run -- \
  --tcp-bind 0.0.0.0:9500 \
  --ws-bind 0.0.0.0:9501 \
  --grpc-bind 0.0.0.0:9502
```

Or run the release binary:

```bash
./target/release/can_bridge_daemon \
  --tcp-bind 0.0.0.0:9500 \
  --ws-bind 0.0.0.0:9501 \
  --grpc-bind 0.0.0.0:9502
```

SocketCAN access may require root or capabilities. For quick local testing, `sudo` is often the simplest option:

```bash
sudo ./target/release/can_bridge_daemon \
  --tcp-bind 0.0.0.0:9500 \
  --ws-bind 0.0.0.0:9501 \
  --grpc-bind 0.0.0.0:9502
```

If you do not want to run as root, grant the binary network capabilities:

```bash
sudo setcap cap_net_raw,cap_net_admin+eip ./target/release/can_bridge_daemon
```

## Connect From Rusty CAN Studio

1. Start the daemon where SocketCAN exists.
2. Open Rusty CAN Studio.
3. Open Connect.
4. Select Remote Daemon.
5. Use the WebSocket host and port, for example `127.0.0.1:9501`.
6. Click Discover to list daemon interfaces.
7. Select `vcan0`, `can0`, or another interface.
8. Save and connect.

For WSL, `localhost` often works, but this depends on your Windows and WSL networking setup. If it does not connect, use the WSL IP address or check firewall rules.

## WebSocket JSONL Example

Install `websocat` if you want a quick manual test.

```bash
websocat ws://127.0.0.1:9501/ws/jsonl
```

Send a hello first:

```json
{"type":"client_hello","client":"websocat","protocol":"json"}
```

List interfaces:

```json
{"type":"list_ifaces"}
```

Subscribe to `vcan0`:

```json
{"type":"subscribe","ifaces":["vcan0"]}
```

Send a frame:

```json
{"type":"send_frame","iface":"vcan0","id":405814273,"is_fd":true,"data_hex":"0101"}
```

## Capture Filters

Capture filters are daemon-side raw CAN ID filters. The daemon should not know protocol-specific words such as service identifier, source address, destination address, or command class. Instead, clients translate those ideas into raw CAN ID masks.

A filter has:

- `id`: the expected bits after masking.
- `mask`: which CAN ID bits matter.

A frame passes when:

```text
(frame.id & mask) == (id & mask)
```

Example: only service identifier `810` when the service identifier is stored in the lower 10 bits:

```text
id:   0000032A
mask: 000003FF
```

Equivalent expression:

```text
(frame.id & 0x000003FF) == (0x0000032A & 0x000003FF)
```

More examples:

Only exact CAN ID `0x18203C01`:

```text
id:   18203C01
mask: 1FFFFFFF
```

Only lower 10-bit service identifier `1`:

```text
id:   00000001
mask: 000003FF
```

Only frames where the top 4 command-class bits are `6` in a 29-bit CAN ID layout:

```text
id:   18000000
mask: 1C000000
```

Only frames from a block of IDs where the lower byte is ignored:

```text
id:   18203C00
mask: 1FFFFF00
```

Keep leading zeroes in the UI when entering filters. They make the mask easier to read and reduce mistakes.

## gRPC Quick Check

List services:

```bash
grpcurl -plaintext localhost:9502 list
```

Send hello:

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

## Test Workflow

A simple development loop:

1. Create `vcan0`.
2. Start the daemon.
3. Connect Rusty CAN Studio or a test script.
4. Send frames with `cansend`, Rusty CAN Studio, or a JSONL client.
5. Watch subscribed frame events.
6. Add capture filters when you want less traffic.

Run Rust checks:

```bash
cargo check
cargo test
```

Run Python transport tests if you use the `test/` scripts. See `test/README.md` for the exact setup.

## VS Code And Zed Tasks

This repo includes editor task files under `.vscode/` and `.zed/`. Use them to start common daemon configurations instead of typing long commands every time. The useful tasks are usually variants of:

- Run daemon on local development ports.
- Run daemon with `vcan0` ready for testing.
- Build release binary.
- Run checks or tests.

If you add a new port or interface workflow, update both task files so VS Code and Zed users get the same experience.

## Troubleshooting

### `cargo` is not found

Install Rust and reload the shell:

```bash
curl https://sh.rustup.rs -sSf | sh
source ~/.cargo/env
```

### `protoc` is missing

Install protobuf compiler:

```bash
sudo apt install protobuf-compiler
```

### Permission denied on CAN socket

Run with `sudo` or grant capabilities:

```bash
sudo setcap cap_net_raw,cap_net_admin+eip ./target/release/can_bridge_daemon
```

### `No such device: can0`

The interface does not exist or is not up. Create `vcan0` for development or bring up the physical interface:

```bash
ip link show
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

### The desktop app connects but sees no frames

Check these in order:

- The daemon is subscribed to the correct interface.
- The interface is up.
- Frames are actually present on the bus.
- Capture filters are not excluding everything.
- The app is connected to the correct WebSocket endpoint.
- WSL or firewall networking is not blocking traffic.

### `TX:sent` appears but no target responds

`TX:sent` means the daemon accepted the send request and the selected SocketCAN interface accepted the send call. It does not mean another ECU or simulator sent an application-level response. Use live capture, profile decoding, and response matching in the client when you need request/response behavior.

## Project Layout

```text
src/
  app and domain code
  transport implementations
  SocketCAN integration
proto/
  gRPC protobuf definition
scripts/
  helper scripts
docs/
  protocol and user documentation
test/
  client and transport tests
.vscode/
  VS Code tasks
.zed/
  Zed tasks
```

## Release

Bump the version in `Cargo.toml`, build a release binary, then package it:

```bash
cargo build --release
mkdir -p dist/can_bridge_daemon-v0.1.1-x86_64-linux
cp target/release/can_bridge_daemon dist/can_bridge_daemon-v0.1.1-x86_64-linux/
cp README.md dist/can_bridge_daemon-v0.1.1-x86_64-linux/
tar -C dist -czf dist/can_bridge_daemon-v0.1.1-x86_64-linux.tar.gz can_bridge_daemon-v0.1.1-x86_64-linux
cd dist && sha256sum can_bridge_daemon-v0.1.1-x86_64-linux.tar.gz > can_bridge_daemon-v0.1.1-x86_64-linux.tar.gz.sha256
```

## More Documentation

- User guide: `docs/user-guide.md`
- Protocol details: `docs/protocol.md`
- gRPC schema: `proto/can_bridge.proto`
- Test scripts: `test/`

## License

GPL-3.0
