"""Download memory-bound property (contract v1 §9): the download must stream from
the shared per-process fill buffer in chunks <= 64 KiB — O(chunk) memory, never
O(total). The other four SDK languages (Rust/Go/JS/C#) assert a streaming/memory
bound; Python did not (coverage-libs-sdks-frontend-2026-07.md). A regression to
``b"\\x42" * total`` (materialising the whole payload) would pass every
functional test yet blow up RSS on a large download.

stdlib unittest (pytest also collects it), matching test_conformance.py.
"""

from __future__ import annotations

import unittest

from harness import auth_headers, make_pair
from laghound import _core


class TestFillChunksMemoryBound(unittest.TestCase):
    """Directly exercise the streaming generator that provides the O(chunk)
    guarantee — the strongest assertion is object IDENTITY (no allocation)."""

    def test_full_chunks_reuse_the_shared_buffer(self):
        total = 8 * 1024 * 1024 + 123  # 128 full 64 KiB chunks + a 123-byte tail
        chunks = list(_core._fill_chunks(total))

        # Correctness: the stream sums to exactly the requested byte count.
        self.assertEqual(sum(len(c) for c in chunks), total)
        # Bounded: no chunk exceeds the 64 KiB cap — never one giant blob.
        self.assertTrue(all(len(c) <= _core.CHUNK_BYTES for c in chunks))
        # Actually streamed: a multi-MiB payload is many chunks, not one.
        self.assertGreater(len(chunks), 1)

        full = chunks[:-1]
        self.assertEqual(len(full), total // _core.CHUNK_BYTES)
        # THE memory-bound guarantee, proven by IDENTITY: every full chunk is the
        # single shared buffer object, so no per-chunk allocation happens. `is`,
        # not `==`, is the whole point — `==` would also pass a fresh-alloc
        # regression that this test exists to catch.
        self.assertTrue(
            all(c is _core._FILL for c in full),
            "full download chunks must reuse the shared _FILL buffer, not allocate",
        )
        # The remainder is a bounded slice of that same buffer (<= 64 KiB).
        self.assertEqual(chunks[-1], _core._FILL[:123])

    def test_exact_multiple_has_no_remainder_chunk(self):
        chunks = list(_core._fill_chunks(_core.CHUNK_BYTES * 4))
        self.assertEqual(len(chunks), 4)
        self.assertTrue(all(c is _core._FILL for c in chunks))

    def test_zero_bytes_yields_nothing(self):
        self.assertEqual(list(_core._fill_chunks(0)), [])

    def test_sub_chunk_request_is_a_single_bounded_slice(self):
        chunks = list(_core._fill_chunks(100))
        self.assertEqual(len(chunks), 1)
        self.assertEqual(chunks[0], _core._FILL[:100])
        self.assertLessEqual(len(chunks[0]), _core.CHUNK_BYTES)


class TestDownloadEndpointStreamsExactBytes(unittest.TestCase):
    """End-to-end: the endpoint wires the streaming generator and reports the
    exact size, over both the WSGI and ASGI adapters."""

    def test_large_download_returns_exact_total_over_both_adapters(self):
        size = 512 * 1024  # > CHUNK_BYTES, so the body must be multi-chunk
        for client in make_pair():
            result, _ = client.request(
                "GET", "/laghound/download", headers=auth_headers(), query="bytes=%d" % size
            )
            self.assertEqual(result.status, 200, client.kind)
            self.assertEqual(len(result.body), size, client.kind)
            self.assertEqual(result.header("content-length"), str(size), client.kind)


if __name__ == "__main__":
    unittest.main()
