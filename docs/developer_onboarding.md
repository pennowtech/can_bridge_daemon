# Developer onboarding guide for contributors

This guide helps you build, run, test, and extend the CAN Bridge Daemon.

## 1. Repo layout (typical)

- `src/`
  - `domain/` – transport-agnostic core logic (CAN operations, routing, frames, subscription state)
  - `infra/transport_tcp/` – TCP JSONL + TCP Binary servers
  - `infra/transport_ws/` – WS JSONL + WS Binary servers
  - `infra/transport_grpc/` – gRPC server adapter
  - `protocol/` – message schemas (JSON structs, binary framing, MsgType enums)
- `test/`
  - Python integration tests for ws/tcp/grpc
- `proto/` or `*.proto`
  - gRPC protobuf definitions

(Names may change in future, but the idea is: **core domain** + **adapters/transports**.)

---

## 2. Prerequisites

Prerequisites are needed to build, run, and test the project. They are already
mentioned in the Readme, but here is a consolidated list.

### System

- Linux (recommended for SocketCAN)
- Rust toolchain (stable)
- Python 3.12+ (for tests)
- `iproute2` tools (for vcan)
- Optional: `grpcurl`, `websocat`

### Python env

If you use Poetry:

```bash
poetry install
```

---

## 3. Build & run

### Create a virtual CAN interface

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

### Run daemon

Example:

```bash
cargo run -- \
  --tcp-bind 127.0.0.1:9500 \
  --ws-bind 127.0.0.1:9501 \
  --grpc-bind 127.0.0.1:9502
```

---

## 3. Adding a new feature safely

### Rule of thumb

Make changes in this order:

1. **Domain behavior** (transport-agnostic)
2. Update **protocol schema** (JSONL + binary + proto, if needed)
3. Update **each transport adapter**
4. Update **tests**
5. Update **docs**

### Keep schemas in sync

* JSONL message names and fields must match tests.
* Binary `MsgType` IDs must match both server and client test encoding/decoding.
* Protobuf generation must be re-run after `.proto` changes.

---

## 4. Debugging workflow

### Enable verbose logs

Use `RUST_LOG`:

```bash
RUST_LOG=debug cargo run -- ...
```

---

## 5. Code style expectations

* Keep transport adapters thin (decode → domain request → encode)
* Put business logic in domain
* Add tests when changing protocol behavior
* Avoid breaking MsgType IDs once released

---

## 6. Release checklist

* Run all tests
* Verify JSONL handshake and binary handshake both client-first
* Confirm `/ws/jsonl` and `/ws/binary` routes
* Confirm gRPC `Hello` works
* Update docs and examples


## 7. “What goes where?” contributor rules

Reference [What goes where](./archs/app_design.md)

This is the checklist that prevents architecture drift.

### Domain (`domain/`)

✅ Allowed:

* CAN types and validation helpers (pure)
* enums for core commands/events
* error types that don’t depend on IO
  🚫 Not allowed:
* JSON parsing, network code, tokio tasks
* SocketCAN syscalls

### Application (`app/`)

✅ Allowed:

* Use-cases, orchestration, policies
* subscription tracking / fan-out rules
* mapping outbound adapter errors to core errors
* timeouts and retry/backoff policies (if needed)
  🚫 Not allowed:
* Binding sockets directly
* Writing websocket frames directly

### Ports (`ports/`)

✅ Allowed:

* Traits/interfaces for inbound + outbound
* DTOs owned by core (transport-agnostic)
  🚫 Not allowed:
* Protocol-specific types (protobuf, websocket message types)

### Transport adapters (`transport/`)

✅ Allowed:

* Wire parsing/serialization
* Connection lifecycle, per-conn tasks
* translating core errors to transport errors
  🚫 Not allowed:
* Subscription logic, per-iface fan-out policy
* Anything that touches SocketCAN directly

### Infra adapters (`infra/`)

✅ Allowed:

* SocketCAN syscalls, netlink/ioctl
* Logging/metrics plumbing
* Config parsing
  🚫 Not allowed:
* Transport parsing logic
* Subscription semantics

---
