# Troubleshooting common issues with CAN frame streaming

## If your daemon currently doesn’t emit FrameEvent for TX

Some setups don’t loop back TX frames as RX on real interfaces unless bus ACKs.
For deterministic streaming, use `vcan0` or the daemon’s `--fake/replay` mode
