#![no_main]

use libfuzzer_sys::fuzz_target;

// Named .tar so a header without the ustar magic, as old tar writes, is
// still parsed as tar.
fuzz_target!(|data: &[u8]| {
    hatchzip_fuzz::check("a.tar", data);
});
