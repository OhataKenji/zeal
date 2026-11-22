// A synchronous gRPC RateDesk server for the parcel-desk example. Domain
// rules live in rates/tariff; this file only maps the wire protocol.

#include <iostream>
#include <memory>
#include <string>

#include <grpcpp/grpcpp.h>

#include "api/rates.grpc.pb.h"
#include "rates/tariff.h"

namespace {

using dispatch::rates::v1::Parcel;
using dispatch::rates::v1::Quote;
using dispatch::rates::v1::RateDesk;

class RateDeskServiceImpl final : public RateDesk::Service {
 public:
  grpc::Status QuoteParcel(grpc::ServerContext* /*context*/,
                           const Parcel* request, Quote* reply) override {
    const auto estimate =
        dispatch::rates::v1::QuoteRate(request->zone(), request->weight_grams());
    if (!estimate.has_value()) {
      return grpc::Status(grpc::StatusCode::INVALID_ARGUMENT,
                          "parcel is not quotable");
    }
    reply->set_price_cents(estimate->price_cents);
    reply->set_transit_days(estimate->transit_days);
    return grpc::Status::OK;
  }
};

}  // namespace

int main(int argc, char** argv) {
  const std::string port = argc == 2 ? argv[1] : "50051";
  if (argc > 2 || port.empty() || port.size() > 5 ||
      port.find_first_not_of("0123456789") != std::string::npos ||
      std::stoi(port) > 65535) {
    std::cerr << "usage: " << argv[0] << " [port] (127.0.0.1 only)"
              << std::endl;
    return 1;
  }
  const std::string address = "127.0.0.1:" + port;

  RateDeskServiceImpl service;
  grpc::ServerBuilder builder;
  int selected_port = 0;
  builder.AddListeningPort(address, grpc::InsecureServerCredentials(),
                           &selected_port);
  builder.RegisterService(&service);
  std::unique_ptr<grpc::Server> server = builder.BuildAndStart();
  if (server == nullptr || selected_port == 0) {
    std::cerr << "failed to bind " << address << std::endl;
    return 1;
  }
  std::cout << "listening on 127.0.0.1:" << selected_port << std::endl;
  server->Wait();
  return 0;
}
