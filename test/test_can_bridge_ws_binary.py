#!/usr/bin/env python3
import argparse
import asyncio
import logging
import struct
import time
from dataclasses import dataclass
from typing import List, Optional, Tuple, Union

import websockets

logger = logging.getLogger("ws-bin-test")
logger.setLevel(logging.DEBUG)
handler = logging.StreamHandler()
handler.setLevel(logging.DEBUG)
handler.setFormatter(logging.Formatter("PY %(asctime)s %(levelname)s %(message)s"))
logger.handlers[:] = [handler]

WS_URL = "ws://127.0.0.1:9501/ws/binary"

UNARY_TIMEOUT = 3.0
WAIT_FOR_FRAMES_SECONDS = 10.0

MAGIC = b"CBD1"
HEADER_LEN = 12

# Update these to match YOUR updated Rust MsgType codes
MT_HELLO_ACK = 1  # daemon -> client (old Hello)
MT_PONG = 2
MT_IFACES = 3
MT_SUBSCRIBED = 4
MT_UNSUBSCRIBED = 5
MT_SEND_ACK = 6
MT_FRAME_EVENT = 7
MT_ERROR = 8

MT_CLIENT_HELLO = 101  # client -> daemon (old HelloAck)
MT_PING = 102
MT_LIST_IFACES = 103
MT_SUBSCRIBE = 104
MT_UNSUBSCRIBE = 105
MT_SEND_FRAME = 106


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
    if len(buffer) < HEADER_LEN:
        return None
    if buffer[0:4] != MAGIC:
        raise RuntimeError(f"bad magic: {buffer[0:4]!r}")
    msg_type = struct.unpack_from("<H", buffer, 4)[0]
    payload_len = struct.unpack_from("<I", buffer, 8)[0]
    total_len = HEADER_LEN + payload_len
    if len(buffer) < total_len:
        return None
    payload = bytes(buffer[HEADER_LEN:total_len])
    del buffer[:total_len]
    return msg_type, payload


def fmt_can_id(can_id: int) -> str:
    return f"0x{can_id:08X}"


# -------- encode client requests --------
def encode_client_hello(client: str, protocol: str = "binary") -> bytes:
    return build_packet(MT_CLIENT_HELLO, pack_string(client) + pack_string(protocol))


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
    iface: str, can_id: int, is_fd: bool, brs: bool, esi: bool, data: bytes
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


# -------- decode daemon messages --------
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
    if msg_type == MT_HELLO_ACK:
        version, off = unpack_string(payload, off)
        server_name, off = unpack_string(payload, off)
        feature_count, off = unpack_u16(payload, off)
        features = []
        for _ in range(feature_count):
            s, off = unpack_string(payload, off)
            features.append(s)
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
        err, off = unpack_string(payload, off)
        return SendAck(ok, None if err == "" else err)

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
        msg, off = unpack_string(payload, off)
        return ErrorMsg(msg)

    raise RuntimeError(f"unknown daemon msg_type={msg_type}")


async def recv_one(ws, buffer: bytearray, timeout_s: float) -> DaemonMsg:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=0.5)
        except asyncio.TimeoutError:
            continue

        if not isinstance(msg, (bytes, bytearray)):
            raise RuntimeError("expected binary ws message")

        buffer.extend(msg)
        parsed = parse_one_packet(buffer)
        if parsed is None:
            continue

        msg_type, payload = parsed
        return decode_daemon_message(msg_type, payload)

    raise RuntimeError("timeout waiting for daemon message")


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default=WS_URL)
    ap.add_argument("--iface", default="can0")
    args = ap.parse_args()

    buffer = bytearray()

    async with websockets.connect(
        args.url,
        open_timeout=UNARY_TIMEOUT,
        ping_interval=None,  # keep ping/pong under your protocol
        max_size=None,  # binary frames can be > default
    ) as ws:
        logger.info("connected to %s", args.url)

        # client speaks first
        await ws.send(encode_client_hello("py-ws-binary-test", "binary"))
        logger.info("CLIENT_HELLO sent")

        hello_ack = await recv_one(ws, buffer, timeout_s=3.0)
        if not isinstance(hello_ack, HelloAck):
            raise RuntimeError(f"expected HelloAck, got {hello_ack}")
        logger.info(
            "HELLO_ACK rx: version=%s server=%s features=%s",
            hello_ack.version,
            hello_ack.server_name,
            hello_ack.features,
        )

        # basic ping
        await ws.send(encode_ping(123))
        pong = await recv_one(ws, buffer, timeout_s=2.0)
        logger.info("pong=%s", pong)

        # list ifaces
        await ws.send(encode_list_ifaces())
        resp = await recv_one(ws, buffer, timeout_s=2.0)
        logger.info("list_ifaces resp=%s", resp)

        # subscribe
        await ws.send(encode_subscribe([args.iface]))
        sub = await recv_one(ws, buffer, timeout_s=2.0)
        logger.info("subscribed=%s", sub)

        # send some frames + print frame events as they come
        logger.warning(
            "sending frames to iface=%s for 3s and waiting for FrameEvent", args.iface
        )
        end = time.time() + 3.0
        saw_frame = False
        counter = 0

        while time.time() < end:
            payload = bytes([counter & 0xFF, 0xAA, 0x55, 0x00])
            await ws.send(
                encode_send_frame(args.iface, 0x123, False, False, False, payload)
            )
            counter += 1

            # try to read frames without blocking too long
            try:
                msg = await recv_one(ws, buffer, timeout_s=0.2)
            except Exception:
                await asyncio.sleep(0.05)
                continue

            if isinstance(msg, FrameEvent):
                saw_frame = True
                logger.info(
                    "FRAME rx: ts_ms=%d iface=%s dir=%s id=%s data_hex=%s",
                    msg.ts_ms,
                    msg.iface,
                    msg.dir,
                    fmt_can_id(msg.can_id),
                    msg.data.hex(),
                )

        if not saw_frame:
            logger.warning(
                "no FrameEvent observed (use vcan0 or replay/fake mode for deterministic tests)"
            )

        # unsubscribe
        await ws.send(encode_unsubscribe())
        unsub = await recv_one(ws, buffer, timeout_s=2.0)
        logger.info("unsubscribed=%s", unsub)

    logger.info("done")


if __name__ == "__main__":
    asyncio.run(main())
