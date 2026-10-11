"""The CLI owns its stdin observer until explicit server shutdown."""
import asyncio
import gc
import os

from fake_provider import __main__ as cli


def test_stdin_eof_remains_observable_after_garbage_collection(monkeypatch):
    read_fd, write_fd = os.pipe()
    stream = os.fdopen(read_fd)
    monkeypatch.setattr(cli.sys, "stdin", stream)

    async def start(self, host, port):
        return host, port

    async def serve(self):
        # Let the observer park on the pipe, then collect unreachable tasks.
        entered = asyncio.Event()
        asyncio.get_running_loop().call_soon(entered.set)
        await entered.wait()
        gc.collect()
        assert any(task.get_coro().__qualname__.endswith(".watch") for task in asyncio.all_tasks())
        os.close(write_fd)
        await self.run.shutdown_requested.wait()

    monkeypatch.setattr(cli.ProviderServer, "start", start)
    monkeypatch.setattr(cli.ProviderServer, "serve_until_shutdown", serve)
    try:
        # No scenario request was served; the report must preserve that failure.
        assert asyncio.run(cli.run("web_console_dogfood", "127.0.0.1", 0, None)) == 1
    finally:
        stream.close()
        try:
            os.close(write_fd)
        except OSError:
            pass
