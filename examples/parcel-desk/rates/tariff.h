// Parcel rating rules for the dispatch domain: zone base rates plus a
// per-started-500-gram surcharge. Pure domain code with no gRPC or Bazel
// dependencies beyond the generated Zone enum.

#pragma once

#include <cstdint>
#include <optional>

#include "api/rates.pb.h"

namespace dispatch::rates::v1 {

struct Estimate {
  uint32_t price_cents;
  uint32_t transit_days;
};

// Returns the quote for a parcel, or std::nullopt when the zone is
// unspecified/unknown or the weight is zero or over 30,000 grams.
std::optional<Estimate> QuoteRate(Zone zone, uint32_t grams);

}  // namespace dispatch::rates::v1
