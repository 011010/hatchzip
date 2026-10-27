#![no_main]

use libfuzzer_sys::fuzz_target;

// A name without an extension, so the content alone decides the format.
fuzz_target!(|data: &[u8]| {
    hatchzip_fuzz::check("archive", data);
});
