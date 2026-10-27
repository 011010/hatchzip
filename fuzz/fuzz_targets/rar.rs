#![no_main]

use libfuzzer_sys::fuzz_target;

// Built only with the rar feature, which compiles UnRAR's C++ source. Always
// a single volume: the first volume of a set is still read header by header
// before it is refused as incomplete.
fuzz_target!(|data: &[u8]| {
    hatchzip_fuzz::check("a.rar", data);
});
