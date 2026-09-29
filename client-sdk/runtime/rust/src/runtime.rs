//! The core-owned executor. All async work and all completion callbacks run
//! on these threads (ABI.md §6); host threads are never borrowed.

use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

static RT: OnceLock<Runtime> = OnceLock::new();

pub fn get() -> &'static Runtime {
    RT.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("acme-core")
            .enable_all()
            .build()
            .expect("failed to start acme core runtime")
    })
}
