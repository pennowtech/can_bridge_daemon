import json
import logging
import socket
import time

logging.basicConfig(
    level=logging.DEBUG, format="PY %(asctime)s %(levelname)s %(message)s"
)


def send_jsonl(sock, obj):
    line = json.dumps(obj) + "\n"
    logging.debug("-> %s", line.strip())
    sock.sendall(line.encode("utf-8"))


def recv_jsonl(sock):
    buf = b""
    while b"\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("server closed")
        buf += chunk
    line, _ = buf.split(b"\n", 1)
    text = line.decode("utf-8")
    logging.debug("<- %s", text)
    return json.loads(text)


def main():
    s = socket.create_connection(("127.0.0.1", 9500), timeout=3)

    hello = recv_jsonl(s)
    assert hello["type"] == "hello", hello
    send_jsonl(s, {"type": "hello_ack", "client": "py-test", "protocol": "jsonl"})

    send_jsonl(s, {"type": "list_ifaces"})
    resp = recv_jsonl(s)
    assert resp["type"] == "ifaces", resp
    logging.info("ifaces=%s", resp["items"])

    send_jsonl(s, {"type": "ping", "id": 9})
    pong = recv_jsonl(s)
    assert pong["type"] == "pong" and pong["id"] == 9, pong

    time.sleep(0.2)

    logging.info("Subscribing  to read can packets from iface = %s", resp["items"][0])

    send_jsonl(s, {"type": "subscribe", "ifaces": [resp["items"][0]]})
    resp = recv_jsonl(s)
    assert resp["type"] == "subscribed"

    # read frames for 2 seconds
    t0 = time.time()
    n = 0
    while time.time() - t0 < 2.0:
        obj = recv_jsonl(s)
        if obj["type"] == "frame":
            n += 1

    logging.info("received %d frames in 2s (%.1f fps)", n, n / 2.0)

    send_jsonl(s, {"type": "unsubscribe"})
    resp = recv_jsonl(s)
    assert resp["type"] == "unsubscribed"

    s.close()


if __name__ == "__main__":
    main()
