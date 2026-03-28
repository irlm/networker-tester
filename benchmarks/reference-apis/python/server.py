"""AletheBench Python reference API — uvicorn + starlette."""

import sys

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, StreamingResponse
from starlette.routing import Route

CHUNK_SIZE = 8192
CHUNK = bytes([0x42]) * CHUNK_SIZE


async def health(request: Request) -> JSONResponse:
    """GET /health -- runtime identity and version."""
    return JSONResponse(
        {"status": "ok", "runtime": "python", "version": sys.version}
    )


async def download(request: Request) -> StreamingResponse | JSONResponse:
    """GET /download/{size} -- stream `size` bytes of 0x42 in 8 KiB chunks."""
    try:
        size = int(request.path_params["size"])
    except (KeyError, ValueError):
        return JSONResponse({"error": "invalid size"}, status_code=400)

    if size <= 0:
        return JSONResponse({"error": "invalid size"}, status_code=400)

    async def generate():
        remaining = size
        while remaining > 0:
            to_send = min(remaining, CHUNK_SIZE)
            yield CHUNK[:to_send]
            remaining -= to_send

    return StreamingResponse(
        generate(),
        media_type="application/octet-stream",
        headers={"content-length": str(size)},
    )


async def upload(request: Request) -> JSONResponse:
    """POST /upload -- consume full request body, return byte count."""
    total = 0
    async for chunk in request.stream():
        total += len(chunk)
    return JSONResponse({"bytes_received": total})


app = Starlette(
    routes=[
        Route("/health", health, methods=["GET"]),
        Route("/download/{size:int}", download, methods=["GET"]),
        Route("/upload", upload, methods=["POST"]),
    ],
)
