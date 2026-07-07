# Protocol

## Big picture

### TCP

* On new connection, daemon peeks first 4 bytes:
  * `CBD1` -> **binary session**
  * otherwise -> **JSONL session** or text messages

### WebSocket

* If client sends **Text** messages -> JSON mode (existing).
* If client sends **Binary** messages -> binary mode.

### gRPC

* gRPC is already efficient binary protocol; no changes needed.


## Binary packet framing (TCP + WS binary)

Every packet:

| Field    | Size | Notes                    |
| -------- | ---: | ------------------------ |
| magic    |    4 | ASCII or `CBD1`             |
| msg_type |  u16 | little-endian            |
| flags    |  u16 | little-endian (reserved) |
| len      |  u32 | payload length           |
| payload  |  len | msg-specific             |

### msg_type (Message types)

```txt
1  HELLO_ACK
2  CLIENT_HELLO
3  PING
4  PONG
5  LIST_IFACES
6  IFACES
7  SUBSCRIBE
8  UNSUBSCRIBE
9 SEND_FRAME
10 SEND_ACK
11 FRAME_EVENT
12 ERROR
```

### flags

* **`flags` is reserved** for compression, encryption marker, etc. Right now,
ignored in decode algorithm in code.

### payload(Payload schemas)

**Strings:** `u16 length` + UTF-8 bytes
**Bytes:** raw, preceded by `u16 length` if variable

* `HELLO_ACK`: `string client_name`
* `CLIENT_HELLO`: `string server_name`, `u16 version_major`, `u16 version_minor`
* `PING`: `u64 id`
* `PONG`: `u64 id`
* `LIST_IFACES`: (empty)
* `IFACES`: `u16 count` + repeated `string iface`
* `SUBSCRIBE`: `u16 count` + repeated `string iface`
* `UNSUBSCRIBE`: empty
* `SEND_FRAME`:
  * `string iface`
  * `u32 can_id`
  * `u8 is_fd`
  * `u8 brs`
  * `u8 esi`
  * `u16 data_len`
  * `data bytes`
* `SEND_ACK`: `u8 ok` + `string error` (empty if ok=1)
* `FRAME_EVENT`:
  * `u64 ts_ms`
  * `string iface`
  * `u8 dir` (0=rx, 1=tx)
  * `u32 can_id`
  * `u8 is_fd`
  * `u16 data_len`
  * `data bytes`
* `ERROR`: `string message`

---

## Working

### TCP transport

On accept:

* Peek first 4 bytes with a short timeout:
  * If `CBD1`, enter **binary session**
  * Else, treat as **JSONL session**

Binary session:

* Read from socket into a buffer
* Repeatedly `decode_packet()`

* Route messages:
  * subscribe handled per-connection (like JSON)
  * others go to `service.handle_binary(msg).await`
* Stream outgoing `FRAME_EVENT` packets to client

### WebSocket transport

* If incoming WS message is:
  * `Text` -> JSON mode (existing)
  * `Binary` -> binary mode:
    * decode packets from the received binary (WS message boundaries are not
      guaranteed to align with our packet boundaries if you ever chunk; but
      typically each WS binary can contain 1 packet—still handle multiple packets
      for robustness)

* For outgoing:

  * JSON mode: send `Text`
  * Binary mode: send `Binary(packet_bytes)`

---

## The wire format (what goes on the TCP/WebSocket stream)

Every message is:

```
[Header 12 bytes] + [Payload N bytes]
```

### Header layout (12 bytes)

| Offset | Size | Field       | Meaning                                  |
| -----: | ---: | ----------- | ---------------------------------------- |
|      0 |    4 | MAGIC       | Always `b"CBD1"`                         |
|      4 |    2 | msg_type    | `u16` little-endian (1..10)              |
|      6 |    2 | flags       | `u16` little-endian (currently always 0) |
|      8 |    4 | payload_len | `u32` little-endian                      |

So header bytes are built by `encode()` like this:

* write `"CBD1"`
* write `msg_type as u16` (LE)
* write `0u16` flags (LE)
* write `payload.len() as u32` (LE)
* then the payload

---

## 2) Sending a message (encode path)

### `encode(msg: &BinMsg) -> Vec<u8>`

1. Calls `encode_payload(msg)` which returns:

   * the `MsgType` enum value (HelloAck, Ping, …)
   * a `Vec<u8>` containing the payload bytes
2. Builds the header (12 bytes)
3. Appends the payload

### How strings are encoded

Write:

```
[u16 length LE] + [UTF-8 bytes]
```

Length is capped at `u16::MAX` (65535).

### How lists of strings are encoded (e.g. Subscribe or list of interfaces)

* First `u16 count` for number of strings
* Then each string with the above string encoding

---

## 3) Receiving messages (decode path)

In real networking we usually receive **chunks**, not “one message at a time”.
So we keep a buffer and repeatedly try to decode:

### `decode_from(buf: &[u8]) -> Option<(BinMsg, consumed_bytes)>`

1. If fewer than 12 bytes: `Ok(None)` (need more data)
2. Check MAGIC: if not `CBD1`, error `bad magic`
3. Read:

   * `msg_type` (u16 LE)
   * `_flags` (ignored)
   * `len` payload length (u32 LE)
4. If buffer doesn’t contain `12 + len` bytes yet: `Ok(None)`
5. Slice payload and call `decode_payload(ty, payload)`
6. Returns `(msg, 12+len)` so caller can `drain(..consumed)` and continue.

That’s how it supports **multiple messages in a single TCP read** or **a message split across multiple reads**.

---

## Full examples for ALL message types

To explain, let's use readable hex. Remember: all integers are **little-endian**.
Notation:

* `MAGIC = 43 42 44 31` (ASCII “CBD1”)
* `flags = 00 00`

### A) HelloAck (MsgType = 1)

If message is:

`HelloAck { client: "tauri-ui".into() }`

Payload:

* string "tauri-ui" length 7 -> `07 00`
* bytes: `74 61 75 72 69 2d 75 69`

Payload hex:

`07 00 74 61 75 72 69 2d 75 69`

Payload length = 2 + 7 = 9 -> `09 00 00 00`

Header:

* type 1 -> `01 00`
* len 9 -> `09 00 00 00`

Full message hex:

`43 42 44 31  01 00  00 00  09 00 00 00 07 00 74 61 75 72 69 2d 75 69`

### B) ClientHello (MsgType = 2)

Message:

```rust
ClientHello { server: "can-daemon".into(), v_major: 2, v_minor: 9 }
```

Payload:

* server string: length 9 -> `09 00`, bytes `63 61 6e 2d 64 61 65 6d 6f 6e`
* v_major=2 -> `02 00`
* v_minor=9 -> `09 00`

Payload hex:

```
09 00 63 61 6e 2d 64 61 65 6d 6f 6e  02 00 09 00
```

Len = (2+9) + 2 + 2 = 15 -> `0F 00 00 00`

Header type 2 -> `02 00`

---

### C) Ping (MsgType = 3)

```rust
BinMsg::Ping { id: 0x1122334455667788 }
```

Payload is u64 LE:
`88 77 66 55 44 33 22 11`

Len 8 -> `08 00 00 00`
Type 3 -> `03 00`

Full:

```
43 42 44 31  03 00 00 00  08 00 00 00
88 77 66 55 44 33 22 11
```

---

### D) Pong (MsgType = 4)

Same payload as Ping; type changes to 4 -> `04 00`.

---

### E) Subscribe (MsgType = 5)

```rust
BinMsg::Subscribe { ifaces: vec!["can0".into(), "vcan0".into()] }
```

Payload:

* count u16 = 2 -> `02 00`
* "can0": len 4 -> `04 00` + `63 61 6e 30`
* "vcan0": len 5 -> `05 00` + `76 63 61 6e 30`

Payload hex:

```
02 00
04 00 63 61 6e 30
05 00 76 63 61 6e 30
```

Len = 2 + (2+4) + (2+5) = 17 -> `11 00 00 00`
Type 5 -> `05 00`

---

### F) Unsubscribe (MsgType = 6)

```rust
BinMsg::Unsubscribe
```

Payload empty, len 0 -> `00 00 00 00`
Type 6 -> `06 00`

Full header only:

```
43 42 44 31  06 00 00 00  00 00 00 00
```

---

### G) SendFrame (MsgType = 7)

```rust
BinMsg::SendFrame {
  iface: "can0".into(),
  id: 0x18DAF110,
  is_fd: true,
  brs: true,
  esi: false,
  data: vec![0x02,0x10,0x03]
}
```

Payload layout (per `encode_payload`):

1. iface string: `04 00 63 61 6e 30`
2. id u32 LE:

   * 0x18DAF110 -> bytes `10 F1 DA 18`
3. is_fd u8: `01`
4. brs u8: `01`
5. esi u8: `00`
6. data length u16 = 3 -> `03 00`
7. data bytes: `02 10 03`

Payload hex:

```txt
04 00 63 61 6e 30
10 F1 DA 18
01 01 00
03 00
02 10 03
```

Len = (2+4) + 4 + 3 + 2 + 3 = 18 -> `12 00 00 00`
Type 7 -> `07 00`

---

### H) SendAck (MsgType = 8)

Success example:

```rust
BinMsg::SendAck { ok: true, error: "".into() }
```

Payload:

* ok u8 = 1 -> `01`
* error string empty: len 0 -> `00 00` and no bytes

Payload hex:

```txt
01 00 00
```

Len 3 -> `03 00 00 00`
Type 8 -> `08 00`

Failure example:

```rust
BinMsg::SendAck { ok: false, error: "iface not found".into() }
```

* ok `00`
* error string length 14 -> `0E 00` + UTF-8 bytes

---

### I) FrameEvent (MsgType = 9)

```rust
BinMsg::FrameEvent {
  ts_ms: 1700000123456,
  iface: "can0".into(),
  dir: 1,          // you define meaning: e.g. 0=RX,1=TX
  id: 0x123,
  is_fd: false,
  data: vec![0xDE,0xAD,0xBE,0xEF]
}
```

Payload layout:

1. ts_ms u64 LE
2. iface string
3. dir u8
4. id u32 LE
5. is_fd u8
6. data_len u16
7. data

So:

* ts_ms = 1700000123456 (decimal) -> (u64 LE bytes; value depends, but the 
  encoding rule is exactly “8 bytes little-endian”)
* iface "can0": `04 00 63 61 6e 30`
* dir: `01`
* id 0x123 -> u32 LE: `23 01 00 00`
* is_fd false: `00`
* data_len 4: `04 00`
* data: `DE AD BE EF`

---

### J) Error (MsgType = 10)

```rust
BinMsg::Error { message: "bad request".into() }
```

Payload:

* string length 11 -> `0B 00`
* bytes of “bad request”

Type 10 -> `0A 00`

---

## 5) “Reception loop” example (how you’d use `decode_from` correctly)

Typical TCP read loop pseudocode:

1. Have `recv_buf: Vec<u8>`
2. Each socket read: append new bytes into `recv_buf`
3. While `decode_from(&recv_buf)` returns `Some((msg, used))`:

   * handle `msg`
   * remove `used` bytes from the front (drain/split_off)

That’s the intended design of returning `consumed_bytes`.

---


If you want, I can also write:

* TODO: a **table spec** (protocol schema) to drop into README.
