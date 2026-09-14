// Test fixture — minimal iox2-flat header used by the L3 C++ codegen
// snapshot tests so the codegen can read the proto-derived namespace
// without invoking `sb message compile --iox2`. Mirrors what
// sb-iox2-typegen would emit for `std/ImageStamped` in the real tree.
#ifndef SB_GEN_SWARMBOTIX_STD_IMAGESTAMPED_H
#define SB_GEN_SWARMBOTIX_STD_IMAGESTAMPED_H

#include <cstdint>

namespace swarmbotix_std {

struct ImageStamped {
  static constexpr const char* IOX2_TYPE_NAME = "ImageStamped";

  int32_t header_stamp_sec;
  uint32_t header_stamp_nanosec;
};

}  // namespace swarmbotix_std

#endif  // SB_GEN_SWARMBOTIX_STD_IMAGESTAMPED_H
