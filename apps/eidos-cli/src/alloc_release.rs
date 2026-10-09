//! Give memory the system allocator keeps after frees back to the system
//! ([`eidos_engine::memory::set_release_hook`]). The platform call lives in
//! `eidos-sysmem`, the workspace's only crate allowed `unsafe`.

/// Install the hook.
pub fn install() {
    eidos_engine::memory::set_release_hook(release);
}

fn release() {
    let _ = eidos_sysmem::release_free_memory();
}
