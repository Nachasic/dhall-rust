use alloc::boxed::Box;

use crate::error::Error;

// Compute the sha256 hash of a bitstring.
pub fn sha256_hash(data: &[u8]) -> Box<[u8]> {
    use sha2::Digest;
    sha2::Sha256::digest(data).as_slice().into()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
pub fn read_binary_file(path: impl AsRef<std::path::Path>) -> Result<Box<[u8]>, Error> {
    use std::io::Read;
    let mut buffer = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buffer)?;
    Ok(buffer.into())
}
