"""Contract test: quote parcels from the Bazel-built C++ RateDesk server."""

import os
import re
import subprocess

import grpc

from api import rates_pb2
from dispatch.quote import quote

_WORKSPACE = os.environ.get("TEST_WORKSPACE", "parcel_desk")


def _zone(name: str) -> int:
    return rates_pb2.Zone.Value(name)


def _server_binary() -> str:
    return os.path.join(
        os.environ["TEST_SRCDIR"], _WORKSPACE, "rates", "rate_server"
    )


def _assert_invalid(address: str, zone: int, grams: int) -> None:
    try:
        quote(address, zone, grams)
    except grpc.RpcError as error:
        assert error.code() == grpc.StatusCode.INVALID_ARGUMENT, error.code()
    else:
        raise AssertionError("expected INVALID_ARGUMENT for invalid parcel")


def main() -> int:
    process = subprocess.Popen(
        [_server_binary(), "0"], stdout=subprocess.PIPE, text=True
    )
    try:
        line = process.stdout.readline()
        if not line:
            raise AssertionError("server exited before printing its port")
        match = re.fullmatch(r"listening on 127\.0\.0\.1:(\d+)\n", line)
        assert match is not None, f"unexpected server output: {line!r}"
        address = f"127.0.0.1:{match.group(1)}"

        assert quote(address, _zone("LOCAL"), 500) == (450, 1)
        assert quote(address, _zone("REGIONAL"), 1250) == (800, 2)
        assert quote(address, _zone("REMOTE"), 501) == (975, 4)
        _assert_invalid(address, _zone("LOCAL"), 0)
        _assert_invalid(address, _zone("ZONE_UNSPECIFIED"), 100)
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
