#!/usr/bin/env python3
"""
TCP Binary Integration Tests for can_bridge_daemon
==================================================

This suite connects to the daemon TCP port and uses the binary framed protocol
(magic "CBD1") with request/response messages aligned with the JSON protocol:

Handshake:
  - daemon sends HelloAck (binary)
  - client sends ClientHello (binary)

Then tests:
  - Ping/Pong
  - ListIfaces/Ifaces
  - SendFrame/SendAck (positive + invalid cases)
  - Subscribe/Subscribed + streaming FrameEvent (with auto traffic injection)
  - Unsubscribe/Unsubscribed

Frames are logged immediately on arrival, with arbitration IDs printed as hex.

Run:
  python3 test_can_bridge_tcp_binary.py --host 127.0.0.1 --port 9500 --iface can0
"""

import argparse
import logging
import socket
import struct
import threading
import time
from dataclasses import dataclass
from typing import List, Optional, Tuple, Union

# -----------------------------
# Logging
# -----------------------------
logger = logging.getLogger("tcp-bin-test")
logger.setLevel(logging.DEBUG)
_handler = logging.StreamHandler()
_handler.setLevel(logging.DEBUG)
_handler.setFormatter(logging.Formatter("PY %(asctime)s %(levelname)s %(message)s"))
logger.handlers[:] = [_handler]


# -----------------------------
# Protocol constants
# -----------------------------
MAGIC = b"CBD1"
HEADER_LEN = 12  # magic(4) + type(u16) + flags(u16) + len(u32)

# MsgType (must match Rust domain/binary.rs)
MT_HELLO = 1
MT_PONG = 2
MT_IFACES = 3
MT_SUBSCRIBED = 4
MT_UNSUBSCRIBED = 5
MT_SEND_ACK = 6
MT_FRAME_EVENT = 7
MT_ERROR = 8

MT_CLIENT_HELLO = 101
MT_PING = 102
MT_LIST_IFACES = 103
MT_SUBSCRIBE = 104
MT_UNSUBSCRIBE = 105
MT_SEND_FRAME = 106


# -----------------------------
# Helpers / primitives
# -----------------------------
class TestFailure(Exception):
    pass


def assert_true(cond: bool, msg: str):
    if not cond:
        raise TestFailure(msg)


def fmt_can_id(can_id: int) -> str:
    return f"0x{can_id:08X}"


def pack_u16(v: int) -> bytes:
    return struct.pack("<H", v)


def pack_u32(v: int) -> bytes:
    return struct.pack("<I", v)


def pack_u64(v: int) -> bytes:
    return struct.pack("<Q", v)


def unpack_u16(b: bytes, off: int) -> Tuple[int, int]:
    return struct.unpack_from("<H", b, off)[0], off + 2


def unpack_u32(b: bytes, off: int) -> Tuple[int, int]:
    return struct.unpack_from("<I", b, off)[0], off + 4


def unpack_u64(b: bytes, off: int) -> Tuple[int, int]:
    return struct.unpack_from("<Q", b, off)[0], off + 8


def pack_string(s: str) -> bytes:
    data = s.encode("utf-8")
    if len(data) > 65535:
        data = data[:65535]
    return pack_u16(len(data)) + data


def unpack_string(b: bytes, off: int) -> Tuple[str, int]:
    ln, off = unpack_u16(b, off)
    s = b[off : off + ln].decode("utf-8", errors="replace")
    return s, off + ln


def build_packet(msg_type: int, payload: bytes) -> bytes:
    flags = 0
    return (
        MAGIC + pack_u16(msg_type) + pack_u16(flags) + pack_u32(len(payload)) + payload
    )


def parse_one_packet(buffer: bytearray) -> Optional[Tuple[int, bytes]]:
    """
    Returns (msg_type, payload) if a full packet is available, else None.
    Removes consumed bytes from buffer.
    """
    if len(buffer) < HEADER_LEN:
        return None
    if buffer[0:4] != MAGIC:
        raise RuntimeError(f"bad magic: {buffer[0:4]!r}")

    msg_type = struct.unpack_from("<H", buffer, 4)[0]
    # flags at 6 ignored
    payload_len = struct.unpack_from("<I", buffer, 8)[0]
    total_len = HEADER_LEN + payload_len

    if len(buffer) < total_len:
        return None

    payload = bytes(buffer[HEADER_LEN:total_len])
    del buffer[:total_len]
    return msg_type, payload


# -----------------------------
# Message encode (client->daemon)
# -----------------------------
def encode_client_hello(client: str, protocol: str = "binary") -> bytes:
    payload = pack_string(client) + pack_string(protocol)
    return build_packet(MT_CLIENT_HELLO, payload)


def encode_ping(ping_id: int) -> bytes:
    return build_packet(MT_PING, pack_u64(ping_id))


def encode_list_ifaces() -> bytes:
    return build_packet(MT_LIST_IFACES, b"")


def encode_subscribe(ifaces: List[str]) -> bytes:
    payload = pack_u16(len(ifaces))
    for iface in ifaces:
        payload += pack_string(iface)
    return build_packet(MT_SUBSCRIBE, payload)


def encode_unsubscribe() -> bytes:
    return build_packet(MT_UNSUBSCRIBE, b"")


def encode_send_frame(
    iface: str,
    can_id: int,
    is_fd: bool,
    brs: bool,
    esi: bool,
    data: bytes,
) -> bytes:
    payload = (
        pack_string(iface)
        + pack_u32(can_id)
        + bytes([1 if is_fd else 0])
        + bytes([1 if brs else 0])
        + bytes([1 if esi else 0])
        + pack_u16(len(data))
        + data
    )
    return build_packet(MT_SEND_FRAME, payload)


# -----------------------------
# Message decode (daemon->client)
# -----------------------------
@dataclass
class HelloAck:
    version: str
    server_name: str
    features: List[str]


@dataclass
class Pong:
    ping_id: int


@dataclass
class Ifaces:
    items: List[str]


@dataclass
class Subscribed:
    ifaces: List[str]


@dataclass
class Unsubscribed:
    pass


@dataclass
class SendAck:
    ok: bool
    error: Optional[str]


@dataclass
class FrameEvent:
    ts_ms: int
    iface: str
    dir: str
    can_id: int
    is_fd: bool
    data: bytes


@dataclass
class ErrorMsg:
    message: str


DaemonMsg = Union[
    HelloAck, Pong, Ifaces, Subscribed, Unsubscribed, SendAck, FrameEvent, ErrorMsg
]


def decode_daemon_message(msg_type: int, payload: bytes) -> DaemonMsg:
    off = 0

    if msg_type == MT_HELLO:
        version, off = unpack_string(payload, off)
        server_name, off = unpack_string(payload, off)
        feature_count, off = unpack_u16(payload, off)
        features = []
        for _ in range(feature_count):
            f, off = unpack_string(payload, off)
            features.append(f)
        return HelloAck(version, server_name, features)

    if msg_type == MT_PONG:
        ping_id, off = unpack_u64(payload, off)
        return Pong(ping_id)

    if msg_type == MT_IFACES:
        count, off = unpack_u16(payload, off)
        items = []
        for _ in range(count):
            s, off = unpack_string(payload, off)
            items.append(s)
        return Ifaces(items)

    if msg_type == MT_SUBSCRIBED:
        count, off = unpack_u16(payload, off)
        ifaces = []
        for _ in range(count):
            s, off = unpack_string(payload, off)
            ifaces.append(s)
        return Subscribed(ifaces)

    if msg_type == MT_UNSUBSCRIBED:
        return Unsubscribed()

    if msg_type == MT_SEND_ACK:
        ok = payload[off] != 0
        off += 1
        error_text, off = unpack_string(payload, off)
        error = None if error_text == "" else error_text
        return SendAck(ok, error)

    if msg_type == MT_FRAME_EVENT:
        ts_ms, off = unpack_u64(payload, off)
        iface, off = unpack_string(payload, off)
        direction, off = unpack_string(payload, off)
        can_id, off = unpack_u32(payload, off)
        is_fd = payload[off] != 0
        off += 1
        dl, off = unpack_u16(payload, off)
        data = payload[off : off + dl]
        return FrameEvent(ts_ms, iface, direction, can_id, is_fd, data)

    if msg_type == MT_ERROR:
        message, off = unpack_string(payload, off)
        return ErrorMsg(message)

    raise RuntimeError(f"unknown daemon msg_type: {msg_type}")


# -----------------------------
# Client connection wrapper
# -----------------------------
class BinaryTcpClient:
    def __init__(self, host: str, port: int, timeout: float = 3.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock: Optional[socket.socket] = None
        self.recv_buffer = bytearray()

        self.reader_thread: Optional[threading.Thread] = None
        self.stop_event = threading.Event()
        self.inbox = []  # only for debug snapshots
        self.queue: "queue.Queue[DaemonMsg]" = __import__("queue").Queue()

    def connect(self):
        self.sock = socket.create_connection(
            (self.host, self.port), timeout=self.timeout
        )
        self.sock.settimeout(0.5)

        # IMPORTANT: send HelloAck immediately so server detects binary mode
        self.send_raw(encode_client_hello(client="py-tcp-binary-test", protocol="tcp"))
        logger.info("HELLO_ACK sent (binary preface)")

        # Expect HelloAck from daemon
        hello_ack = self._recv_one(deadline=time.time() + 3.0)
        assert_true(
            isinstance(hello_ack, HelloAck), f"expected HelloAck, got {hello_ack}"
        )
        logger.info(
            "HELLO_ACK rx: version=%s server=%s features=%s",
            hello_ack.version,
            hello_ack.server_name,
            hello_ack.features,
        )

        # Send ClientHello
        self.send_raw(
            encode_client_hello(client="py-tcp-binary-test", protocol="binary")
        )
        logger.info("CLIENT_HELLO sent")

        # Start background reader for streaming tests
        self.reader_thread = threading.Thread(target=self._reader_loop, daemon=True)
        self.reader_thread.start()

    def close(self):
        self.stop_event.set()
        if self.sock:
            try:
                self.sock.close()
            except Exception:
                pass
        if self.reader_thread:
            self.reader_thread.join(timeout=1.0)

    def send_raw(self, packet: bytes):
        assert self.sock is not None
        logger.debug(
            "TX %d bytes: type=%d", len(packet), struct.unpack_from("<H", packet, 4)[0]
        )
        self.sock.sendall(packet)

    def request(
        self, packet: bytes, expect_types: Tuple[type, ...], timeout: float = 2.0
    ) -> DaemonMsg:
        """
        Sends a request and waits for the first message of an expected class.
        """
        self.send_raw(packet)
        end = time.time() + timeout
        while time.time() < end:
            try:
                msg = self.queue.get(timeout=0.25)
            except Exception:
                continue
            if isinstance(msg, expect_types):
                return msg
            # keep non-matching messages visible in logs
            logger.debug("ignoring msg=%s while waiting for %s", msg, expect_types)
        raise TestFailure(f"timeout waiting for {expect_types}")

    # ---- internal ----

    def _reader_loop(self):
        logger.debug("[reader] thread started")
        assert self.sock is not None
        while not self.stop_event.is_set():
            try:
                data = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError as e:
                logger.debug("[reader] socket closed: %s", e)
                break

            if not data:
                logger.debug("[reader] server closed connection")
                break

            self.recv_buffer.extend(data)

            while True:
                parsed = parse_one_packet(self.recv_buffer)
                if parsed is None:
                    break
                msg_type, payload = parsed
                msg = decode_daemon_message(msg_type, payload)

                # live logging for frames
                if isinstance(msg, FrameEvent):
                    logger.info(
                        "FRAME rx: ts_ms=%d iface=%s dir=%s id=%s is_fd=%s data_hex=%s",
                        msg.ts_ms,
                        msg.iface,
                        msg.dir,
                        fmt_can_id(msg.can_id),
                        msg.is_fd,
                        msg.data.hex(),
                    )
                else:
                    logger.debug("MSG rx: %s", msg)

                self.inbox.append(msg)
                self.queue.put(msg)

        logger.debug("[reader] thread exiting")

    def _recv_one(self, deadline: float) -> DaemonMsg:
        """
        Receive exactly one daemon message (used for handshake before reader thread starts).
        """
        assert self.sock is not None
        while time.time() < deadline:
            try:
                data = self.sock.recv(4096)
            except socket.timeout:
                continue
            if not data:
                raise TestFailure("server closed during handshake")
            self.recv_buffer.extend(data)
            parsed = parse_one_packet(self.recv_buffer)
            if parsed:
                msg_type, payload = parsed
                return decode_daemon_message(msg_type, payload)
        raise TestFailure("handshake recv timeout")


# -----------------------------
# Tests
# -----------------------------
def test_ping(client: BinaryTcpClient):
    logger.info("=== test_ping ===")
    msg = client.request(encode_ping(7), expect_types=(Pong,))
    assert_true(msg.ping_id == 7, "pong id mismatch")
    logger.info("✓ ping OK")


def test_list_ifaces(client: BinaryTcpClient) -> List[str]:
    logger.info("=== test_list_ifaces ===")
    msg = client.request(encode_list_ifaces(), expect_types=(Ifaces, ErrorMsg))
    if isinstance(msg, ErrorMsg):
        raise TestFailure(f"list_ifaces error: {msg.message}")
    assert_true(len(msg.items) >= 0, "items should exist")
    logger.info("✓ ifaces: %s", msg.items)
    return msg.items


def test_send_frame_success(client: BinaryTcpClient, iface: str):
    logger.info("=== test_send_frame_success === iface=%s", iface)
    packet = encode_send_frame(
        iface,
        can_id=0x321,
        is_fd=False,
        brs=False,
        esi=False,
        data=bytes.fromhex("deadbeef"),
    )
    msg = client.request(packet, expect_types=(SendAck, ErrorMsg), timeout=3.0)
    if isinstance(msg, ErrorMsg):
        raise TestFailure(f"send_frame error: {msg.message}")
    assert_true(msg.ok, f"send_frame should succeed, error={msg.error}")
    logger.info("✓ send_frame OK")


def test_send_frame_invalid_len_classic(client: BinaryTcpClient, iface: str):
    logger.info("=== test_send_frame_invalid_len_classic === iface=%s", iface)
    data = b"\x00" * 9  # classic CAN > 8
    packet = encode_send_frame(
        iface, can_id=0x123, is_fd=False, brs=False, esi=False, data=data
    )
    msg = client.request(packet, expect_types=(SendAck, ErrorMsg), timeout=3.0)
    if isinstance(msg, ErrorMsg):
        raise TestFailure(f"send_frame error: {msg.message}")
    assert_true(not msg.ok, "expected failure for classic overflow")
    logger.info("✓ classic overflow rejected: %s", msg.error)


def test_subscribe_and_stream(client: BinaryTcpClient, iface: str):
    logger.info("=== test_subscribe_and_stream === iface=%s", iface)

    # Subscribe and expect Subscribed
    msg = client.request(
        encode_subscribe([iface]), expect_types=(Subscribed, ErrorMsg), timeout=3.0
    )
    if isinstance(msg, ErrorMsg):
        raise TestFailure(f"subscribe error: {msg.message}")
    logger.info("✓ subscribed: %s", msg.ifaces)

    logger.warning(
        "EXPECTING CAN TRAFFIC on iface='%s'. Will generate traffic via SendFrame for 10 seconds.",
        iface,
    )

    # Start traffic injection thread
    stop_tx = threading.Event()

    def traffic_sender():
        i = 0
        while not stop_tx.is_set():
            payload = bytes([i & 0xFF, (i >> 8) & 0xFF, 0xAA, 0x55])
            pkt = encode_send_frame(
                iface, can_id=0x123, is_fd=False, brs=False, esi=False, data=payload
            )
            try:
                client.send_raw(pkt)
            except Exception as e:
                logger.warning("traffic send failed: %s", e)
                break
            i += 1
            time.sleep(0.05)

    tx_thread = threading.Thread(target=traffic_sender, daemon=True)
    tx_thread.start()

    # Wait up to 10 seconds for at least 1 frame
    start = time.time()
    received_frames = 0
    sample_frames: List[FrameEvent] = []

    try:
        while time.time() - start < 10.0:
            try:
                msg = client.queue.get(timeout=0.5)
            except Exception:
                continue

            if isinstance(msg, FrameEvent) and msg.iface == iface:
                received_frames += 1
                if len(sample_frames) < 3:
                    sample_frames.append(msg)
                if received_frames >= 1:
                    break
    finally:
        stop_tx.set()
        tx_thread.join(timeout=1.0)

    assert_true(
        received_frames >= 1, "expected at least one FrameEvent within 10 seconds"
    )
    logger.info("✓ streaming receive OK frames=%d", received_frames)
    for i, f in enumerate(sample_frames):
        logger.info(
            "sample[%d]: iface=%s dir=%s id=%s data_hex=%s",
            i,
            f.iface,
            f.dir,
            fmt_can_id(f.can_id),
            f.data.hex(),
        )


def test_unsubscribe(client: BinaryTcpClient):
    logger.info("=== test_unsubscribe ===")
    msg = client.request(
        encode_unsubscribe(), expect_types=(Unsubscribed, ErrorMsg), timeout=3.0
    )
    if isinstance(msg, ErrorMsg):
        raise TestFailure(f"unsubscribe error: {msg.message}")
    logger.info("✓ unsubscribed OK")


# -----------------------------
# Main runner
# -----------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=9500)
    ap.add_argument(
        "--iface",
        default=None,
        help="iface to test (default: vcan0 if present else can0 else first)",
    )
    args = ap.parse_args()

    client = BinaryTcpClient(args.host, args.port)

    failures = 0
    try:
        client.connect()

        test_ping(client)

        ifaces = test_list_ifaces(client)
        iface = args.iface
        if iface is None:
            if "vcan0" in ifaces:
                iface = "vcan0"
            elif "can0" in ifaces:
                iface = "can0"
            elif len(ifaces) > 0:
                iface = ifaces[0]
            else:
                iface = "vcan0"
                logger.warning("no ifaces returned; defaulting to %s", iface)

        test_send_frame_success(client, iface)
        test_send_frame_invalid_len_classic(client, iface)
        test_subscribe_and_stream(client, iface)
        test_unsubscribe(client)

    except Exception as e:
        failures += 1
        logger.error("❌ TEST FAILED: %s", e, exc_info=True)
    finally:
        client.close()

    if failures == 0:
        logger.info("🎉 ALL TCP BINARY TESTS PASSED")
    else:
        logger.error("❌ %d TEST(S) FAILED", failures)


if __name__ == "__main__":
    main()
