//! Print StrCode32 / StrCode64 for each argument.
fn main() {
    for a in std::env::args().skip(1) {
        println!(
            "{a} {:#010x} {:#014x}",
            fox_hash::str_code32(a.as_bytes()),
            fox_hash::str_code64(a.as_bytes())
        );
    }
}
