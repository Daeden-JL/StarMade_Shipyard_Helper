#!/usr/bin/env python3
"""LAN relay for the shipyard dev server.

macOS's application firewall drops incoming connections to freshly built,
ad-hoc signed binaries, and the rule would have to be renewed after every
rebuild. This relay listens on the network and forwards plain TCP to the dev
server on loopback, so the server can be rebuilt freely. Stop the relay to
take the shipyard off the network again.

    python3 scripts/lan-relay.py [listen_host:port] [target_host:port]

Defaults: 0.0.0.0:4280 -> 127.0.0.1:4281
Note: the server sees every client as 127.0.0.1 while behind the relay.
"""
import asyncio
import sys


def parse(value, default):
    host, _, port = (value or default).rpartition(":")
    return host, int(port)


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        try:
            if writer.can_write_eof():
                writer.write_eof()
        except (ConnectionError, OSError, RuntimeError):
            pass


async def main():
    listen = parse(sys.argv[1] if len(sys.argv) > 1 else None, "0.0.0.0:4280")
    target = parse(sys.argv[2] if len(sys.argv) > 2 else None, "127.0.0.1:4281")

    async def handle(client_reader, client_writer):
        try:
            up_reader, up_writer = await asyncio.open_connection(*target)
        except OSError:
            client_writer.close()
            return
        await asyncio.gather(pipe(client_reader, up_writer), pipe(up_reader, client_writer))
        for writer in (up_writer, client_writer):
            try:
                writer.close()
            except (ConnectionError, OSError, RuntimeError):
                pass

    server = await asyncio.start_server(handle, *listen)
    print(f"relay {listen[0]}:{listen[1]} -> {target[0]}:{target[1]}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
