pub mod stt;

// The speech engine allocates large buffers in every layer. The system heap
// hands each one back to Windows, and the page faults on the next take stall
// all threads. The binaries choose this allocator too.
#[cfg(test)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
