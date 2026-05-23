//! ObjectStore backends.

pub mod local;
pub mod memory;

pub use local::LocalFsBackend;
pub use memory::MemoryBackend;
