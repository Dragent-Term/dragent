//! Shared build-slot pool sizing: one GNU-make-compatible jobserver for the
//! whole machine, so parallel `cargo`/`make` builds in any number of agent
//! terminals divide the machine instead of each taking all of it.
//!
//! Pure sizing only (no OS calls): the host owns the pool itself (see
//! `dragent_host::build_slots`). `slot_count` is approximately the core
//! count, capped by memory at roughly one compiler slot per 2.5 GB of RAM —
//! about 6 slots on a 16 GB machine.

/// Bytes of RAM budgeting one compiler slot.
pub const BYTES_PER_BUILD_SLOT: u64 = 2_500_000_000;

/// How many parallel compiler slots this machine gets: the core count,
/// capped by memory (`total_ram_bytes / BYTES_PER_BUILD_SLOT`), at least 1.
pub fn slot_count(num_cpus: usize, total_ram_bytes: u64) -> usize {
    let by_ram = (total_ram_bytes / BYTES_PER_BUILD_SLOT).max(1) as usize;
    num_cpus.max(1).min(by_ram)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_gb_caps_ten_cores_to_six() {
        // ~6 compiler slots on a 16 GB machine, even with 10+ cores.
        assert_eq!(slot_count(10, 16 * 1024 * 1024 * 1024), 6);
        assert_eq!(slot_count(8, 16 * 1024 * 1024 * 1024), 6);
    }

    #[test]
    fn few_cores_or_little_ram_floor_and_bind() {
        // Core-bound small machines keep their cores.
        assert_eq!(slot_count(4, 64 * 1024 * 1024 * 1024), 4);
        // RAM-bound: 8 GB / 2.5 GB ≈ 3.
        assert_eq!(slot_count(32, 8 * 1024 * 1024 * 1024), 3);
        // Degenerate inputs still yield a usable pool.
        assert_eq!(slot_count(0, 0), 1);
        assert_eq!(slot_count(1, 1), 1);
    }
}
