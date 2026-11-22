"""RateDesk client and CLI for the parcel-desk example."""

import argparse

import grpc

from api import rates_pb2
from api import rates_pb2_grpc


def quote(
    address: str, zone: int, grams: int, timeout: float = 5.0
) -> tuple[int, int]:
    """Returns (price_cents, transit_days) for a parcel at `address`."""
    with grpc.insecure_channel(address) as channel:
        stub = rates_pb2_grpc.RateDeskStub(channel)
        reply = stub.QuoteParcel(
            rates_pb2.Parcel(zone=zone, weight_grams=grams), timeout=timeout
        )
    return reply.price_cents, reply.transit_days


def _zone_value(name: str) -> int:
    return rates_pb2.Zone.Value(name.upper())


def _zone_name(zone: int) -> str:
    return rates_pb2.Zone.Name(zone)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zone", required=True, choices=["local", "regional", "remote"])
    parser.add_argument("--grams", required=True, type=int)
    parser.add_argument("--address", default="127.0.0.1:50051")
    args = parser.parse_args()
    zone = _zone_value(args.zone)
    price_cents, transit_days = quote(args.address, zone, args.grams)
    dollars, cents = divmod(price_cents, 100)
    day = "day" if transit_days == 1 else "days"
    print(
        f"{_zone_name(zone)} {args.grams} g: "
        f"${dollars}.{cents:02d}, {transit_days} business {day}"
    )


if __name__ == "__main__":
    main()
