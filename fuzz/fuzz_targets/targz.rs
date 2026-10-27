#![no_main]

use libfuzzer_sys::fuzz_target;

// Named .tar.gz so any gzip stream reaches the compressed tar reader, even
// when what it decodes to no longer looks like a tar.
fuzz_target!(|data: &[u8]| {
    hatchzip_fuzz::check("a.tar.gz", data);
});
