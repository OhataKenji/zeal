#include "rates/tariff.h"

namespace dispatch::rates::v1 {
namespace {

constexpr uint32_t kMaxGrams = 30'000;
constexpr uint32_t kSurchargeGrams = 500;
constexpr uint32_t kSurchargeCents = 75;

}  // namespace

std::optional<Estimate> QuoteRate(Zone zone, uint32_t grams) {
  if (grams == 0 || grams > kMaxGrams) {
    return std::nullopt;
  }
  Estimate estimate;
  switch (zone) {
    case Zone::LOCAL:
      estimate = {450, 1};
      break;
    case Zone::REGIONAL:
      estimate = {650, 2};
      break;
    case Zone::REMOTE:
      estimate = {900, 4};
      break;
    default:
      return std::nullopt;
  }
  // The first 500 g are included; each additional started 500 g block adds
  // a flat surcharge.
  estimate.price_cents += ((grams - 1) / kSurchargeGrams) * kSurchargeCents;
  return estimate;
}

}  // namespace dispatch::rates::v1
