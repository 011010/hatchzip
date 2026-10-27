#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    hatchzip_fuzz::check("a.zip", data);
});
