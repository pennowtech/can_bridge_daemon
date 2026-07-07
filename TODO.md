* Binary message header: **No checksum / no version in header**: only MAGIC + type/len. Corruption in-stream could desync until MAGIC is seen again.
* **`FrameEvent` does not include `brs` or `esi`** (only `SendFrame` has them). If you need FD flags on events too, extend payload.
* **Decode doesn’t enforce “payload fully consumed”**. It reads what it needs and ignores any extra bytes (rare here, but good to validate if you want strictness).
