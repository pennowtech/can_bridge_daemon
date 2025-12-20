Got it — those errors are because I used `flowchart` syntax (and also included characters like `():` in node labels that some Mermaid renderers choke on). You asked specifically for **`sequenceDiagram`**, so here are **detailed sequence diagrams** for the full `transport_tcp.rs` behavior.

---

## 1) Server accept loop + mode autodetect (JSONL vs Binary)

```mermaid
sequenceDiagram
  autonumber
  participant Main as TcpJsonOrBinaryServer.run
  participant L as TcpListener
  participant C as Client
  participant H as handle_tcp_connection
  participant S as TcpStream

  Main->>L: bind(addr)
  loop accept loop
    L-->>Main: accept() -> (TcpStream, peer)
    Main->>Main: conn_id = atomic++
    Main->>H: spawn handle_tcp_connection(stream, peer, conn_id, service)
  end

  Note over H,S: Connection handler chooses protocol by peeking 4 bytes
  H->>S: peek(4)
  alt first 4 bytes == "CBD1"
    H->>H: handle_binary_session(...)
  else otherwise
    H->>H: handle_jsonl_session(...)
  end
```

---

## 2) JSONL session: handshake, writer task, reader loop, and frame streaming

```mermaid
sequenceDiagram
  autonumber
  participant C as Client(JSONL)
  participant H as handle_jsonl_session
  participant R as ReaderLoop(lines)
  participant W as WriterTask(socket)
  participant B as Broadcast(frames)
  participant SV as BridgeService

  H->>H: split socket into read_half/write_half
  H->>SV: subscribe_frames()
  SV-->>H: frames_rx (broadcast receiver)
  H->>W: spawn writer task(out_rx, frames_rx, subscribed_set)
  H->>W: out_tx.send(HelloAck)

  Note over W,C: Server sends HelloAck immediately on connect
  W-->>C: {"type":"hello_ack",...}\n

  Note over H,C: Client must respond with client_hello within 3 seconds
  H->>R: timeout(3s) read first line
  C-->>R: {"type":"client_hello","client":"...","protocol":"jsonl"}\n
  R-->>H: first_line
  H->>H: parse -> ClientRequest::ClientHello
  H-->>H: handshake OK (log client_hello)

  loop while client sends lines
    C-->>R: JSON request line\n
    R-->>H: next request
    alt Subscribe
      H->>H: update subscribed_set = ifaces
      H->>W: out_tx.send(Subscribed)
      W-->>C: {"type":"subscribed","ifaces":[...]}\n
    else Unsubscribe
      H->>H: subscribed_set.clear()
      H->>W: out_tx.send(Unsubscribed)
      W-->>C: {"type":"unsubscribed"}\n
    else Other request
      H->>SV: service.handle(request)
      SV-->>H: DaemonResponse
      H->>W: out_tx.send(response)
      W-->>C: response as JSON\n
    end
  end

  Note over B,W: Frame streaming happens concurrently in WriterTask
  loop broadcast frames
    B-->>W: FrameEvent
    alt subscribed_set contains ev.iface
      W-->>C: {"type":"frame","id":...,"data_hex":...}\n
    else not subscribed
      W-->>W: drop frame
    end
  end
```

---

## 3) Binary session: handshake, decode loop, writer task, and frame streaming

```mermaid
sequenceDiagram
  autonumber
  participant C as Client(Binary)
  participant H as handle_binary_session
  participant RD as BinaryReadLoop
  participant W as WriterTask(socket)
  participant B as Broadcast(frames)
  participant SV as BridgeService

  H->>SV: subscribe_frames()
  SV-->>H: frames_rx
  H->>W: spawn writer task(out_rx, frames_rx, subscribed_set)

  Note over H,C: Binary handshake expects BinMsg::HelloAck within 3 seconds
  H->>RD: timeout(3s) read bytes until decode yields 1 message
  C-->>RD: CBD1 + HELLO_ACK packet bytes
  RD-->>H: BinMsg::HelloAck(client)

  H->>W: out_tx.send(ClientHello)
  W-->>C: CBD1 + CLIENT_HELLO packet bytes

  loop while socket open
    C-->>RD: CBD1 + packet bytes (possibly multiple)
    RD-->>H: decode_from(buffer) -> BinMsg
    alt Subscribe
      H->>H: update subscribed_set = ifaces
      H->>W: out_tx.send(SendAck ok=true)
      W-->>C: CBD1 + SEND_ACK(ok=1)
    else Unsubscribe
      H->>H: subscribed_set.clear()
      H->>W: out_tx.send(SendAck ok=true)
      W-->>C: CBD1 + SEND_ACK(ok=1)
    else Ping
      H->>W: out_tx.send(Pong)
      W-->>C: CBD1 + PONG
    else SendFrame
      Note over H: maps bytes payload -> hex internally for existing app API
      H->>SV: service.handle(SendFrame{iface,id,is_fd,brs,esi,data_hex})
      SV-->>H: DaemonResponse
      alt DaemonResponse::SendAck
        H->>W: out_tx.send(BinMsg::SendAck)
        W-->>C: CBD1 + SEND_ACK(ok=?, error=?)
      else DaemonResponse::Error
        H->>W: out_tx.send(BinMsg::SendAck ok=false)
        W-->>C: CBD1 + SEND_ACK(ok=0, error=message)
      else unexpected
        H->>W: out_tx.send(BinMsg::Error)
        W-->>C: CBD1 + ERROR(message)
      end
    else Unexpected client message
      H->>W: out_tx.send(Error unexpected client message)
      W-->>C: CBD1 + ERROR
    end
  end

  Note over B,W: Frame streaming happens concurrently in WriterTask
  loop broadcast frames
    B-->>W: FrameEvent
    alt subscribed_set contains ev.iface
      W-->>C: CBD1 + FRAME_EVENT(ts, iface, dir, id, is_fd, data)
    else not subscribed
      W-->>W: drop frame
    end
  end
```

---

## 4) Shutdown behavior (both modes)

```mermaid
sequenceDiagram
  autonumber
  participant C as Client
  participant H as SessionHandler
  participant W as WriterTask
  participant S as Socket

  C-->>S: close / EOF
  H->>H: reader detects read=0 or lines EOF
  H->>H: drop out_tx (closes channel)
  W-->>W: out_rx.recv() returns None
  W-->>W: exit writer loop
  H->>W: await writer_handle
  H-->>H: return Ok
```

---
 **one combined sequence diagram** that shows JSONL and Binary as two lifelines under a single “alt” block


```mermaid
sequenceDiagram
  autonumber
  participant C as Client
  participant Main as TcpServer(run)
  participant L as TcpListener
  participant H as ConnHandler(handle_tcp_connection)
  participant S as TcpStream
  participant SV as BridgeService
  participant B as Broadcast(frames)
  participant W as WriterTask

  Main->>L: bind(addr)
  loop accept loop
    L-->>Main: accept() -> (stream, peer)
    Main->>Main: conn_id = atomic++
    Main->>H: spawn(stream, peer, conn_id, service)
  end

  Note over H,S: Autodetect protocol (same port)
  H->>S: peek(4)
  alt First 4 bytes == "CBD1" (Binary mode)
    Note over H,C: Binary handshake
    H->>SV: subscribe_frames()
    SV-->>H: frames_rx
    H->>W: spawn writer(out_rx, frames_rx, subscribed_set, binary_encode)
    H->>S: read bytes until decode yields 1 message (timeout 3s)
    C-->>S: CBD1 + HELLO_ACK packet
    H-->>H: decode -> BinMsg::HelloAck(client)
    H->>W: send BinMsg::ClientHello
    W-->>C: CBD1 + CLIENT_HELLO packet

    par Streaming frames (binary)
      loop broadcast frames
        B-->>W: FrameEvent
        alt iface subscribed
          W-->>C: CBD1 + FRAME_EVENT(ts,iface,dir,id,is_fd,data)
        else not subscribed
          W-->>W: drop
        end
      end
    and Client requests (binary)
      loop while socket open
        C-->>S: CBD1 + packet bytes
        H-->>H: decode_from(buffer) -> BinMsg
        alt Subscribe/Unsubscribe/Ping
          H->>H: update subscribed_set or build Pong
          H->>W: send BinMsg::SendAck(ok=1) or BinMsg::Pong
          W-->>C: CBD1 + ACK/PONG packet
        else SendFrame
          Note over H: bytes->hex mapping for existing app request
          H->>SV: handle(SendFrame{iface,id,is_fd,brs,esi,data_hex})
          SV-->>H: DaemonResponse
          H->>W: send BinMsg::SendAck(ok,error) or BinMsg::Error
          W-->>C: CBD1 + SEND_ACK/ERROR packet
        else Unexpected
          H->>W: send BinMsg::Error("unexpected client message")
          W-->>C: CBD1 + ERROR packet
        end
      end
    end

  else Otherwise (JSONL mode)
    Note over H,C: JSONL handshake
    H->>SV: subscribe_frames()
    SV-->>H: frames_rx
    H->>W: spawn writer(out_rx, frames_rx, subscribed_set, json_serialize)
    H->>W: send DaemonResponse::HelloAck
    W-->>C: {"type":"hello_ack",...}\n

    H->>S: read first JSON line (timeout 3s)
    C-->>S: {"type":"client_hello","client":"...","protocol":"jsonl"}\n
    H-->>H: parse -> ClientRequest::ClientHello

    par Streaming frames (jsonl)
      loop broadcast frames
        B-->>W: FrameEvent
        alt iface subscribed
          W-->>C: {"type":"frame","id":...,"data_hex":...}\n
        else not subscribed
          W-->>W: drop
        end
      end
    and Client requests (jsonl)
      loop while lines available
        C-->>S: {"type":"..."}\n
        H-->>H: parse -> ClientRequest
        alt Subscribe/Unsubscribe
          H->>H: update subscribed_set
          H->>W: send Subscribed/Unsubscribed
          W-->>C: {"type":"subscribed"/"unsubscribed"}\n
        else Other
          H->>SV: handle(request)
          SV-->>H: DaemonResponse
          H->>W: send response
          W-->>C: response JSON\n
        end
      end
    end
  end

  Note over C,H: Shutdown (both modes)
  C-->>S: close / EOF
  H-->>H: reader ends -> drop out_tx
  W-->>W: out_rx closed -> writer exits
  H-->>H: await writer; end handler
```
